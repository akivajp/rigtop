//! NVIDIA GPU statistics via NVML.
//!
//! `nvml-wrapper` loads `libnvidia-ml` at runtime, so machines without an
//! NVIDIA driver simply report no GPUs instead of failing to start.

use std::collections::HashMap;

use nvml_wrapper::Nvml;
use nvml_wrapper::bitmasks::device::ThrottleReasons;
use nvml_wrapper::enum_wrappers::device::{Clock, EccCounter, MemoryError, TemperatureSensor};
use nvml_wrapper::enums::device::UsedGpuMemory;
use nvml_wrapper::{Device, struct_wrappers::device::ProcessInfo as NvmlProcessInfo};

use crate::snapshot::{EccErrors, GpuProcess, GpuStats, PcieLink};

/// Collects per-GPU and per-process statistics from NVML.
pub struct NvmlCollector {
    /// `None` when the library or driver is unavailable.
    nvml: Option<Nvml>,
    /// Why NVML could not be initialized, for diagnostics.
    init_error: Option<String>,
    /// Newest per-process utilization sample timestamp seen, per GPU index.
    /// Passing it back to NVML returns only samples newer than the last poll.
    last_sample_ts: HashMap<u32, u64>,
    /// Whether to query per-process utilization, the costliest NVML call
    /// (about 2 ms per GPU). Only the full process table shows it.
    process_utilization: bool,
    /// Whether ECC is on, per GPU index (changing it takes a reboot).
    ecc_enabled: HashMap<u32, bool>,
}

impl NvmlCollector {
    /// Initializes NVML. Never fails; check [`NvmlCollector::init_error`] for the reason
    /// when no GPUs are reported.
    pub fn new() -> Self {
        let (nvml, init_error) = Self::init();
        Self {
            nvml,
            init_error,
            last_sample_ts: HashMap::new(),
            process_utilization: true,
            ecc_enabled: HashMap::new(),
        }
    }

    /// NVIDIA ships no NVML for macOS (nvml-wrapper would look for `nvml.dll`
    /// there), so it is not tried, and having no GPUs is not worth a note.
    #[cfg(target_os = "macos")]
    fn init() -> (Option<Nvml>, Option<String>) {
        (None, None)
    }

    #[cfg(not(target_os = "macos"))]
    fn init() -> (Option<Nvml>, Option<String>) {
        match Nvml::init() {
            Ok(nvml) => (Some(nvml), None),
            Err(err) => (None, Some(err.to_string())),
        }
    }

    /// Turns the per-process utilization query on or off (off while only
    /// the top processes are wanted, as for an overview).
    pub fn set_process_utilization(&mut self, enabled: bool) {
        self.process_utilization = enabled;
    }

    /// The NVML initialization error, if any.
    pub fn init_error(&self) -> Option<&str> {
        self.init_error.as_deref()
    }

    /// Queries every NVIDIA GPU. Individual metrics a device does not support
    /// are left as `None`.
    pub fn collect(&mut self) -> Vec<GpuStats> {
        let Some(nvml) = self.nvml.as_ref() else {
            return Vec::new();
        };
        let count = nvml.device_count().unwrap_or(0);
        let mut gpus = Vec::with_capacity(count as usize);
        for index in 0..count {
            let Ok(device) = nvml.device_by_index(index) else {
                continue;
            };
            let last_ts = self.last_sample_ts.get(&index).copied();
            let ecc = *self.ecc_enabled.entry(index).or_insert_with(|| {
                device
                    .is_ecc_enabled()
                    .is_ok_and(|mode| mode.currently_enabled)
            });
            let (mut stats, newest_ts) =
                collect_device(index, &device, last_ts, self.process_utilization);
            if ecc {
                stats.ecc = ecc_errors(&device);
            }
            if let Some(ts) = newest_ts {
                self.last_sample_ts.insert(index, ts);
            }
            gpus.push(stats);
        }
        gpus
    }
}

impl Default for NvmlCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads one device. Also returns the newest process sample timestamp.
fn collect_device(
    index: u32,
    device: &Device<'_>,
    last_ts: Option<u64>,
    process_utilization: bool,
) -> (GpuStats, Option<u64>) {
    let utilization = device.utilization_rates().ok();
    let memory = device.memory_info().ok();
    let (processes, newest_ts) = collect_processes(device, last_ts, process_utilization);
    let stats = GpuStats {
        index,
        vendor: "NVIDIA".to_owned(),
        name: device.name().unwrap_or_else(|_| format!("GPU {index}")),
        // NVML covers discrete NVIDIA GPUs only.
        integrated: false,
        utilization_percent: utilization.as_ref().map(|u| u.gpu as f32),
        memory_utilization_percent: utilization.as_ref().map(|u| u.memory as f32),
        memory_used_bytes: memory.as_ref().map(|m| m.used),
        memory_total_bytes: memory.as_ref().map(|m| m.total),
        temperature_c: device
            .temperature(TemperatureSensor::Gpu)
            .ok()
            .map(|t| t as f32),
        // NVML reports milliwatts.
        power_w: device.power_usage().ok().map(|mw| mw as f32 / 1000.0),
        power_limit_w: device
            .enforced_power_limit()
            .ok()
            .map(|mw| mw as f32 / 1000.0),
        fan_percent: device.fan_speed(0).ok().map(|p| p as f32),
        clock_mhz: device.clock_info(Clock::Graphics).ok(),
        throttle_reasons: device
            .current_throttle_reasons()
            .map(throttle_reasons)
            .unwrap_or_default(),
        ecc: None,
        pcie: pcie_link(device),
        processes,
    };
    (stats, newest_ts)
}

