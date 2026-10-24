//! btrfs filesystem health from `/sys/fs/btrfs/<fsid>`.
//!
//! Everything read here is world-readable, so RAID profile, allocation and
//! per-device error counters are available without root. (`btrfs device
//! stats` needs root and offers the same counters.)
//!
//! Layout used:
//! - `label`
//! - `devices/<name>`: one symlink per member block device
//! - `devinfo/<devid>/missing`, `devinfo/<devid>/error_stats`
//! - `allocation/<data|metadata|system>/<profile>/{total_bytes,used_bytes}`
//!
//! Scrub results are not in sysfs: btrfs-progs keeps them in
//! `/var/lib/btrfs/scrub.status.<fsid>` (mode 0600). The root collector in
//! `contrib/collect/` copies that file to a world-readable directory; see
//! [`read_scrub`].

use std::fs;
use std::path::{Path, PathBuf};

use super::mounts::MountEntry;
use super::{SysRoots, read_trimmed, read_u64, sorted_entries};
use crate::snapshot::{BtrfsAllocation, BtrfsDeviceStats, BtrfsFilesystem, BtrfsScrub};

/// Where btrfs-progs itself writes scrub status files (readable by root only).
pub const BTRFS_PROGS_STATUS_DIR: &str = "/var/lib/btrfs";

/// Block group types in display order.
const ALLOCATION_KINDS: &[&str] = &["data", "metadata", "system"];

/// Reads every btrfs filesystem known to the kernel and attaches mount points
/// from `mounts`.
pub fn read_filesystems(roots: &SysRoots, mounts: &[MountEntry]) -> Vec<BtrfsFilesystem> {
    let base = roots.sys.join("fs/btrfs");
    sorted_entries(&base)
        .into_iter()
        // `features` is a global directory, not a filesystem.
        .filter(|entry| entry != "features")
        .map(|fsid| read_filesystem(&base.join(&fsid), fsid, mounts))
        .collect()
}

fn read_filesystem(dir: &Path, fsid: String, mounts: &[MountEntry]) -> BtrfsFilesystem {
    let devices = sorted_entries(&dir.join("devices"));
    let device_stats = sorted_entries(&dir.join("devinfo"))
        .into_iter()
        .filter_map(|devid| read_device_stats(&dir.join("devinfo").join(&devid), &devid))
        .collect();
    let allocations = ALLOCATION_KINDS
        .iter()
        .flat_map(|kind| read_allocations(&dir.join("allocation").join(kind), kind))
        .collect();

    // A btrfs mount's source can be any member device, so match on all of them.
    let mut mount_points: Vec<String> = mounts
        .iter()
        .filter(|m| m.fs_type == "btrfs" && devices.contains(&device_basename(&m.device)))
        .map(|m| m.mount_point.clone())
        .collect();
    mount_points.sort_by_key(|m| (m.len(), m.clone()));
    mount_points.dedup();

    BtrfsFilesystem {
        label: read_trimmed(&dir.join("label")).unwrap_or_default(),
        fsid,
        mount_points,
        devices,
        device_stats,
        allocations,
        scrub: None,
    }
}

/// Reads the last scrub result of filesystem `fsid` from the first of `dirs`
/// containing a readable `scrub.status.<fsid>`.
pub fn read_scrub(dirs: &[PathBuf], fsid: &str) -> Option<BtrfsScrub> {
    let name = format!("scrub.status.{fsid}");
    dirs.iter()
        .find_map(|dir| fs::read_to_string(dir.join(&name)).ok())
        .and_then(|text| parse_scrub_status(&text))
}

