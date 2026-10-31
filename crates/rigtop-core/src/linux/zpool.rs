//! Details of ZFS pools from `zpool status -p` and `zpool list -p`: the
//! vdev layout, each device's state and error counts, the last scrub or
//! resilver and permanent data errors. Both work without root wherever
//! `/dev/zfs` is open to everyone (the default on Ubuntu and others).
//!
//! The pools themselves (and their health) come from the kernel module's
//! statistics ([`super::zfs`]); `zpool` is only run when there are pools,
//! at most every [`ZPOOL_REFRESH`].

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::snapshot::{ZfsDevice, ZfsPool};

/// How often `zpool` is run again.
const ZPOOL_REFRESH: Duration = Duration::from_secs(30);

/// What `zpool status` tells about one pool.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PoolStatus {
    pub devices: Vec<ZfsDevice>,
    pub scan: Option<String>,
    pub data_errors: Option<u64>,
}

/// What one run of `zpool` found: each pool's status, and its size and
/// allocated bytes.
#[derive(Debug, Default)]
struct Found {
    status: HashMap<String, PoolStatus>,
    sizes: HashMap<String, (u64, u64)>,
}

/// Pool details, cached between refreshes.
#[derive(Debug, Default)]
pub struct ZpoolReader {
    last: Option<(Instant, Found)>,
}

impl ZpoolReader {
    /// Creates a reader with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the details `zpool` gives to `pools` (from the kernel module).
    pub fn annotate(&mut self, pools: &mut [ZfsPool], now: Instant) {
        if pools.is_empty() {
            return;
        }
        let fresh = self
            .last
            .as_ref()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) < ZPOOL_REFRESH);
        if !fresh {
            let status = zpool(&["status", "-p"])
                .map(|t| parse_status(&t))
                .unwrap_or_default();
            let sizes = zpool(&["list", "-p", "-H", "-o", "name,size,alloc"])
                .map(|t| parse_list(&t))
                .unwrap_or_default();
            self.last = Some((now, Found { status, sizes }));
        }
        let Some((_, found)) = &self.last else {
            return;
        };
        for pool in pools {
            if let Some(status) = found.status.get(&pool.name) {
                pool.devices = status.devices.clone();
                pool.scan = status.scan.clone();
                pool.data_errors = status.data_errors;
            }
            if let Some(&(size, allocated)) = found.sizes.get(&pool.name) {
                pool.size_bytes = Some(size);
                pool.allocated_bytes = Some(allocated);
            }
        }
    }
}

/// Runs `zpool` with `args` in the C locale; its output, if it succeeded.
fn zpool(args: &[&str]) -> Option<String> {
    let output = Command::new("zpool")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Sections of a pool's configuration after its data vdevs.
const SECTIONS: [&str; 5] = ["logs", "cache", "spares", "special", "dedup"];

/// Each pool of `zpool status -p` output, by name.
pub(crate) fn parse_status(text: &str) -> HashMap<String, PoolStatus> {
    let mut pools = HashMap::new();
    let mut name: Option<String> = None;
    let mut status = PoolStatus::default();
    // The field a continuation line (starting with a tab) belongs to.
    let mut field = "";
    let mut section = String::new();
    let mut pool_indent: Option<usize> = None;
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("pool:")
            && !line.starts_with('\t')
        {
            if let Some(done) = name.take() {
                pools.insert(done, std::mem::take(&mut status));
            }
            name = Some(rest.trim().to_owned());
            field = "";
            section.clear();
            pool_indent = None;
            continue;
        }
        if name.is_none() {
            continue;
        }
        if !line.starts_with('\t') {
            // "  scan: ...", "config:", "errors: ..."
            let Some((key, value)) = line.trim_start().split_once(':') else {
                continue;
            };
            field = key.trim();
            let value = value.trim();
            match field {
                "scan" => status.scan = Some(value.to_owned()),
                "errors" => status.data_errors = parse_data_errors(value),
                _ => {}
            }
            continue;
        }
        let body = &line[1..];
        match field {
            // The progress of a running scan: keep the "% done" part.
            "scan" => {
                if let Some(scan) = &mut status.scan
                    && body.contains("done")
                {
                    scan.push_str(" · ");
                    scan.push_str(body.trim());
                }
            }
            "config" => {
                let indent = body.len() - body.trim_start().len();
                let words: Vec<&str> = body.split_whitespace().collect();
                let Some(&first) = words.first() else {
                    continue;
                };
                if first == "NAME" {
                    continue;
                }
                // The pool's own row comes first, at the outermost indent.
                let Some(base) = pool_indent else {
                    pool_indent = Some(indent);
                    continue;
                };
                if indent == base && SECTIONS.contains(&first) && words.len() == 1 {
                    section = first.to_owned();
                    continue;
                }
                let depth = (indent.saturating_sub(base + 2) / 2) as u8;
                let count = |i: usize| {
                    words
                        .get(i)
                        .and_then(|w| w.parse::<u64>().ok())
                        .unwrap_or(0)
                };
                status.devices.push(ZfsDevice {
                    name: first.to_owned(),
                    depth,
                    section: section.clone(),
                    state: words.get(1).map_or(String::new(), |s| (*s).to_owned()),
                    read_errors: count(2),
                    write_errors: count(3),
                    checksum_errors: count(4),
                });
            }
            _ => {}
        }
    }
    if let Some(done) = name {
        pools.insert(done, status);
    }
    pools
}

