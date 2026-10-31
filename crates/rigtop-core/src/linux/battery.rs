//! Laptop batteries from `/sys/class/power_supply`.
//!
//! Each supply has a `type`; system batteries are `Battery`. Batteries of
//! peripherals (wireless mice, keyboards) are also `Battery` but have
//! `scope` set to `Device`, and are left out.

use std::path::Path;

use super::{SysRoots, read_i64, read_trimmed, sorted_entries};
use crate::snapshot::BatteryStats;

/// Weight of a new rate reading in the smoothed rate: about 10 samples
/// (seconds at the default interval) to follow a change, so the time left
/// does not jump with every short load.
const RATE_SMOOTHING: f64 = 0.1;

/// Shortest time left shown; below a few seconds' worth the rate is noise.
const MIN_RATE: f64 = 1e-6;

/// Combined state of the system batteries, or `None` without any, with the
/// time left at the current rate (see [`BatteryMeter`] for a steadier one).
pub fn read_battery(roots: &SysRoots) -> Option<BatteryStats> {
    BatteryMeter::default().read(roots)
}

/// Reads the batteries and smooths the charge or discharge rate between
/// readings for the time left.
#[derive(Debug, Default)]
pub struct BatteryMeter {
    /// Smoothed rate, and the status it was measured in.
    rate: Option<(String, f64)>,
}

impl BatteryMeter {
    /// Combined state of the system batteries, or `None` without any.
    pub fn read(&mut self, roots: &SysRoots) -> Option<BatteryStats> {
        let class_dir = roots.sys.join("class/power_supply");
        let batteries: Vec<Reading> = sorted_entries(&class_dir)
            .iter()
            .filter_map(|entry| read_one(&class_dir.join(entry)))
            .collect();
        if batteries.is_empty() {
            self.rate = None;
            return None;
        }
        // Weight the charge by capacity when energies are known (two batteries
        // of different sizes); otherwise average the percentages.
        let energy_now: Option<f64> = batteries.iter().map(|b| b.energy_now).sum();
        let energy_full: Option<f64> = batteries.iter().map(|b| b.energy_full).sum();
        let percent = match (energy_now, energy_full) {
            (Some(now), Some(full)) if full > 0.0 => Some((100.0 * now / full) as f32),
            _ => {
                let known: Vec<f32> = batteries.iter().filter_map(|b| b.capacity).collect();
                (!known.is_empty()).then(|| known.iter().sum::<f32>() / known.len() as f32)
            }
        };
        let powers: Vec<f32> = batteries.iter().filter_map(|b| b.power_w).collect();
        let power_w = (!powers.is_empty()).then(|| powers.iter().sum());
        let status = ["Discharging", "Charging"]
            .into_iter()
            .find(|s| batteries.iter().any(|b| b.status == *s))
            .map(str::to_owned)
            .unwrap_or_else(|| batteries[0].status.clone());

        // Amounts and rates add up only in the same unit (energy or charge).
        let same_units = batteries
            .iter()
            .all(|b| b.charge_units == batteries[0].charge_units);
        let design: Option<f64> = batteries.iter().map(|b| b.design_full).sum();
        let health_percent = match (energy_full, design) {
            (Some(full), Some(design)) if same_units && design > 0.0 => {
                Some((100.0 * full / design) as f32)
            }
            _ => None,
        };
        let rate: Option<f64> = batteries.iter().map(|b| b.rate).sum();
        let left = match status.as_str() {
            "Discharging" => energy_now,
            "Charging" => energy_full
                .zip(energy_now)
                .map(|(full, now)| (full - now).max(0.0)),
            _ => None,
        };
        let time_left_secs = match (left, rate) {
            (Some(left), Some(rate)) if same_units => {
                let smoothed = match self.rate.take() {
                    // A new status (plugged in, unplugged) starts over.
                    Some((was, previous)) if was == status => {
                        previous + RATE_SMOOTHING * (rate - previous)
                    }
                    _ => rate,
                };
                self.rate = Some((status.clone(), smoothed));
                (smoothed > MIN_RATE).then(|| (left / smoothed * 3600.0).round() as u64)
            }
            _ => {
                self.rate = None;
                None
            }
        };
        Some(BatteryStats {
            percent,
            status,
            power_w,
            time_left_secs,
            health_percent,
        })
    }
}

/// One battery's attributes.
struct Reading {
    status: String,
    capacity: Option<f32>,
    /// Energy in µWh, or charge in µAh when `charge_units` (consistently
    /// per battery, design capacity and rate included).
    charge_units: bool,
    energy_now: Option<f64>,
    energy_full: Option<f64>,
    design_full: Option<f64>,
    /// Charge or discharge rate in µW (µA with `charge_units`), unsigned.
    rate: Option<f64>,
    power_w: Option<f32>,
}