/// Parses a btrfs-progs scrub status file:
///
/// ```text
/// scrub status:1
/// <fsid>:<devid>|data_bytes_scrubbed:N|...|t_start:EPOCH|duration:S|canceled:0|finished:1
/// ```
///
/// One line per device; the result sums the devices. Returns `None` when the
/// file has no device line or no start time (scrub never ran).
pub(crate) fn parse_scrub_status(text: &str) -> Option<BtrfsScrub> {
    let mut scrub = BtrfsScrub::default();
    let mut started: Option<u64> = None;
    let (mut all_finished, mut any_canceled, mut devices) = (true, false, 0);
    for line in text.lines().filter(|l| l.contains('|')) {
        devices += 1;
        let mut finished = false;
        // Skip the "<fsid>:<devid>" prefix, then read key:value pairs.
        for pair in line.split('|').skip(1) {
            let Some((key, value)) = pair.split_once(':') else {
                continue;
            };
            let Ok(value) = value.trim().parse::<u64>() else {
                continue;
            };
            match key {
                "data_bytes_scrubbed" | "tree_bytes_scrubbed" => scrub.bytes_scrubbed += value,
                "read_errors" | "csum_errors" | "verify_errors" | "super_errors" => {
                    scrub.errors += value
                }
                "corrected_errors" => scrub.corrected += value,
                "uncorrectable_errors" => scrub.uncorrectable += value,
                // Devices run in parallel: earliest start, longest duration.
                "t_start" if value > 0 => started = Some(started.map_or(value, |s| s.min(value))),
                "duration" => scrub.duration_secs = scrub.duration_secs.max(value),
                "canceled" => any_canceled |= value != 0,
                "finished" => finished = value != 0,
                _ => {}
            }
        }
        all_finished &= finished;
    }
    let started = started.filter(|_| devices > 0)?;
    scrub.started_at_ms = started.saturating_mul(1000);
    scrub.state = if any_canceled {
        "canceled"
    } else if all_finished {
        "finished"
    } else {
        "unfinished"
    }
    .to_owned();
    Some(scrub)
}

/// Reads `devinfo/<devid>`. Returns `None` for non-numeric entries.
fn read_device_stats(dir: &Path, devid: &str) -> Option<BtrfsDeviceStats> {
    let mut stats = BtrfsDeviceStats {
        devid: devid.parse().ok()?,
        missing: read_u64(&dir.join("missing")).is_some_and(|v| v != 0),
        ..Default::default()
    };
    // error_stats: one "<name> <count>" pair per line.
    let text = fs::read_to_string(dir.join("error_stats")).unwrap_or_default();
    for line in text.lines() {
        let Some((name, value)) = line.split_once(' ') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<u64>() else {
            continue;
        };
        match name {
            "write_errs" => stats.write_errs = value,
            "read_errs" => stats.read_errs = value,
            "flush_errs" => stats.flush_errs = value,
            "corruption_errs" => stats.corruption_errs = value,
            "generation_errs" => stats.generation_errs = value,
            _ => {}
        }
    }
    Some(stats)
}

/// Reads the profile subdirectories of one block group type. Usually there is
/// exactly one; two appear while a balance converts between profiles.
fn read_allocations(dir: &Path, kind: &str) -> Vec<BtrfsAllocation> {
    sorted_entries(dir)
        .into_iter()
        .filter(|entry| dir.join(entry).is_dir())
        .filter_map(|profile| {
            let profile_dir = dir.join(&profile);
            Some(BtrfsAllocation {
                kind: kind.to_owned(),
                total_bytes: read_u64(&profile_dir.join("total_bytes"))?,
                used_bytes: read_u64(&profile_dir.join("used_bytes"))?,
                profile,
            })
        })
        .collect()
}

