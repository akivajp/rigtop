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
    /// amdgpu of an APU (integrated graphics): only APUs have the `vddnb`
    /// voltage rail. Discrete GPUs report their board power instead.
    pub is_apu: bool,
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

/// Physical location of a logical core: `(physical_package_id, core_id)`,
/// as coretemp's `Package id P` and `Core N` labels name it.
pub type CoreLocation = (u32, u32);

/// Reads the package and core id of logical cores `cpu0..cpu<count>`.
/// `None` for cores whose topology is not readable (offline cores).
pub fn read_core_locations(roots: &SysRoots, count: usize) -> Vec<Option<CoreLocation>> {
    let cpu_dir = roots.sys.join("devices/system/cpu");
    (0..count)
        .map(|i| {
            let topology = cpu_dir.join(format!("cpu{i}/topology"));
            let package = read_u64(&topology.join("physical_package_id"))?;
            let core = read_u64(&topology.join("core_id"))?;
            Some((package as u32, core as u32))
        })
        .collect()
}

/// Splits CPU temperatures into those to list and the temperature of each
/// logical core, from coretemp's `Core N` readings (Intel). `Core N` names
/// the core id; `Package id P` in the same chip names its package.
///
/// Readings matched to at least one logical core leave the list, so that
/// many-core CPUs do not get a long row of `Core N` values. When nothing
/// matches, the list is returned unchanged with no per-core values.
pub fn split_core_temperatures(
    chips: &[HwmonChip],
    cores: &[Option<CoreLocation>],
) -> (Vec<Temperature>, Vec<Option<f32>>) {
    let cpu_chips: Vec<&HwmonChip> = chips.iter().filter(|c| is_cpu_chip(&c.chip)).collect();
    let coretemp_chips = cpu_chips
        .iter()
        .filter(|c| c.chip.name == "coretemp")
        .count();
    // Per-core readings: (package, if it must match; core id; reading).
    let mut readings: Vec<(Option<u32>, u32, &Temperature)> = Vec::new();
    for chip in cpu_chips.iter().filter(|c| c.chip.name == "coretemp") {
        let package = chip
            .chip
            .temperatures
            .iter()
            .find_map(|t| t.label.strip_prefix("Package id ")?.parse().ok());
        for temp in &chip.chip.temperatures {
            if let Some(core) = temp
                .label
                .strip_prefix("Core ")
                .and_then(|n| n.parse().ok())
            {
                // With a single chip the package does not need to match
                // (some kernels number packages differently).
                readings.push((package.filter(|_| coretemp_chips > 1), core, temp));
            }
        }
    }
    let mut used = std::collections::HashSet::new();
    let per_core: Vec<Option<f32>> = cores
        .iter()
        .map(|location| {
            let (package, core) = (*location)?;
            let (_, _, temp) = readings
                .iter()
                .find(|(p, c, _)| *c == core && p.is_none_or(|p| p == package))?;
            used.insert(temp.key.as_str());
            Some(temp.celsius)
        })
        .collect();
    let all = cpu_chips
        .iter()
        .flat_map(|c| c.chip.temperatures.iter().cloned());
    if used.is_empty() {
        return (all.collect(), Vec::new());
    }
    let listed = all.filter(|t| !used.contains(t.key.as_str())).collect();
    (listed, per_core)
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
        .filter_map(|n| read_temperature(dir, &key, n))
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

    let is_apu = name == "amdgpu"
        && read_trimmed(&dir.join("in1_label")).is_some_and(|label| label == "vddnb");
    Some(HwmonChip {
        chip: SensorChip {
            key,
            name,
            temperatures,
            fans,
        },
        power_w,
        is_apu,
    })
}

