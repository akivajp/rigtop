//! Hardware monitoring chips from `/sys/class/hwmon`.
//!
//! Each `hwmonN` directory is one chip (k10temp, nct6686, nvme, spd5118,
//! amdgpu, ...). Channels are numbered files such as `temp1_input` (millidegree
//! Celsius), `fan1_input` (RPM) and `power1_input` (microwatts).

use std::fs;
use std::path::Path;

use super::{SysRoots, read_i64, read_trimmed, read_u64, sorted_entries};
use crate::snapshot::{Fan, SensorChip, Temperature};

/// Chip names whose temperatures describe the CPU.
const CPU_CHIPS: &[&str] = &["k10temp", "zenpower", "coretemp", "cpu_thermal"];

/// A hwmon chip together with the power channels that [`SensorChip`] does not carry.
#[derive(Debug, Clone, Default)]
pub struct HwmonChip {
    /// Temperatures and fans in their serializable form.
    pub chip: SensorChip,
    /// Power readings in watts (`power<N>_input`, or `power<N>_average`).
    pub power_w: Vec<(String, f32)>,
}

/// Reads all hwmon chips, in `hwmonN` order.
pub fn read_chips(roots: &SysRoots) -> Vec<HwmonChip> {
    let class_dir = roots.sys.join("class/hwmon");
    sorted_entries(&class_dir)
        .into_iter()
        .filter_map(|entry| read_chip(&class_dir.join(&entry), &entry))
        // Some hwmon devices (e.g. HID battery reporters) expose no channel
        // rigtop understands; they would only add empty rows.
        .filter(|c| {
            !c.chip.temperatures.is_empty() || !c.chip.fans.is_empty() || !c.power_w.is_empty()
        })
        .collect()
}

/// Returns true if the chip reports CPU temperatures.
pub fn is_cpu_chip(chip: &SensorChip) -> bool {
    CPU_CHIPS.contains(&chip.name.as_str())
}

/// Reads one `hwmonN` directory. Returns `None` if it has no `name` attribute.
fn read_chip(dir: &Path, entry: &str) -> Option<HwmonChip> {
    let name = read_trimmed(&dir.join("name"))?;
    // The `device` link points at the parent device (PCI function, platform
    // device, NVMe controller, ...). Its last component is stable across
    // reboots, unlike the hwmonN number.
    let device = fs::read_link(dir.join("device"))
        .ok()
        .and_then(|target| target.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| entry.to_owned());
    let key = format!("{name}:{device}");

    let files = sorted_entries(dir);
    let temperatures = channel_numbers(&files, "temp")
        .into_iter()
        .filter_map(|n| read_temperature(dir, n))
        .collect();
    let fans = channel_numbers(&files, "fan")
        .into_iter()
        .filter_map(|n| {
            let rpm = read_u64(&dir.join(format!("fan{n}_input")))?;
            let label = read_trimmed(&dir.join(format!("fan{n}_label")))
                .unwrap_or_else(|| format!("fan{n}"));
            Some(Fan {
                key: format!("{key}/fan{n}"),
                label,
                rpm: rpm.min(u32::MAX as u64) as u32,
            })
        })
        .collect();
    let power_w = channel_numbers(&files, "power")
        .into_iter()
        .filter_map(|n| {
            // amdgpu exposes `power1_input` on newer kernels and
            // `power1_average` on older ones.
            let micro = read_u64(&dir.join(format!("power{n}_input")))
                .or_else(|| read_u64(&dir.join(format!("power{n}_average"))))?;
            let label = read_trimmed(&dir.join(format!("power{n}_label")))
                .unwrap_or_else(|| format!("power{n}"));
            Some((label, micro as f32 / 1_000_000.0))
        })
        .collect();

    Some(HwmonChip {
        chip: SensorChip {
            key,
            name,
            temperatures,
            fans,
        },
        power_w,
    })
}

