//! Per-device disk I/O from `/proc/diskstats`.
//!
//! Only whole block devices listed in `/sys/block` are reported (partitions
//! are skipped, as are loop and RAM disks). For btrfs RAID this shows the load
//! on each member device, which per-mount statistics cannot.

use std::collections::HashMap;
use std::fs;
use std::time::Instant;

use super::{SysRoots, sorted_entries};
use crate::snapshot::DiskIo;

/// `/proc/diskstats` always counts in 512-byte sectors, whatever the device's
/// logical block size.
const SECTOR_BYTES: f64 = 512.0;

/// Raw cumulative counters of one device.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Counters {
    sectors_read: u64,
    sectors_written: u64,
    /// Milliseconds spent with I/O in flight.
    io_ticks_ms: u64,
}

/// Turns successive `/proc/diskstats` readings into rates.
#[derive(Debug, Default)]
pub struct DiskStatsMeter {
    previous: HashMap<String, Counters>,
    last_sample: Option<Instant>,
}

impl DiskStatsMeter {
    /// Creates a meter with no history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the current counters and returns rates since the previous call.
    /// Rates are zero on the first call.
    pub fn sample(&mut self, roots: &SysRoots, now: Instant) -> Vec<DiskIo> {
        let whole_disks: Vec<String> = sorted_entries(&roots.sys.join("block"))
            .into_iter()
            .filter(|name| !is_virtual(name))
            .collect();
        let text = fs::read_to_string(roots.proc.join("diskstats")).unwrap_or_default();
        let current = parse(&text);
        let elapsed = self
            .last_sample
            .map(|prev| now.saturating_duration_since(prev).as_secs_f64())
            .filter(|secs| *secs > 0.0);

        let disks = whole_disks
            .iter()
            .filter_map(|name| {
                let cur = current.get(name)?;
                let mut io = DiskIo {
                    name: name.clone(),
                    ..Default::default()
                };
                if let (Some(prev), Some(secs)) = (self.previous.get(name), elapsed) {
                    io.read_bytes_per_sec =
                        cur.sectors_read.saturating_sub(prev.sectors_read) as f64 * SECTOR_BYTES
                            / secs;
                    io.write_bytes_per_sec =
                        cur.sectors_written.saturating_sub(prev.sectors_written) as f64
                            * SECTOR_BYTES
                            / secs;
                    let busy_ms = cur.io_ticks_ms.saturating_sub(prev.io_ticks_ms) as f64;
                    io.busy_percent = (busy_ms / (secs * 1000.0) * 100.0).min(100.0) as f32;
                }
                Some(io)
            })
            .collect();

        self.previous = current;
        self.last_sample = Some(now);
        disks
    }
}

/// Loop devices and RAM disks only add noise to the disk panel.
fn is_virtual(name: &str) -> bool {
    name.starts_with("loop") || name.starts_with("ram")
}

/// Parses `/proc/diskstats` into counters keyed by device name.
pub(crate) fn parse(text: &str) -> HashMap<String, Counters> {
    text.lines()
        .filter_map(|line| {
            // Fields: major minor name reads reads_merged sectors_read ms_reading
            //         writes writes_merged sectors_written ms_writing
            //         in_flight io_ticks ...
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 13 {
                return None;
            }
            let num = |i: usize| fields[i].parse::<u64>().ok();
            Some((
                fields[2].to_owned(),
                Counters {
                    sectors_read: num(5)?,
                    sectors_written: num(9)?,
                    io_ticks_ms: num(12)?,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;
    use std::time::Duration;

    const SAMPLE: &str = "\
   7       0 loop0 14 0 34 0 0 0 0 0 0 0 0 0 0 0 0 0 0
 259       0 nvme0n1 1000 0 2000 10 500 0 4000 20 0 100 30 0 0 0 0 0 0
 259       1 nvme0n1p1 900 0 1800 9 400 0 3000 15 0 90 24 0 0 0 0 0 0
";

    #[test]
    fn parse_reads_sector_and_tick_fields() {
        let counters = parse(SAMPLE);
        assert_eq!(
            counters["nvme0n1"],
            Counters {
                sectors_read: 2000,
                sectors_written: 4000,
                io_ticks_ms: 100
            }
        );
        assert_eq!(counters.len(), 3);
    }

    #[test]
    fn meter_reports_whole_disks_with_rates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // /sys/block lists whole disks only; partitions are absent there.
        write(root, "sys/block/nvme0n1/size", "0\n");
        write(root, "sys/block/loop0/size", "0\n");
        write(root, "proc/diskstats", SAMPLE);
        let roots = SysRoots {
            sys: root.join("sys"),
            proc: root.join("proc"),
        };

        let mut meter = DiskStatsMeter::new();
        let t0 = Instant::now();
        let first = meter.sample(&roots, t0);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].name, "nvme0n1");
        assert_eq!(first[0].read_bytes_per_sec, 0.0);

        // +2048 sectors read (1 MiB), +1024 written, +250 ms busy, over 0.5 s.
        write(
            root,
            "proc/diskstats",
            " 259       0 nvme0n1 1100 0 4048 10 600 0 5024 20 0 350 30 0 0 0 0 0 0\n",
        );
        let second = meter.sample(&roots, t0 + Duration::from_millis(500));
        assert!((second[0].read_bytes_per_sec - 2.0 * 1024.0 * 1024.0).abs() < 1e-6);
        assert!((second[0].write_bytes_per_sec - 1024.0 * 1024.0).abs() < 1e-6);
        assert!((second[0].busy_percent - 50.0).abs() < 1e-3);
    }
}