fn read_one(dir: &Path) -> Option<Reading> {
    if read_trimmed(&dir.join("type"))? != "Battery"
        || read_trimmed(&dir.join("scope")).is_some_and(|s| s == "Device")
    {
        return None;
    }
    let value = |name: &str| read_i64(&dir.join(name));
    let float = |name: &str| value(name).map(|v| v.unsigned_abs() as f64);
    // Drivers report either energy (µWh) or charge (µAh); power either
    // directly (µW) or as current (µA) times voltage (µV).
    let volts = float("voltage_now").map(|uv| uv / 1e6);
    let power_uw = float("power_now").or_else(|| Some(float("current_now")? * volts?));
    let charge_units = !(value("energy_now").is_some() && value("energy_full").is_some());
    let (energy_now, energy_full, design_full, rate) = if charge_units {
        (
            float("charge_now"),
            float("charge_full"),
            float("charge_full_design"),
            float("current_now").or_else(|| Some(power_uw? / volts?)),
        )
    } else {
        (
            float("energy_now"),
            float("energy_full"),
            float("energy_full_design"),
            power_uw,
        )
    };
    Some(Reading {
        status: read_trimmed(&dir.join("status")).unwrap_or_default(),
        capacity: value("capacity").map(|c| c as f32),
        charge_units,
        energy_now,
        energy_full,
        design_full,
        rate,
        power_w: power_uw.map(|uw| (uw / 1e6) as f32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    fn roots(root: &Path) -> SysRoots {
        SysRoots {
            sys: root.join("sys"),
            proc: root.join("proc"),
        }
    }

    #[test]
    fn reads_the_system_battery_and_skips_peripherals() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ps = "sys/class/power_supply";
        write(root, &format!("{ps}/AC/type"), "Mains\n");
        write(root, &format!("{ps}/BAT0/type"), "Battery\n");
        write(root, &format!("{ps}/BAT0/status"), "Discharging\n");
        write(root, &format!("{ps}/BAT0/capacity"), "72\n");
        write(root, &format!("{ps}/BAT0/energy_now"), "31500000\n");
        write(root, &format!("{ps}/BAT0/energy_full"), "43750000\n");
        write(root, &format!("{ps}/BAT0/power_now"), "12340000\n");
        // A wireless mouse.
        write(root, &format!("{ps}/hidpp_battery_0/type"), "Battery\n");
        write(root, &format!("{ps}/hidpp_battery_0/scope"), "Device\n");
        write(root, &format!("{ps}/hidpp_battery_0/capacity"), "5\n");

        let battery = read_battery(&roots(root)).unwrap();
        assert_eq!(battery.status, "Discharging");
        assert!((battery.percent.unwrap() - 72.0).abs() < 0.1);
        assert!((battery.power_w.unwrap() - 12.34).abs() < 0.01);
        // 31.5 Wh at 12.34 W: 2h33m.
        assert_eq!(battery.time_left_secs, Some(9190));
        assert_eq!(battery.summary(), "72% -12W 2h33m");
        assert_eq!(battery.health_percent, None);
    }

    #[test]
    fn charge_and_current_based_drivers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ps = "sys/class/power_supply";
        write(root, &format!("{ps}/BAT1/type"), "Battery\n");
        write(root, &format!("{ps}/BAT1/status"), "Charging\n");
        write(root, &format!("{ps}/BAT1/charge_now"), "2000000\n");
        write(root, &format!("{ps}/BAT1/charge_full"), "4000000\n");
        write(root, &format!("{ps}/BAT1/current_now"), "1500000\n");
        write(root, &format!("{ps}/BAT1/voltage_now"), "12000000\n");
        let battery = read_battery(&roots(root)).unwrap();
        assert!((battery.percent.unwrap() - 50.0).abs() < 0.1);
        assert!((battery.power_w.unwrap() - 18.0).abs() < 0.01);
        // 2 Ah to go at 1.5 A: 1h20m.
        assert_eq!(battery.summary(), "50% +18W 1h20m");
    }

    #[test]
    fn time_left_follows_the_smoothed_rate_and_health_shows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bat = "sys/class/power_supply/BAT0";
        write(root, &format!("{bat}/type"), "Battery\n");
        write(root, &format!("{bat}/status"), "Discharging\n");
        write(root, &format!("{bat}/energy_now"), "20000000\n");
        write(root, &format!("{bat}/energy_full"), "43710000\n");
        write(root, &format!("{bat}/energy_full_design"), "57000000\n");
        write(root, &format!("{bat}/power_now"), "10000000\n");
        let mut meter = BatteryMeter::default();
        let first = meter.read(&roots(root)).unwrap();
        assert_eq!(first.time_left_secs, Some(7200));
        assert_eq!(first.health_note().as_deref(), Some("health 77%"));

        // A short burst of 30 W moves the estimate only a little.
        write(root, &format!("{bat}/power_now"), "30000000\n");
        let burst = meter.read(&roots(root)).unwrap();
        assert_eq!(burst.time_left_secs, Some(6000));

        // Plugged in: the rate starts over.
        write(root, &format!("{bat}/status"), "Charging\n");
        let charging = meter.read(&roots(root)).unwrap();
        // 23.71 Wh to go at 30 W.
        assert_eq!(charging.time_left_secs, Some(2845));
        assert!(charging.details().contains("until full"));

        // Full: no time left.
        write(root, &format!("{bat}/status"), "Full\n");
        assert_eq!(meter.read(&roots(root)).unwrap().time_left_secs, None);
    }

    #[test]
    fn no_battery_on_a_desktop() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "sys/class/power_supply/AC/type", "Mains\n");
        assert_eq!(read_battery(&roots(tmp.path())), None);
        // Full and idle: no power figure.
        let full = BatteryStats {
            percent: Some(100.0),
            status: "Full".to_owned(),
            power_w: Some(0.0),
            ..Default::default()
        };
        assert_eq!(full.summary(), "100%");
    }
}
