//! Data sources: anything that can produce [`Snapshot`]s.
//!
//! Front-ends only talk to the [`DataSource`] trait. The local machine is one
//! implementation ([`LocalSource`]); a remote host reached through
//! `ssh host rigtop --agent` will be another, so remote monitoring needs no
//! changes in the front-ends.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::gpu::NvmlCollector;
use crate::snapshot::{PowerStats, Snapshot};
use crate::system::SystemCollector;

#[cfg(target_os = "linux")]
use crate::linux::{self, SysRoots, diskstats::DiskStatsMeter, rapl::RaplMeter};

/// Shortest sensible interval between two polls. CPU usage is computed from
/// the difference between polls and is inaccurate below this.
pub const MIN_POLL_INTERVAL: Duration = sysinfo::MINIMUM_CPU_UPDATE_INTERVAL;

/// Errors a data source can report.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Reading from the underlying channel failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The source is no longer available (e.g. the SSH connection closed).
    #[error("source disconnected: {0}")]
    Disconnected(String),
}

/// A producer of snapshots, local or remote.
pub trait DataSource {
    /// Takes a new measurement.
    ///
    /// Rates (CPU %, throughput, power) cover the time since the previous
    /// call, so callers should poll at a steady interval of at least
    /// [`MIN_POLL_INTERVAL`].
    fn poll(&mut self) -> Result<Snapshot, SourceError>;
}

/// Collects snapshots of the machine rigtop runs on.
pub struct LocalSource {
    system: SystemCollector,
    nvml: NvmlCollector,
    last_poll: Option<Instant>,
    #[cfg(target_os = "linux")]
    roots: SysRoots,
    #[cfg(target_os = "linux")]
    rapl: RaplMeter,
    #[cfg(target_os = "linux")]
    diskstats: DiskStatsMeter,
}

impl LocalSource {
    /// Creates a source and takes a warm-up sample, so that the first
    /// [`DataSource::poll`] (after at least [`MIN_POLL_INTERVAL`]) already
    /// has rates.
    pub fn new() -> Self {
        let mut source = Self {
            system: SystemCollector::new(),
            nvml: NvmlCollector::new(),
            last_poll: None,
            #[cfg(target_os = "linux")]
            roots: SysRoots::default(),
            #[cfg(target_os = "linux")]
            rapl: RaplMeter::new(),
            #[cfg(target_os = "linux")]
            diskstats: DiskStatsMeter::new(),
        };
        // The warm-up result itself is meaningless (no previous sample).
        let _ = source.collect();
        source
    }

    /// Why NVIDIA GPUs are unavailable, if NVML failed to initialize.
    pub fn gpu_unavailable_reason(&self) -> Option<&str> {
        self.nvml.init_error()
    }

    fn collect(&mut self) -> Snapshot {
        let now = Instant::now();
        let elapsed = self
            .last_poll
            .map(|prev| now.saturating_duration_since(prev).as_secs_f64());
        self.last_poll = Some(now);

        self.system.refresh();
        let processes = self.system.processes(elapsed);
        let gpus = self.nvml.collect();
        let gpus_w = sum_present(gpus.iter().map(|g| g.power_w));

        let mut snapshot = Snapshot {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            host: self.system.host(),
            cpu: self.system.cpu(),
            memory: self.system.memory(),
            networks: self.system.networks(elapsed),
            filesystems: self.system.filesystems(),
            process_count: processes.len(),
            processes,
            gpus,
            power: PowerStats {
                gpus_w,
                ..Default::default()
            },
            ..Default::default()
        };

        #[cfg(target_os = "linux")]
        self.collect_linux(&mut snapshot, now);

        let power = &mut snapshot.power;
        power.total_w = sum_present([power.cpu_package_w, power.gpus_w]);
        snapshot
    }

    /// Adds sensors, power, per-device I/O and RAID state from `/sys` and `/proc`.
    #[cfg(target_os = "linux")]
    fn collect_linux(&mut self, snapshot: &mut Snapshot, now: Instant) {
        let chips = linux::hwmon::read_chips(&self.roots);

        snapshot.cpu.temperatures = chips
            .iter()
            .filter(|c| linux::hwmon::is_cpu_chip(&c.chip))
            .flat_map(|c| c.chip.temperatures.iter().cloned())
            .collect();
        snapshot.memory.cached_bytes = linux::meminfo::read_cached_bytes(&self.roots);
        snapshot.power.cpu_package_w = self.rapl.sample(&self.roots, now);
        // amdgpu power is treated as the integrated GPU (Ryzen iGPU).
        // Discrete AMD GPUs are not distinguished yet.
        snapshot.power.igpu_w = sum_present(
            chips
                .iter()
                .filter(|c| c.chip.name == "amdgpu")
                .flat_map(|c| c.power_w.iter().map(|(_, w)| Some(*w))),
        );

        let mut disks = self.diskstats.sample(&self.roots, now);
        for disk in &mut disks {
            disk.temperature_c = nvme_temperature(&chips, &disk.name);
        }
        snapshot.disks = disks;

        let mounts = linux::mounts::read_mounts(&self.roots);
        snapshot.raid.btrfs = linux::btrfs::read_filesystems(&self.roots, &mounts);
        snapshot.raid.md = linux::mdstat::read_arrays(&self.roots);

        snapshot.sensors = chips.into_iter().map(|c| c.chip).collect();
    }
}

impl Default for LocalSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSource for LocalSource {
    fn poll(&mut self) -> Result<Snapshot, SourceError> {
        Ok(self.collect())
    }
}

/// Sums the values that are present; `None` if none are.
fn sum_present(values: impl IntoIterator<Item = Option<f32>>) -> Option<f32> {
    values
        .into_iter()
        .flatten()
        .fold(None, |acc, v| Some(acc.unwrap_or(0.0) + v))
}

/// Finds the composite temperature of the NVMe controller backing `disk`
/// (hwmon chip key `nvme:nvme0` for disk `nvme0n1`).
#[cfg(target_os = "linux")]
fn nvme_temperature(chips: &[linux::hwmon::HwmonChip], disk: &str) -> Option<f32> {
    chips.iter().find_map(|c| {
        let controller = c.chip.key.strip_prefix("nvme:")?;
        // "nvme1" must not match "nvme10n1": require the namespace "n" right after.
        disk.strip_prefix(controller)?
            .starts_with('n')
            .then_some(())?;
        // The first channel is the "Composite" temperature the drive reports.
        c.chip.temperatures.first().map(|t| t.celsius)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_present_ignores_missing_values() {
        assert_eq!(sum_present([None, None]), None);
        assert_eq!(sum_present([Some(1.5), None, Some(2.0)]), Some(3.5));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nvme_temperature_matches_controller_exactly() {
        use crate::linux::hwmon::HwmonChip;
        use crate::snapshot::{SensorChip, Temperature};

        let chip = |key: &str, celsius: f32| HwmonChip {
            chip: SensorChip {
                key: key.to_owned(),
                name: "nvme".to_owned(),
                temperatures: vec![Temperature {
                    label: "Composite".to_owned(),
                    celsius,
                    ..Default::default()
                }],
                fans: Vec::new(),
            },
            power_w: Vec::new(),
        };
        let chips = vec![chip("nvme:nvme1", 40.0), chip("nvme:nvme10", 50.0)];
        assert_eq!(nvme_temperature(&chips, "nvme10n1"), Some(50.0));
        assert_eq!(nvme_temperature(&chips, "nvme1n1"), Some(40.0));
        assert_eq!(nvme_temperature(&chips, "sda"), None);
    }
}