/// The reasons worth showing: idling, application or display clock
/// settings and sync boost are not held-down clocks.
fn throttle_reasons(reasons: ThrottleReasons) -> Vec<String> {
    let mut names = Vec::new();
    if reasons.contains(ThrottleReasons::SW_POWER_CAP) {
        names.push("power cap");
    }
    if reasons.contains(ThrottleReasons::SW_THERMAL_SLOWDOWN) {
        names.push("thermal");
    }
    if reasons.contains(ThrottleReasons::HW_THERMAL_SLOWDOWN) {
        names.push("hw thermal");
    }
    if reasons.contains(ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN) {
        names.push("power brake");
    }
    // Set together with the two above; alone, its cause is not known.
    if reasons.contains(ThrottleReasons::HW_SLOWDOWN)
        && !reasons.intersects(
            ThrottleReasons::HW_THERMAL_SLOWDOWN | ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN,
        )
    {
        names.push("hw slowdown");
    }
    names.into_iter().map(str::to_owned).collect()
}

/// ECC errors since the driver loaded.
fn ecc_errors(device: &Device<'_>) -> Option<EccErrors> {
    let count = |kind| device.total_ecc_errors(kind, EccCounter::Volatile).ok();
    Some(EccErrors {
        corrected: count(MemoryError::Corrected)?,
        uncorrected: count(MemoryError::Uncorrected)?,
    })
}

fn pcie_link(device: &Device<'_>) -> Option<PcieLink> {
    Some(PcieLink {
        generation: device.current_pcie_link_gen().ok()?,
        width: device.current_pcie_link_width().ok()?,
        max_generation: device.max_pcie_link_gen().ok()?,
        max_width: device.max_pcie_link_width().ok()?,
    })
}

/// Merges compute and graphics process lists with per-process utilization
/// (when `with_utilization`).
fn collect_processes(
    device: &Device<'_>,
    last_ts: Option<u64>,
    with_utilization: bool,
) -> (Vec<GpuProcess>, Option<u64>) {
    // A process can appear in both lists (e.g. CUDA + OpenGL); keep one entry
    // per pid with the larger memory figure.
    let mut by_pid: HashMap<u32, GpuProcess> = HashMap::new();
    let lists = [
        device.running_compute_processes(),
        device.running_graphics_processes(),
    ];
    for info in lists.into_iter().flat_map(|r| r.unwrap_or_default()) {
        let memory = used_memory(&info);
        let entry = by_pid.entry(info.pid).or_insert_with(|| GpuProcess {
            pid: info.pid,
            ..Default::default()
        });
        entry.memory_bytes = entry.memory_bytes.max(memory);
    }

    // Utilization samples: keep the newest sample of each pid. Skipped when
    // not wanted or when no process uses the GPU (often a second GPU).
    let mut newest_ts = last_ts;
    if !with_utilization || by_pid.is_empty() {
        let mut processes: Vec<GpuProcess> = by_pid.into_values().collect();
        processes.sort_unstable_by_key(|p| std::cmp::Reverse(p.memory_bytes));
        return (processes, newest_ts);
    }
    // NotFound means "no samples since last_ts", which is normal for idle
    // GPUs; NotSupported is common on older devices. Both just mean no data.
    let samples = device
        .process_utilization_stats(last_ts)
        .unwrap_or_default();
    let mut latest: HashMap<u32, (u64, u32)> = HashMap::new();
    for sample in samples {
        newest_ts = Some(newest_ts.map_or(sample.timestamp, |t| t.max(sample.timestamp)));
        let slot = latest.entry(sample.pid).or_insert((0, 0));
        if sample.timestamp >= slot.0 {
            *slot = (sample.timestamp, sample.sm_util);
        }
    }
    for (pid, (_, sm_util)) in latest {
        if let Some(process) = by_pid.get_mut(&pid) {
            process.sm_percent = Some(sm_util as f32);
        }
    }

    let mut processes: Vec<GpuProcess> = by_pid.into_values().collect();
    processes.sort_unstable_by_key(|p| std::cmp::Reverse(p.memory_bytes));
    (processes, newest_ts)
}

fn used_memory(info: &NvmlProcessInfo) -> Option<u64> {
    match info.used_gpu_memory {
        UsedGpuMemory::Used(bytes) => Some(bytes),
        UsedGpuMemory::Unavailable => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_reasons_worth_showing() {
        assert!(throttle_reasons(ThrottleReasons::GPU_IDLE).is_empty());
        assert!(throttle_reasons(ThrottleReasons::NONE).is_empty());
        assert_eq!(
            throttle_reasons(ThrottleReasons::SW_POWER_CAP | ThrottleReasons::GPU_IDLE),
            ["power cap"]
        );
        assert_eq!(
            throttle_reasons(ThrottleReasons::HW_SLOWDOWN | ThrottleReasons::HW_THERMAL_SLOWDOWN),
            ["hw thermal"]
        );
        assert_eq!(
            throttle_reasons(ThrottleReasons::HW_SLOWDOWN),
            ["hw slowdown"]
        );
    }
}
