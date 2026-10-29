//! AMD and Intel GPUs from `/sys/class/drm/card<N>/device`.
//!
//! NVIDIA GPUs come from NVML (`gpu.rs`) and are skipped here. Everything
//! read is world-readable:
//!
//! - amdgpu: `gpu_busy_percent`, `mem_busy_percent`,
//!   `mem_info_vram_{used,total}`, and the device's hwmon (temperature,
//!   power, fan, clock).
//! - i915 / xe (Intel): the clock and, on discrete cards, hwmon. Intel
//!   exposes utilization only through perf events, which need privileges,
//!   so it is left unknown.
//!
//! Names come from the driver (`product_name` on some AMD boards) or the PCI
//! ID database (`pci.ids`, from hwdata / pciutils).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::{SysRoots, read_trimmed, read_u64, sorted_entries};
use crate::snapshot::GpuStats;

const VENDOR_AMD: u16 = 0x1002;
const VENDOR_INTEL: u16 = 0x8086;

/// Where distributions put the PCI ID database.
const PCI_IDS: &[&str] = &[
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/usr/share/pci.ids",
];

/// Reads AMD and Intel GPUs, remembering their names.
#[derive(Default)]
pub struct DrmGpuReader {
    /// Names by (vendor, device), looked up once.
    names: HashMap<(u16, u16), String>,
}

impl DrmGpuReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// AMD and Intel GPUs, numbered from `first_index` (after NVIDIA's).
    pub fn read(&mut self, roots: &SysRoots, first_index: u32) -> Vec<GpuStats> {
        let class_dir = roots.sys.join("class/drm");
        let mut gpus = Vec::new();
        for entry in sorted_entries(&class_dir) {
            // card0, card1, ...; not connectors such as card1-DP-1.
            let is_card = entry
                .strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if !is_card {
                continue;
            }
            let card = class_dir.join(&entry);
            let device = card.join("device");
            let Some(vendor) = read_hex(&device.join("vendor")) else {
                continue;
            };
            let driver = fs::read_link(device.join("driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let index = first_index + gpus.len() as u32;
            let gpu = match (vendor, driver.as_str()) {
                (VENDOR_AMD, "amdgpu") => Some(self.read_amd(&device, index)),
                (VENDOR_INTEL, "i915" | "xe") => Some(self.read_intel(&card, &device, index)),
                _ => None,
            };
            gpus.extend(gpu);
        }
        gpus
    }

    fn read_amd(&mut self, device: &Path, index: u32) -> GpuStats {
        let hwmon = Hwmon::find(device);
        let percent = |name: &str| read_u64(&device.join(name)).map(|v| v as f32);
        let integrated = hwmon.is_apu;
        GpuStats {
            index,
            vendor: "AMD".to_owned(),
            name: self.name(device, VENDOR_AMD, "AMD"),
            integrated,
            utilization_percent: percent("gpu_busy_percent"),
            memory_utilization_percent: percent("mem_busy_percent"),
            memory_used_bytes: read_u64(&device.join("mem_info_vram_used")),
            memory_total_bytes: read_u64(&device.join("mem_info_vram_total")),
            temperature_c: hwmon.temperature_c,
            // An APU's reading is the whole SoC (see PowerStats::igpu_w),
            // not the GPU's own share.
            power_w: hwmon.power_w.filter(|_| !integrated),
            power_limit_w: hwmon.power_cap_w.filter(|_| !integrated),
            fan_percent: hwmon.fan_percent,
            clock_mhz: hwmon.clock_mhz.or_else(|| current_sclk(device)),
            ..Default::default()
        }
    }

    fn read_intel(&mut self, card: &Path, device: &Path, index: u32) -> GpuStats {
        let hwmon = Hwmon::find(device);
        // i915 has gt_act_freq_mhz on the card; xe has it per tile and GT.
        let clock_mhz = read_u64(&card.join("gt_act_freq_mhz"))
            .or_else(|| read_u64(&device.join("tile0/gt0/freq0/act_freq")))
            .map(|mhz| mhz as u32);
        // The integrated GPU sits at PCI 00:02.0 on Intel platforms.
        let integrated = fs::canonicalize(device)
            .ok()
            .and_then(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(":00:02.0"))
            })
            .unwrap_or(false);
        GpuStats {
            index,
            vendor: "Intel".to_owned(),
            name: self.name(device, VENDOR_INTEL, "Intel"),
            integrated,
            temperature_c: hwmon.temperature_c,
            power_w: hwmon.power_w,
            power_limit_w: hwmon.power_cap_w,
            fan_percent: hwmon.fan_percent,
            clock_mhz,
            ..Default::default()
        }
    }

    /// Driver-provided name, else the PCI ID database, else the device id;
    /// prefixed with the vendor.
    fn name(&mut self, device: &Path, vendor: u16, vendor_name: &str) -> String {
        if let Some(name) = read_trimmed(&device.join("product_name")).filter(|n| !n.is_empty()) {
            return with_vendor(&name, vendor_name);
        }
        let id = read_hex(&device.join("device")).unwrap_or(0);
        self.names
            .entry((vendor, id))
            .or_insert_with(|| {
                let name = PCI_IDS
                    .iter()
                    .find_map(|path| fs::read_to_string(path).ok())
                    .and_then(|text| pci_device_name(&text, vendor, id))
                    .unwrap_or_else(|| format!("GPU {id:04x}"));
                with_vendor(&name, vendor_name)
            })
            .clone()
    }
}

