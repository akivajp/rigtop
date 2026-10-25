//! Laptop batteries from `/sys/class/power_supply`.
//!
//! Each supply has a `type`; system batteries are `Battery`. Batteries of
//! peripherals (wireless mice, keyboards) are also `Battery` but have
//! `scope` set to `Device`, and are left out.

use std::path::Path;

use super::{SysRoots, read_i64, read_trimmed, sorted_entries};
use crate::snapshot::BatteryStats;

/// Combined state of the system batteries, or `None` without any.
pub fn read_battery(roots: &SysRoots) -> Option<BatteryStats> {
    let class_dir = roots.sys.join("class/power_supply");
    let batteries: Vec<Reading> = sorted_entries(&class_dir)
        .iter()
        .filter_map(|entry| read_one(&class_dir.join(entry)))
        .collect();
    if batteries.is_empty() {
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
    Some(BatteryStats {
        percent,
        status,
        power_w,
    })
}

/// One battery's attributes.
struct Reading {
    status: String,
    capacity: Option<f32>,
    /// Energy in µWh (or charge in µAh, consistently per battery).
    energy_now: Option<f64>,
    energy_full: Option<f64>,
    power_w: Option<f32>,
}

fn read_one(dir: &Path) -> Option<Reading> {
    if read_trimmed(&dir.join("type"))? != "Battery"
        || read_trimmed(&dir.join("scope")).is_some_and(|s| s == "Device")
    {
        return None;
    }
    let value = |name: &str| read_i64(&dir.join(name));
    // Drivers report either energy (µWh) or charge (µAh); power either
    // directly (µW) or as current (µA) times voltage (µV).
    let (energy_now, energy_full) = match (value("energy_now"), value("energy_full")) {
        (Some(now), Some(full)) => (Some(now as f64), Some(full as f64)),
        _ => (
            value("charge_now").map(|v| v as f64),
            value("charge_full").map(|v| v as f64),
        ),
    };
    let power_w = value("power_now")
        .map(|uw| uw.unsigned_abs() as f32 / 1e6)
        .or_else(|| {
            let amps = value("current_now")?.unsigned_abs() as f64 / 1e6;
            let volts = value("voltage_now")?.unsigned_abs() as f64 / 1e6;
            Some((amps * volts) as f32)
        });
    Some(Reading {
        status: read_trimmed(&dir.join("status")).unwrap_or_default(),
        capacity: value("capacity").map(|c| c as f32),
        energy_now,
        energy_full,
        power_w,
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
        assert_eq!(battery.summary(), "72% -12W");
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
        assert_eq!(battery.summary(), "50% +18W");
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
        };
        assert_eq!(full.summary(), "100%");
    }
}
