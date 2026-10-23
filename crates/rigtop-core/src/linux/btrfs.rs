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

use std::fs;
use std::path::Path;

use super::mounts::MountEntry;
use super::{SysRoots, read_trimmed, read_u64, sorted_entries};
use crate::snapshot::{BtrfsAllocation, BtrfsDeviceStats, BtrfsFilesystem};

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
    }
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