fn with_vendor(name: &str, vendor: &str) -> String {
    if name.starts_with(vendor) {
        name.to_owned()
    } else {
        format!("{vendor} {name}")
    }
}

/// `0x1002` style sysfs attributes.
fn read_hex(path: &Path) -> Option<u16> {
    let text = read_trimmed(path)?;
    u16::from_str_radix(text.trim_start_matches("0x"), 16).ok()
}

/// The `*`-marked line of `pp_dpm_sclk` ("1: 1800Mhz *").
fn current_sclk(device: &Path) -> Option<u32> {
    let text = fs::read_to_string(device.join("pp_dpm_sclk")).ok()?;
    let line = text.lines().find(|l| l.trim_end().ends_with('*'))?;
    let value = line.split_whitespace().nth(1)?;
    value
        .trim_end_matches("Mhz")
        .trim_end_matches("MHz")
        .parse()
        .ok()
}

/// Device name from `pci.ids`: a vendor line `vvvv  Name`, then device
/// lines `\tdddd  Name` until the next vendor.
pub(crate) fn pci_device_name(text: &str, vendor: u16, device: u16) -> Option<String> {
    let vendor_prefix = format!("{vendor:04x}  ");
    let device_prefix = format!("\t{device:04x}  ");
    let mut in_vendor = false;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return None;
            }
            in_vendor = line.starts_with(&vendor_prefix);
        } else if in_vendor && let Some(name) = line.strip_prefix(&device_prefix) {
            return Some(name.trim().to_owned());
        }
    }
    None
}

/// The readings of a GPU's own hwmon directory.
#[derive(Default)]
struct Hwmon {
    temperature_c: Option<f32>,
    power_w: Option<f32>,
    power_cap_w: Option<f32>,
    fan_percent: Option<f32>,
    clock_mhz: Option<u32>,
    /// amdgpu's `vddnb` rail exists only on APUs.
    is_apu: bool,
}