/// The count of `errors: 442 data errors, use '-v' for a list`; 0 for
/// `No known data errors`.
fn parse_data_errors(value: &str) -> Option<u64> {
    if value.starts_with("No known data errors") {
        return Some(0);
    }
    value.split_whitespace().next()?.parse().ok()
}

/// Size and allocated bytes by pool, from `zpool list -p -H -o
/// name,size,alloc`.
pub(crate) fn parse_list(text: &str) -> HashMap<String, (u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.to_owned();
            let size = fields.next()?.parse().ok()?;
            let allocated = fields.next()?.parse().ok()?;
            Some((name, (size, allocated)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str, pool: &str) -> PoolStatus {
        parse_status(text).remove(pool).expect("pool parsed")
    }

    #[test]
    fn reads_layout_errors_and_scrub() {
        let tank = one(include_str!("testdata/zpool-status-checksum.txt"), "tank");
        let names: Vec<(&str, u8)> = tank
            .devices
            .iter()
            .map(|d| (d.name.as_str(), d.depth))
            .collect();
        assert_eq!(
            names,
            [
                ("raidz1-0", 0),
                ("virtio-disk1", 1),
                ("virtio-disk2", 1),
                ("virtio-disk3", 1)
            ]
        );
        assert_eq!(tank.devices[2].checksum_errors, 491);
        assert_eq!(tank.data_errors, Some(0));
        assert!(
            tank.scan
                .as_deref()
                .unwrap()
                .starts_with("scrub repaired 27.7M")
        );

        let pool = ZfsPool {
            name: "tank".to_owned(),
            state: "ONLINE".to_owned(),
            devices: tank.devices,
            ..Default::default()
        };
        assert_eq!(pool.layout().as_deref(), Some("raidz1"));
        let troubled: Vec<&str> = pool.troubled_devices().map(|d| d.name.as_str()).collect();
        assert_eq!(troubled, ["virtio-disk2"]);
    }

    #[test]
    fn reads_degraded_mirrors_data_errors_and_sections() {
        let backup = one(include_str!("testdata/zpool-status-degraded.txt"), "backup");
        assert_eq!(backup.devices[2].state, "OFFLINE");
        assert!(backup.devices[2].is_unhealthy());

        let tank = one(
            include_str!("testdata/zpool-status-data-errors.txt"),
            "tank",
        );
        assert_eq!(tank.data_errors, Some(442));
        assert!(tank.scan.as_deref().unwrap().contains("with 442 errors"));

        let logs = one(
            include_str!("testdata/zpool-status-log-spare.txt"),
            "backup",
        );
        let rows: Vec<(&str, &str, &str)> = logs
            .devices
            .iter()
            .map(|d| (d.name.as_str(), d.section.as_str(), d.state.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("mirror-0", "", "ONLINE"),
                ("virtio-disk4", "", "ONLINE"),
                ("virtio-disk5", "", "ONLINE"),
                ("/var/tmp/log1", "logs", "ONLINE"),
                ("/var/tmp/spare1", "spares", "AVAIL"),
            ]
        );
        // A paused scrub keeps its progress.
        let scan = logs.scan.unwrap();
        assert!(
            scan.starts_with("scrub paused since") && scan.contains("75.76% done"),
            "{scan}"
        );
        let pool = ZfsPool {
            devices: logs.devices,
            ..Default::default()
        };
        assert_eq!(pool.layout().as_deref(), Some("mirror"));
        assert_eq!(
            pool.troubled_devices().count(),
            0,
            "a waiting spare is fine"
        );

        let resilvered = one(
            include_str!("testdata/zpool-status-resilvered.txt"),
            "backup",
        );
        assert!(resilvered.scan.unwrap().starts_with("resilvered 36K"));
    }

    #[test]
    fn several_pools_and_sizes() {
        let text = format!(
            "{}\n{}",
            include_str!("testdata/zpool-status-degraded.txt"),
            include_str!("testdata/zpool-status-checksum.txt")
        );
        let pools = parse_status(&text);
        assert_eq!(pools.len(), 2);
        let sizes = parse_list(include_str!("testdata/zpool-list.txt"));
        assert_eq!(sizes["tank"], (2_952_790_016, 315_153_408));
        assert!(parse_status("no pools available\n").is_empty());
        // Devices straight under the pool: a stripe; two mirrors.
        let pool = |names: &[&str]| ZfsPool {
            devices: names
                .iter()
                .map(|n| ZfsDevice {
                    name: (*n).to_owned(),
                    depth: 0,
                    state: "ONLINE".to_owned(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(pool(&["sda", "sdb"]).layout().as_deref(), Some("stripe"));
        assert_eq!(
            pool(&["mirror-0", "mirror-1"]).layout().as_deref(),
            Some("2 × mirror")
        );
    }
}