/// Reads `temp<n>_*` attributes. Returns `None` if the input is unreadable.
fn read_temperature(dir: &Path, n: u32) -> Option<Temperature> {
    let milli =
        |suffix: &str| read_i64(&dir.join(format!("temp{n}_{suffix}"))).map(|v| v as f32 / 1000.0);
    let celsius = milli("input")?;
    let label =
        read_trimmed(&dir.join(format!("temp{n}_label"))).unwrap_or_else(|| format!("temp{n}"));
    Some(Temperature {
        label,
        celsius,
        max_celsius: milli("max"),
        critical_celsius: milli("crit"),
    })
}

/// Collects the channel numbers `N` of files named `<prefix>N_input` (or
/// `<prefix>N_average` for power), sorted and de-duplicated.
fn channel_numbers(files: &[String], prefix: &str) -> Vec<u32> {
    let mut numbers: Vec<u32> = files
        .iter()
        .filter_map(|f| {
            let rest = f.strip_prefix(prefix)?;
            let (num, suffix) = rest.split_once('_')?;
            if suffix == "input" || (prefix == "power" && suffix == "average") {
                num.parse().ok()
            } else {
                None
            }
        })
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::{symlink, write};
    use std::path::PathBuf;

    fn roots(dir: &Path) -> SysRoots {
        SysRoots {
            sys: dir.join("sys"),
            proc: dir.join("proc"),
        }
    }

    #[test]
    fn reads_temperatures_fans_and_power() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // k10temp: labelled CCD temperatures, no fans.
        write(root, "sys/class/hwmon/hwmon5/name", "k10temp\n");
        write(root, "sys/class/hwmon/hwmon5/temp1_input", "45250\n");
        write(root, "sys/class/hwmon/hwmon5/temp1_label", "Tctl\n");
        write(root, "sys/class/hwmon/hwmon5/temp3_input", "40000\n");
        write(root, "sys/class/hwmon/hwmon5/temp3_label", "Tccd1\n");
        symlink(
            root,
            "sys/class/hwmon/hwmon5/device",
            &PathBuf::from("../../../0000:00:18.3"),
        );
        // nct6686: unlabelled fans, one stopped.
        write(root, "sys/class/hwmon/hwmon10/name", "nct6686\n");
        write(root, "sys/class/hwmon/hwmon10/fan1_input", "886\n");
        write(root, "sys/class/hwmon/hwmon10/fan2_input", "0\n");
        symlink(
            root,
            "sys/class/hwmon/hwmon10/device",
            &PathBuf::from("../../../nct6683.2592"),
        );
        // amdgpu: power in microwatts.
        write(root, "sys/class/hwmon/hwmon2/name", "amdgpu\n");
        write(root, "sys/class/hwmon/hwmon2/power1_input", "12500000\n");
        write(root, "sys/class/hwmon/hwmon2/power1_label", "PPT\n");

        // A chip without any supported channel must be skipped.
        write(root, "sys/class/hwmon/hwmon3/name", "hid_battery\n");

        let chips = read_chips(&roots(root));
        // Natural order: hwmon2, hwmon5, hwmon10.
        let keys: Vec<&str> = chips.iter().map(|c| c.chip.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "amdgpu:hwmon2",
                "k10temp:0000:00:18.3",
                "nct6686:nct6683.2592"
            ]
        );

        let amdgpu = &chips[0];
        assert_eq!(amdgpu.power_w, vec![("PPT".to_owned(), 12.5)]);

        let k10 = &chips[1].chip;
        assert!(is_cpu_chip(k10));
        assert_eq!(k10.temperatures.len(), 2);
        assert_eq!(k10.temperatures[0].label, "Tctl");
        assert!((k10.temperatures[0].celsius - 45.25).abs() < 1e-6);
        assert_eq!(k10.temperatures[1].label, "Tccd1");
        assert_eq!(k10.temperatures[1].max_celsius, None);

        let nct = &chips[2].chip;
        assert_eq!(nct.fans.len(), 2);
        assert_eq!(nct.fans[0].key, "nct6686:nct6683.2592/fan1");
        assert_eq!(nct.fans[0].label, "fan1");
        assert_eq!(nct.fans[0].rpm, 886);
        assert_eq!(nct.fans[1].rpm, 0);
    }

    #[test]
    fn missing_class_directory_yields_no_chips() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_chips(&roots(tmp.path())).is_empty());
    }
}