impl Hwmon {
    fn find(device: &Path) -> Self {
        let dir = device.join("hwmon");
        let Some(entry) = sorted_entries(&dir).into_iter().next() else {
            return Self::default();
        };
        let dir = dir.join(entry);
        let read = |name: &str| read_u64(&dir.join(name));
        Self {
            temperature_c: read("temp1_input").map(|m| m as f32 / 1000.0),
            power_w: read("power1_average")
                .or_else(|| read("power1_input"))
                .map(|uw| uw as f32 / 1e6),
            power_cap_w: read("power1_cap").map(|uw| uw as f32 / 1e6),
            // pwm1 is 0-255.
            fan_percent: read("pwm1").map(|pwm| pwm as f32 * 100.0 / 255.0),
            clock_mhz: read("freq1_input").map(|hz| (hz / 1_000_000) as u32),
            is_apu: read_trimmed(&dir.join("in1_label")).is_some_and(|l| l == "vddnb"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::{symlink, write};

    #[test]
    fn finds_device_names_in_pci_ids() {
        let ids = "# comment\n\
                   1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
                   \t15bf  Phoenix1\n\
                   \t\t17aa 50d8  ThinkPad\n\
                   \t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX]\n\
                   1003  ULSI Systems\n\
                   \t0201  US201\n";
        assert_eq!(
            pci_device_name(ids, 0x1002, 0x15bf).as_deref(),
            Some("Phoenix1")
        );
        assert_eq!(
            pci_device_name(ids, 0x1002, 0x744c).as_deref(),
            Some("Navi 31 [Radeon RX 7900 XT/7900 XTX]")
        );
        // A device id of another vendor does not match.
        assert_eq!(pci_device_name(ids, 0x1002, 0x0201), None);
    }

    fn roots(root: &Path) -> SysRoots {
        SysRoots {
            sys: root.join("sys"),
            proc: root.join("proc"),
        }
    }

    /// A card whose `device` points at `pci` with `driver`.
    fn card(
        root: &Path,
        card: &str,
        pci: &str,
        vendor: &str,
        device: &str,
        driver: &str,
    ) -> String {
        let dev = format!("sys/devices/pci0000:00/{pci}");
        write(root, &format!("{dev}/vendor"), &format!("{vendor}\n"));
        write(root, &format!("{dev}/device"), &format!("{device}\n"));
        write(root, &format!("sys/bus/pci/drivers/{driver}/.keep"), "");
        symlink(
            root,
            &format!("{dev}/driver"),
            &root.join(format!("sys/bus/pci/drivers/{driver}")),
        );
        symlink(
            root,
            &format!("sys/class/drm/{card}/device"),
            &root.join(&dev),
        );
        dev
    }

    #[test]
    fn reads_amd_apu_and_discrete_skipping_nvidia() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // An NVIDIA card (left to NVML) and a connector entry.
        card(root, "card0", "0000:01:00.0", "0x10de", "0x2684", "nvidia");
        write(root, "sys/class/drm/card1-DP-1/status", "connected\n");
        // A laptop APU.
        let apu = card(root, "card1", "0000:c3:00.0", "0x1002", "0x15bf", "amdgpu");
        write(root, &format!("{apu}/product_name"), "Radeon 780M\n");
        write(root, &format!("{apu}/gpu_busy_percent"), "12\n");
        write(root, &format!("{apu}/mem_info_vram_used"), "1073741824\n");
        write(root, &format!("{apu}/mem_info_vram_total"), "2147483648\n");
        write(root, &format!("{apu}/hwmon/hwmon6/temp1_input"), "45000\n");
        write(
            root,
            &format!("{apu}/hwmon/hwmon6/power1_input"),
            "6000000\n",
        );
        write(
            root,
            &format!("{apu}/hwmon/hwmon6/freq1_input"),
            "800000000\n",
        );
        write(root, &format!("{apu}/hwmon/hwmon6/in1_label"), "vddnb\n");
        // A discrete card with a fan and a power cap.
        let dgpu = card(root, "card2", "0000:03:00.0", "0x1002", "0x744c", "amdgpu");
        write(root, &format!("{dgpu}/gpu_busy_percent"), "99\n");
        write(
            root,
            &format!("{dgpu}/pp_dpm_sclk"),
            "0: 500Mhz\n1: 2500Mhz *\n",
        );
        write(
            root,
            &format!("{dgpu}/hwmon/hwmon9/power1_average"),
            "250000000\n",
        );
        write(
            root,
            &format!("{dgpu}/hwmon/hwmon9/power1_cap"),
            "300000000\n",
        );
        write(root, &format!("{dgpu}/hwmon/hwmon9/pwm1"), "51\n");

        let gpus = DrmGpuReader::new().read(&roots(root), 1);
        assert_eq!(gpus.len(), 2, "NVIDIA and connectors are skipped");

        let apu = &gpus[0];
        assert_eq!(apu.index, 1, "numbered after NVIDIA's");
        assert_eq!(apu.name, "AMD Radeon 780M");
        assert!(apu.integrated);
        assert_eq!(apu.utilization_percent, Some(12.0));
        assert_eq!(apu.memory_total_bytes, Some(2 << 30));
        assert_eq!(apu.temperature_c, Some(45.0));
        assert_eq!(apu.power_w, None, "an APU's reading is the whole SoC");
        assert_eq!(apu.clock_mhz, Some(800));

        let dgpu = &gpus[1];
        assert!(!dgpu.integrated);
        assert!(dgpu.name.starts_with("AMD "), "{}", dgpu.name);
        assert_eq!(dgpu.power_w, Some(250.0));
        assert_eq!(dgpu.power_limit_w, Some(300.0));
        assert_eq!(dgpu.fan_percent, Some(20.0));
        assert_eq!(
            dgpu.clock_mhz,
            Some(2500),
            "from pp_dpm_sclk without hwmon freq"
        );
    }

    #[test]
    fn reads_intel_clock_without_utilization() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        card(root, "card0", "0000:00:02.0", "0x8086", "0x46a6", "i915");
        write(root, "sys/class/drm/card0/gt_act_freq_mhz", "1300\n");
        let gpus = DrmGpuReader::new().read(&roots(root), 0);
        assert_eq!(gpus.len(), 1);
        let igpu = &gpus[0];
        assert_eq!(igpu.vendor, "Intel");
        assert!(igpu.name.starts_with("Intel "), "{}", igpu.name);
        assert!(igpu.integrated, "at 00:02.0");
        assert_eq!(igpu.clock_mhz, Some(1300));
        assert_eq!(igpu.utilization_percent, None);
    }
}