/// Maps a mount source such as `/dev/nvme1n1p1` or `/dev/mapper/root` to the
/// kernel device name used under `devices/` (`nvme1n1p1`, `dm-0`).
fn device_basename(device: &str) -> String {
    let path = Path::new(device);
    // Device-mapper names are symlinks to `../dm-N`; resolve them.
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    resolved
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    const FSID: &str = "acb63bf0-ab89-4069-bb64-d3352482c507";

    fn fixture(root: &Path) -> SysRoots {
        let base = format!("sys/fs/btrfs/{FSID}");
        write(root, "sys/fs/btrfs/features/raid1c34", "0\n");
        write(root, &format!("{base}/label"), "\n");
        for dev in ["nvme0n1", "nvme1n1p1", "nvme2n1"] {
            write(root, &format!("{base}/devices/{dev}/size"), "0\n");
        }
        for devid in 1..=3 {
            let corruption = if devid == 2 { 3 } else { 0 };
            write(
                root,
                &format!("{base}/devinfo/{devid}/error_stats"),
                &format!(
                    "write_errs 0\nread_errs 0\nflush_errs 0\ncorruption_errs {corruption}\ngeneration_errs 0\n"
                ),
            );
            write(root, &format!("{base}/devinfo/{devid}/missing"), "0\n");
        }
        write(
            root,
            &format!("{base}/allocation/data/raid0/total_bytes"),
            "1000\n",
        );
        write(
            root,
            &format!("{base}/allocation/data/raid0/used_bytes"),
            "900\n",
        );
        // Files directly under the kind directory must not be taken as profiles.
        write(
            root,
            &format!("{base}/allocation/data/total_bytes"),
            "1000\n",
        );
        write(
            root,
            &format!("{base}/allocation/metadata/raid1c3/total_bytes"),
            "30\n",
        );
        write(
            root,
            &format!("{base}/allocation/metadata/raid1c3/used_bytes"),
            "10\n",
        );
        SysRoots {
            sys: root.join("sys"),
            proc: root.join("proc"),
        }
    }

    fn mount(device: &str, mount_point: &str, fs_type: &str) -> MountEntry {
        MountEntry {
            device: device.to_owned(),
            mount_point: mount_point.to_owned(),
            fs_type: fs_type.to_owned(),
        }
    }

    #[test]
    fn parses_scrub_status_across_devices() {
        let text = "scrub status:1\n\
acb6:1|data_extents_scrubbed:10|data_bytes_scrubbed:1000|tree_bytes_scrubbed:24|read_errors:0|csum_errors:2|verify_errors:0|super_errors:0|uncorrectable_errors:0|corrected_errors:2|last_physical:0|t_start:1792700000|t_resumed:0|duration:3600|canceled:0|finished:1\n\
acb6:2|data_bytes_scrubbed:2000|csum_errors:0|t_start:1792700005|duration:4000|canceled:0|finished:1\n";
        let scrub = parse_scrub_status(text).unwrap();
        assert_eq!(scrub.started_at_ms, 1_792_700_000_000);
        assert_eq!(scrub.duration_secs, 4000);
        assert_eq!(scrub.state, "finished");
        assert_eq!(scrub.bytes_scrubbed, 3024);
        assert_eq!(scrub.errors, 2);
        assert_eq!(scrub.corrected, 2);
    }

    #[test]
    fn scrub_state_and_missing_status() {
        let running = "scrub status:1\nfs:1|t_start:100|finished:0|canceled:0\nfs:2|t_start:100|finished:1|canceled:0\n";
        assert_eq!(parse_scrub_status(running).unwrap().state, "unfinished");
        let canceled = "scrub status:1\nfs:1|t_start:100|finished:0|canceled:1\n";
        assert_eq!(parse_scrub_status(canceled).unwrap().state, "canceled");
        assert!(parse_scrub_status("scrub status:1\n").is_none());

        // The first directory with the file wins; missing directories are skipped.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("copies");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("scrub.status.abc"), canceled).unwrap();
        let dirs = vec![tmp.path().join("absent"), dir];
        assert_eq!(read_scrub(&dirs, "abc").unwrap().state, "canceled");
        assert!(read_scrub(&dirs, "other").is_none());
    }

    #[test]
    fn reads_devices_errors_and_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = fixture(tmp.path());
        let mounts = vec![
            mount("/dev/nvme1n1p1", "/home", "btrfs"),
            mount("/dev/nvme1n1p1", "/", "btrfs"),
            mount("/dev/sda1", "/boot", "ext4"),
        ];

        let filesystems = read_filesystems(&roots, &mounts);
        assert_eq!(filesystems.len(), 1);
        let fs = &filesystems[0];
        assert_eq!(fs.fsid, FSID);
        assert_eq!(fs.devices, vec!["nvme0n1", "nvme1n1p1", "nvme2n1"]);
        assert_eq!(fs.mount_points, vec!["/", "/home"]);

        assert_eq!(fs.device_stats.len(), 3);
        assert_eq!(fs.device_stats[1].devid, 2);
        assert_eq!(fs.device_stats[1].corruption_errs, 3);
        assert!(fs.has_problems());

        let profiles: Vec<(&str, &str)> = fs
            .allocations
            .iter()
            .map(|a| (a.kind.as_str(), a.profile.as_str()))
            .collect();
        assert_eq!(profiles, vec![("data", "raid0"), ("metadata", "raid1c3")]);
        assert_eq!(fs.allocations[0].used_bytes, 900);
    }
}
