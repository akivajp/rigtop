//! CPU package power from RAPL energy counters in `/sys/class/powercap`.
//!
//! The kernel exposes cumulative energy (`energy_uj`, microjoules) per
//! package zone; power is the difference between two samples divided by the
//! elapsed time. AMD CPUs use the same `intel-rapl:N` zone names.
//!
//! Some distributions restrict `energy_uj` to root (it can leak information
//! through power side channels); in that case no power is reported.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{SysRoots, read_trimmed, read_u64, sorted_entries};

/// Tracks RAPL counters between samples to turn energy into power.
#[derive(Debug, Default)]
pub struct RaplMeter {
    /// Package zones discovered on the first sample.
    zones: Option<Vec<Zone>>,
    /// Time of the previous sample.
    last_sample: Option<Instant>,
}

#[derive(Debug)]
struct Zone {
    /// Path to `energy_uj`.
    energy_path: PathBuf,
    /// Counter wrap-around value (`max_energy_range_uj`).
    max_range_uj: u64,
    /// Counter value at the previous sample.
    last_uj: Option<u64>,
}

impl RaplMeter {
    /// Creates a meter. Zones are discovered lazily on the first sample.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes a sample and returns the average package power since the
    /// previous call, summed over all packages.
    ///
    /// Returns `None` on the first call, when no readable zone exists, or
    /// when samples are too close together to be meaningful.
    pub fn sample(&mut self, roots: &SysRoots, now: Instant) -> Option<f32> {
        let zones = self.zones.get_or_insert_with(|| discover_zones(roots));
        if zones.is_empty() {
            return None;
        }
        let elapsed = self
            .last_sample
            .map(|prev| now.saturating_duration_since(prev).as_secs_f64());
        self.last_sample = Some(now);

        let mut total_w = 0.0;
        let mut complete = true;
        for zone in zones.iter_mut() {
            let current = read_u64(&zone.energy_path);
            match (zone.last_uj, current, elapsed) {
                (Some(prev), Some(cur), Some(secs)) => {
                    match power_from_delta(prev, cur, zone.max_range_uj, secs) {
                        Some(w) => total_w += w,
                        None => complete = false,
                    }
                }
                _ => complete = false,
            }
            zone.last_uj = current;
        }
        complete.then_some(total_w as f32)
    }
}

/// Finds top-level package zones (`intel-rapl:0`, `intel-rapl:1`, ...).
/// Sub-zones such as `intel-rapl:0:0` (core, uncore) are already included in
/// their package and are skipped to avoid double counting.
fn discover_zones(roots: &SysRoots) -> Vec<Zone> {
    let class_dir = roots.sys.join("class/powercap");
    sorted_entries(&class_dir)
        .into_iter()
        .filter(|entry| {
            entry.matches(':').count() == 1 && entry.ends_with(|c: char| c.is_ascii_digit())
        })
        .map(|entry| class_dir.join(entry))
        .filter(|dir| is_package_zone(dir))
        .filter_map(|dir| {
            let energy_path = dir.join("energy_uj");
            // Probe readability now so that a permission error is detected once.
            read_u64(&energy_path)?;
            Some(Zone {
                max_range_uj: read_u64(&dir.join("max_energy_range_uj")).unwrap_or(0),
                energy_path,
                last_uj: None,
            })
        })
        .collect()
}

fn is_package_zone(dir: &Path) -> bool {
    read_trimmed(&dir.join("name")).is_some_and(|name| name.starts_with("package"))
}

/// Converts two energy counter readings into watts, handling one wrap-around.
///
/// Returns `None` if the interval is too short or the counter went backwards
/// without a known wrap value.
pub(crate) fn power_from_delta(
    prev_uj: u64,
    cur_uj: u64,
    max_range_uj: u64,
    secs: f64,
) -> Option<f64> {
    // Below ~10 ms the counter resolution dominates the result.
    if secs < 0.01 {
        return None;
    }
    let delta = if cur_uj >= prev_uj {
        cur_uj - prev_uj
    } else if max_range_uj > prev_uj {
        // The counter wrapped past `max_range_uj` back to zero.
        (max_range_uj - prev_uj) + cur_uj
    } else {
        return None;
    };
    Some(delta as f64 / 1_000_000.0 / secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;
    use std::time::Duration;

    #[test]
    fn power_from_plain_delta() {
        // 50 J over 2 s = 25 W.
        let w = power_from_delta(1_000_000, 51_000_000, 0, 2.0).unwrap();
        assert!((w - 25.0).abs() < 1e-9);
    }

    #[test]
    fn power_across_wrap_around() {
        // 10 J before the wrap + 5 J after, over 1 s.
        let w = power_from_delta(90_000_000, 5_000_000, 100_000_000, 1.0).unwrap();
        assert!((w - 15.0).abs() < 1e-9);
    }

    #[test]
    fn power_rejects_tiny_interval_and_unknown_wrap() {
        assert!(power_from_delta(0, 10, 0, 0.001).is_none());
        assert!(power_from_delta(10, 5, 0, 1.0).is_none());
    }

    #[test]
    fn meter_sums_packages_and_skips_subzones() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "sys/class/powercap/intel-rapl:0/name", "package-0\n");
        write(
            root,
            "sys/class/powercap/intel-rapl:0/energy_uj",
            "1000000\n",
        );
        write(
            root,
            "sys/class/powercap/intel-rapl:0/max_energy_range_uj",
            "65532610987\n",
        );
        // A core sub-zone that must not be added on top of its package.
        write(root, "sys/class/powercap/intel-rapl:0:0/name", "core\n");
        write(
            root,
            "sys/class/powercap/intel-rapl:0:0/energy_uj",
            "500000\n",
        );
        let roots = SysRoots {
            sys: root.join("sys"),
            proc: root.join("proc"),
        };

        let mut meter = RaplMeter::new();
        let t0 = Instant::now();
        assert_eq!(meter.sample(&roots, t0), None);

        write(
            root,
            "sys/class/powercap/intel-rapl:0/energy_uj",
            "31000000\n",
        );
        write(
            root,
            "sys/class/powercap/intel-rapl:0:0/energy_uj",
            "99000000\n",
        );
        let w = meter.sample(&roots, t0 + Duration::from_secs(1)).unwrap();
        assert!((w - 30.0).abs() < 1e-3);
    }
}