/// Reads `temp<n>_*` attributes. Returns `None` if the input is unreadable.
fn read_temperature(dir: &Path, chip_key: &str, n: u32) -> Option<Temperature> {
    let milli =
        |suffix: &str| read_i64(&dir.join(format!("temp{n}_{suffix}"))).map(|v| v as f32 / 1000.0);
    let celsius = milli("input")?;
    let label =
        read_trimmed(&dir.join(format!("temp{n}_label"))).unwrap_or_else(|| format!("temp{n}"));
    Some(Temperature {
        key: format!("{chip_key}/temp{n}"),
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
        write(root, "sys/class/hwmon/hwmon2/in1_label", "vddnb\n");
        // A discrete amdgpu has no vddnb rail.
        write(root, "sys/class/hwmon/hwmon7/name", "amdgpu\n");
        write(root, "sys/class/hwmon/hwmon7/power1_input", "80000000\n");
        write(root, "sys/class/hwmon/hwmon7/in0_label", "vddgfx\n");

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
                "amdgpu:hwmon7",
                "nct6686:nct6683.2592"
            ]
        );

        let amdgpu = &chips[0];
        assert_eq!(amdgpu.power_w, vec![("PPT".to_owned(), 12.5)]);
        assert!(amdgpu.is_apu);
        assert!(!chips[2].is_apu, "discrete GPU");

        let k10 = &chips[1].chip;
        assert!(is_cpu_chip(k10));
        assert_eq!(k10.temperatures.len(), 2);
        assert_eq!(k10.temperatures[0].label, "Tctl");
        assert_eq!(k10.temperatures[0].key, "k10temp:0000:00:18.3/temp1");
        assert!((k10.temperatures[0].celsius - 45.25).abs() < 1e-6);
        assert_eq!(k10.temperatures[1].label, "Tccd1");
        assert_eq!(k10.temperatures[1].max_celsius, None);

        let nct = &chips[3].chip;
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

    /// A coretemp chip with the given package and `(core id, celsius)` readings.
    fn coretemp(device: &str, package: Option<u32>, cores: &[(u32, f32)]) -> HwmonChip {
        let key = format!("coretemp:{device}");
        let temp = |n: usize, label: String, celsius: f32| Temperature {
            key: format!("{key}/temp{n}"),
            label,
            celsius,
            ..Default::default()
        };
        let mut temperatures: Vec<Temperature> = package
            .map(|p| temp(1, format!("Package id {p}"), 60.0))
            .into_iter()
            .collect();
        for (i, (core, celsius)) in cores.iter().enumerate() {
            temperatures.push(temp(i + 2, format!("Core {core}"), *celsius));
        }
        HwmonChip {
            chip: SensorChip {
                key: key.clone(),
                name: "coretemp".to_owned(),
                temperatures,
                fans: Vec::new(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn reads_core_locations() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (cpu, package, core) in [(0, 0, 0), (1, 0, 4)] {
            let dir = format!("sys/devices/system/cpu/cpu{cpu}/topology");
            write(
                root,
                &format!("{dir}/physical_package_id"),
                &format!("{package}\n"),
            );
            write(root, &format!("{dir}/core_id"), &format!("{core}\n"));
        }
        // cpu2 is offline: no topology.
        let locations = read_core_locations(&roots(root), 3);
        assert_eq!(locations, vec![Some((0, 0)), Some((0, 4)), None]);
    }

    #[test]
    fn matches_core_temperatures_to_logical_cores() {
        // Hyper-threading: logical cores 0/2 and 1/3 share physical cores 0
        // and 4 (hybrid CPUs number core ids with gaps).
        let chips = vec![coretemp("coretemp.0", Some(0), &[(0, 50.0), (4, 55.0)])];
        let cores = [Some((0, 0)), Some((0, 4)), Some((0, 0)), Some((0, 4)), None];
        let (listed, per_core) = split_core_temperatures(&chips, &cores);
        assert_eq!(
            per_core,
            vec![Some(50.0), Some(55.0), Some(50.0), Some(55.0), None]
        );
        // Only the package reading stays in the list.
        let labels: Vec<&str> = listed.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, vec!["Package id 0"]);
    }

    #[test]
    fn matches_core_temperatures_by_package_on_multi_socket_hosts() {
        let chips = vec![
            coretemp("coretemp.0", Some(0), &[(0, 40.0)]),
            coretemp("coretemp.1", Some(1), &[(0, 70.0)]),
        ];
        let (listed, per_core) = split_core_temperatures(&chips, &[Some((1, 0)), Some((0, 0))]);
        assert_eq!(per_core, vec![Some(70.0), Some(40.0)]);
        assert_eq!(listed.len(), 2);
    }

    #[test]
    fn keeps_temperatures_without_per_core_readings() {
        // AMD: no per-core sensors, so the list is unchanged.
        let mut k10 = coretemp("0000:00:18.3", None, &[]);
        k10.chip.name = "k10temp".to_owned();
        k10.chip.temperatures.push(Temperature {
            label: "Tctl".to_owned(),
            ..Default::default()
        });
        let (listed, per_core) = split_core_temperatures(&[k10], &[Some((0, 0))]);
        assert_eq!(listed.len(), 1);
        assert!(per_core.is_empty());
    }
}
