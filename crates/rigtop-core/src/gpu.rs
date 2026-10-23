//! NVIDIA GPU statistics via NVML.
//!
//! `nvml-wrapper` loads `libnvidia-ml` at runtime, so machines without an
//! NVIDIA driver simply report no GPUs instead of failing to start.

use std::collections::HashMap;

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::enums::device::UsedGpuMemory;
use nvml_wrapper::{Device, struct_wrappers::device::ProcessInfo as NvmlProcessInfo};

use crate::snapshot::{GpuProcess, GpuStats};

/// Collects per-GPU and per-process statistics from NVML.
pub struct NvmlCollector {
    /// `None` when the library or driver is unavailable.
    nvml: Option<Nvml>,
    /// Why NVML could not be initialized, for diagnostics.
    init_error: Option<String>,
    /// Newest per-process utilization sample timestamp seen, per GPU index.
    /// Passing it back to NVML returns only samples newer than the last poll.
    last_sample_ts: HashMap<u32, u64>,
}

impl NvmlCollector {
    /// Initializes NVML. Never fails; check [`NvmlCollector::init_error`] for the reason
    /// when no GPUs are reported.
    pub fn new() -> Self {
        match Nvml::init() {
            Ok(nvml) => Self {
                nvml: Some(nvml),
                init_error: None,
                last_sample_ts: HashMap::new(),
            },
            Err(err) => Self {
                nvml: None,
                init_error: Some(err.to_string()),
                last_sample_ts: HashMap::new(),
            },
        }
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
            let (stats, newest_ts) = collect_device(index, &device, last_ts);
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
) -> (GpuStats, Option<u64>) {
    let utilization = device.utilization_rates().ok();
    let memory = device.memory_info().ok();
    let (processes, newest_ts) = collect_processes(device, last_ts);
    let stats = GpuStats {
        index,
        vendor: "NVIDIA".to_owned(),
        name: device.name().unwrap_or_else(|_| format!("GPU {index}")),
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
        processes,
    };
    (stats, newest_ts)
}

/// Merges compute and graphics process lists with per-process utilization.
fn collect_processes(device: &Device<'_>, last_ts: Option<u64>) -> (Vec<GpuProcess>, Option<u64>) {
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

    // Utilization samples: keep the newest sample of each pid.
    let mut newest_ts = last_ts;
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
