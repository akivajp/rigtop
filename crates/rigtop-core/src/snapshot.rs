//! Serializable snapshot of everything rigtop displays.
//!
//! A [`Snapshot`] is the single unit of data exchanged between a data source
//! (local collectors, or a remote `rigtop --agent` over SSH) and a front-end
//! (TUI or GUI). Every type here derives `Serialize`/`Deserialize` so that the
//! same structure can be streamed over the wire unchanged.
//!
//! Conventions:
//! - Sizes are in bytes, rates in bytes per second.
//! - Temperatures are in degrees Celsius, power in watts, fan speed in RPM.
//! - Percentages are in the range `0.0..=100.0` (per-core values are not summed).
//! - Values that a platform or device cannot provide are `None` (or an empty
//!   list) rather than zero, so that front-ends can hide them.
//! - Every struct is `#[serde(default)]`: fields missing from the input take
//!   their default, so a client can read snapshots from an older or newer
//!   `rigtop --agent` as long as fields are only added.

use serde::{Deserialize, Serialize};

/// One full measurement of a host.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// Wall-clock time of the measurement, in milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
    /// Static-ish information about the host.
    pub host: HostInfo,
    /// CPU load, frequency and temperature.
    pub cpu: CpuStats,
    /// Physical memory and swap.
    pub memory: MemoryStats,
    /// Discrete and integrated GPUs that could be queried.
    pub gpus: Vec<GpuStats>,
    /// Raw hardware sensor chips (temperatures and fans), grouped by chip.
    pub sensors: Vec<SensorChip>,
    /// Power draw summary.
    pub power: PowerStats,
    /// I/O activity per physical block device.
    pub disks: Vec<DiskIo>,
    /// Capacity of mounted filesystems.
    pub filesystems: Vec<FilesystemUsage>,
    /// Software RAID state (btrfs and mdadm).
    pub raid: RaidStats,
    /// SMART health per drive, from files written by a privileged timer.
    pub smart: Vec<SmartDevice>,
    /// Network interface traffic.
    pub networks: Vec<NetworkStats>,
    /// Process list. May be truncated by the source (see [`Snapshot::truncate_processes`]).
    pub processes: Vec<ProcessInfo>,
    /// Total number of processes on the host, even if `processes` was truncated.
    pub process_count: usize,
    /// Windows: why LibreHardwareMonitor, the source of temperatures, fans
    /// and CPU power, could not be read. `None` when it was read, when it is
    /// disabled in the configuration, and on other platforms.
    pub lhm_error: Option<String>,
}

impl Snapshot {
    /// Keeps only the `limit` processes with the highest CPU usage.
    ///
    /// Used to reduce the payload when a host is shown in an overview rather
    /// than in detail. `process_count` keeps the original total.
    pub fn truncate_processes(&mut self, limit: usize) {
        if self.processes.len() <= limit {
            return;
        }
        // Partial sort is enough: only the top `limit` entries matter.
        self.processes
            .select_nth_unstable_by(limit, |a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        self.processes.truncate(limit);
        self.processes
            .sort_unstable_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
    }
}

/// Host identification and uptime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostInfo {
    /// Host name as reported by the OS.
    pub hostname: String,
    /// Human readable OS name and version (e.g. "Linux 26.04 Ubuntu").
    pub os: String,
    /// Kernel version string.
    pub kernel: String,
    /// Seconds since boot.
    pub uptime_secs: u64,
}

/// CPU-wide and per-core statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuStats {
    /// Brand string of the first CPU (e.g. "AMD Ryzen 9 9950X3D 16-Core Processor").
    pub model: String,
    /// Usage across all logical cores.
    pub total_percent: f32,
    /// Usage of each logical core, in OS order.
    pub per_core_percent: Vec<f32>,
    /// Current frequency of each logical core in MHz.
    pub per_core_mhz: Vec<u64>,
    /// 1, 5 and 15 minute load averages; `None` on platforms without them
    /// (Windows).
    pub load_average: Option<[f64; 3]>,
    /// CPU temperature readings, e.g. `Tctl`, `Tccd1`, `Tccd2` on AMD or
    /// `Package id 0`, `Core 0`.. on Intel.
    pub temperatures: Vec<Temperature>,
}

/// Physical memory and swap usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryStats {
    /// Installed (visible) memory.
    pub total_bytes: u64,
    /// Memory in use by processes (total minus available).
    pub used_bytes: u64,
    /// Memory that can be allocated without swapping.
    pub available_bytes: u64,
    /// Completely unused memory.
    pub free_bytes: u64,
    /// Page cache and reclaimable slab, where the platform reports it.
    pub cached_bytes: Option<u64>,
    /// Total swap space.
    pub swap_total_bytes: u64,
    /// Swap space in use.
    pub swap_used_bytes: u64,
}

/// A single GPU.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuStats {
    /// Index within its vendor API (NVML index for NVIDIA).
    pub index: u32,
    /// Vendor name ("NVIDIA", "AMD", ...).
    pub vendor: String,
    /// Marketing name of the device.
    pub name: String,
    /// Core utilization.
    pub utilization_percent: Option<f32>,
    /// Memory controller utilization.
    pub memory_utilization_percent: Option<f32>,
    /// VRAM in use.
    pub memory_used_bytes: Option<u64>,
    /// Total VRAM.
    pub memory_total_bytes: Option<u64>,
    /// Core temperature.
    pub temperature_c: Option<f32>,
    /// Current board power draw.
    pub power_w: Option<f32>,
    /// Enforced power limit.
    pub power_limit_w: Option<f32>,
    /// Fan speed as a percentage of the maximum (first fan).
    pub fan_percent: Option<f32>,
    /// Graphics clock in MHz.
    pub clock_mhz: Option<u32>,
    /// Processes currently using this GPU.
    pub processes: Vec<GpuProcess>,
}

/// GPU usage attributed to one process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuProcess {
    /// OS process id.
    pub pid: u32,
    /// VRAM used by the process, if the driver reports it.
    pub memory_bytes: Option<u64>,
    /// Streaming multiprocessor (core) utilization by the process.
    pub sm_percent: Option<f32>,
}

/// A hardware monitoring chip (a Linux hwmon device) and its readings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SensorChip {
    /// Stable identifier, `"<driver name>:<device name>"`
    /// (e.g. `"nct6686:nct6683.2592"`, `"nvme:nvme0"`).
    ///
    /// Unlike `hwmonN`, this survives reboots, so it is used as the key for
    /// user-defined labels.
    pub key: String,
    /// Driver-provided chip name (e.g. `"k10temp"`, `"nvme"`, `"spd5118"`).
    pub name: String,
    /// Temperature channels.
    pub temperatures: Vec<Temperature>,
    /// Fan channels.
    pub fans: Vec<Fan>,
}

/// One temperature channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Temperature {
    /// Stable key `"<chip key>/temp<N>"` used to look up user-defined names.
    /// Empty for readings that do not come from a hwmon chip.
    pub key: String,
    /// Driver label (e.g. `"Tccd1"`), or a generic `"temp<N>"` when none exists.
    pub label: String,
    /// Current reading.
    pub celsius: f32,
    /// High threshold, if exposed.
    pub max_celsius: Option<f32>,
    /// Critical threshold, if exposed.
    pub critical_celsius: Option<f32>,
}

/// One fan channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Fan {
    /// Stable key `"<chip key>/fan<N>"` used to look up user-defined names.
    pub key: String,
    /// Driver label, or a generic `"fan<N>"` when none exists.
    pub label: String,
    /// Current speed (0 when stopped or not connected).
    pub rpm: u32,
}

/// Power draw summary.
///
/// There is no portable way to measure whole-system power without external
/// hardware, so the total is the sum of the CPU package and discrete GPUs,
/// which dominate consumption on typical workstations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerStats {
    /// CPU package power (RAPL on Linux). `None` before the second sample or
    /// when RAPL is unavailable.
    pub cpu_package_w: Option<f32>,
    /// Sum of power reported by discrete GPUs.
    pub gpus_w: Option<f32>,
    /// Integrated GPU power (e.g. amdgpu PPT). Reported separately and *not*
    /// added to the total, because it may already be part of the CPU package.
    pub igpu_w: Option<f32>,
    /// `cpu_package_w + gpus_w`, counting only the parts that are available.
    pub total_w: Option<f32>,
}

/// I/O activity of a block device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskIo {
    /// Kernel device name (e.g. `"nvme0n1"`, `"sda"`).
    pub name: String,
    /// Read throughput.
    pub read_bytes_per_sec: f64,
    /// Write throughput.
    pub write_bytes_per_sec: f64,
    /// Fraction of time the device had I/O in flight, where the platform
    /// reports it (Linux).
    pub busy_percent: Option<f32>,
    /// Drive temperature (NVMe composite sensor), if available.
    pub temperature_c: Option<f32>,
}

/// Capacity of one filesystem.
///
/// Filesystems mounted at several places (e.g. btrfs subvolumes) appear once,
/// with all mount points listed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesystemUsage {
    /// Source device (e.g. `"/dev/nvme1n1p1"`).
    pub device: String,
    /// Filesystem type (e.g. `"btrfs"`, `"ext4"`).
    pub fs_type: String,
    /// All mount points of this filesystem, shortest first.
    pub mount_points: Vec<String>,
    /// Total size.
    pub total_bytes: u64,
    /// Space available to unprivileged users.
    pub available_bytes: u64,
}

/// Returns true if `device` (a disk such as `"sda"`, or an NVMe controller
/// such as `"nvme0"`) is the drive behind the block device `disk`
/// (`"sda"`, `"nvme0n1"`).
///
/// `"nvme1"` does not match `"nvme10n1"`: after the controller name only the
/// namespace suffix (`n<N>`) may follow.
pub fn device_matches_disk(device: &str, disk: &str) -> bool {
    match disk.strip_prefix(device) {
        Some("") => true,
        Some(rest) => device.starts_with("nvme") && rest.starts_with('n'),
        None => false,
    }
}

/// SMART health of one drive, parsed from `smartctl -j -a` output.
///
/// Fields a drive or protocol does not report are `None` (NVMe drives have
/// no sector reallocation counters, ATA drives no "percentage used").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartDevice {
    /// Kernel device name (`"nvme0"`, `"sda"`). See [`device_matches_disk`].
    pub device: String,
    /// Protocol reported by smartctl (`"NVMe"`, `"ATA"`, `"SCSI"`).
    pub protocol: String,
    /// Drive model.
    pub model: String,
    /// Overall health self-assessment; `None` if smartctl could not tell.
    pub passed: Option<bool>,
    /// Current drive temperature.
    pub temperature_c: Option<f32>,
    /// Accumulated power-on time.
    pub power_on_hours: Option<u64>,
    /// NVMe wear estimate (may exceed 100).
    pub percentage_used: Option<u32>,
    /// NVMe spare capacity left.
    pub available_spare_percent: Option<u32>,
    /// NVMe spare threshold below which the drive reports a warning.
    pub available_spare_threshold_percent: Option<u32>,
    /// NVMe critical warning bit field (0 when healthy).
    pub critical_warning: Option<u32>,
    /// NVMe unrecovered data integrity errors.
    pub media_errors: Option<u64>,
    /// ATA attribute 5 (Reallocated_Sector_Ct), raw value.
    pub reallocated_sectors: Option<u64>,
    /// ATA attribute 197 (Current_Pending_Sector), raw value.
    pub pending_sectors: Option<u64>,
    /// ATA attribute 198 (Offline_Uncorrectable), raw value.
    pub offline_uncorrectable: Option<u64>,
    /// Total host writes, where the drive reports them.
    pub data_written_bytes: Option<u64>,
    /// When smartctl ran, in milliseconds since the Unix epoch. Lets front-ends
    /// flag stale results (e.g. the timer stopped running).
    pub collected_at_ms: Option<u64>,
    /// Error reported by smartctl (e.g. the device could not be opened).
    pub error: Option<String>,
}

impl SmartDevice {
    /// Returns true if anything in the SMART data calls for attention.
    pub fn needs_attention(&self) -> bool {
        let positive = |v: Option<u64>| v.is_some_and(|v| v > 0);
        let spare_low = match (
            self.available_spare_percent,
            self.available_spare_threshold_percent,
        ) {
            (Some(spare), Some(threshold)) => spare < threshold,
            _ => false,
        };
        self.passed == Some(false)
            || self.error.is_some()
            || self.critical_warning.is_some_and(|w| w != 0)
            || positive(self.media_errors)
            || positive(self.reallocated_sectors)
            || positive(self.pending_sectors)
            || positive(self.offline_uncorrectable)
            || self.percentage_used.is_some_and(|p| p >= 100)
            || spare_low
    }
}

/// Software RAID state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RaidStats {
    /// Mounted btrfs filesystems.
    pub btrfs: Vec<BtrfsFilesystem>,
    /// Linux md (mdadm) arrays.
    pub md: Vec<MdArray>,
}

/// A btrfs filesystem as seen in `/sys/fs/btrfs/<fsid>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BtrfsFilesystem {
    /// Filesystem UUID.
    pub fsid: String,
    /// Filesystem label (may be empty).
    pub label: String,
    /// Mount points found in the mount table for this filesystem.
    pub mount_points: Vec<String>,
    /// Member block devices (e.g. `"nvme0n1"`, `"nvme1n1p1"`).
    pub devices: Vec<String>,
    /// Per-device error counters, keyed by btrfs device id.
    ///
    /// The kernel does not expose the devid-to-name mapping in sysfs, so this
    /// list is not aligned with `devices`.
    pub device_stats: Vec<BtrfsDeviceStats>,
    /// Allocation per block group type (data, metadata, system).
    pub allocations: Vec<BtrfsAllocation>,
    /// Result of the most recent scrub, if it is known. Error counters in
    /// `device_stats` only grow when data is read, so the scrub time tells
    /// when all data was last verified.
    pub scrub: Option<BtrfsScrub>,
}

/// Outcome of the last btrfs scrub (summed over devices).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BtrfsScrub {
    /// Start time, in milliseconds since the Unix epoch.
    pub started_at_ms: u64,
    /// Run time in seconds.
    pub duration_secs: u64,
    /// `"finished"`, `"canceled"` or `"unfinished"` (running or interrupted).
    pub state: String,
    /// Bytes verified.
    pub bytes_scrubbed: u64,
    /// Read, checksum, verify and superblock errors found.
    pub errors: u64,
    /// Errors that were repaired from a good copy.
    pub corrected: u64,
    /// Errors that could not be repaired.
    pub uncorrectable: u64,
}

impl BtrfsFilesystem {
    /// Returns true if any member is missing or has a non-zero error counter.
    pub fn has_problems(&self) -> bool {
        self.device_stats
            .iter()
            .any(|d| d.missing || d.total_errors() > 0)
    }
}

/// Error counters of one btrfs member device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BtrfsDeviceStats {
    /// btrfs device id (1-based).
    pub devid: u64,
    /// Whether the device is missing from the filesystem.
    pub missing: bool,
    /// Write I/O errors.
    pub write_errs: u64,
    /// Read I/O errors.
    pub read_errs: u64,
    /// Flush errors.
    pub flush_errs: u64,
    /// Checksum mismatches.
    pub corruption_errs: u64,
    /// Generation mismatches (stale data).
    pub generation_errs: u64,
}

impl BtrfsDeviceStats {
    /// Sum of all error counters.
    pub fn total_errors(&self) -> u64 {
        self.write_errs
            + self.read_errs
            + self.flush_errs
            + self.corruption_errs
            + self.generation_errs
    }
}

/// Space allocation of one btrfs block group type and RAID profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BtrfsAllocation {
    /// Block group type: `"data"`, `"metadata"` or `"system"`.
    pub kind: String,
    /// RAID profile (e.g. `"single"`, `"raid0"`, `"raid1c3"`).
    pub profile: String,
    /// Space allocated to this profile (logical size).
    pub total_bytes: u64,
    /// Space used within the allocation (logical size).
    pub used_bytes: u64,
}

/// One Linux md array parsed from `/proc/mdstat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MdArray {
    /// Array name (e.g. `"md0"`).
    pub name: String,
    /// Whether the array is active.
    pub active: bool,
    /// Whether the array is read-only (`active (read-only)` / `(auto-read-only)`).
    pub read_only: bool,
    /// RAID level (e.g. `"raid1"`); empty for inactive arrays.
    pub level: String,
    /// Member devices.
    pub members: Vec<MdMember>,
    /// Number of devices the array should have (`[n/m]` -> n).
    pub expected_devices: Option<u32>,
    /// Number of devices currently in sync (`[n/m]` -> m).
    pub working_devices: Option<u32>,
    /// Ongoing resync/recovery/reshape/check, if any.
    pub sync: Option<MdSync>,
}

impl MdArray {
    /// Returns true if the array runs with fewer devices than expected or has a failed member.
    pub fn is_degraded(&self) -> bool {
        let short = match (self.expected_devices, self.working_devices) {
            (Some(expected), Some(working)) => working < expected,
            _ => false,
        };
        short || self.members.iter().any(|m| m.faulty)
    }
}

/// A member device of an md array.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MdMember {
    /// Device name (e.g. `"sda1"`).
    pub name: String,
    /// Role number within the array.
    pub role: u32,
    /// Marked faulty `(F)`.
    pub faulty: bool,
    /// Spare device `(S)`.
    pub spare: bool,
}

/// Progress of an md background operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MdSync {
    /// Operation name: `"resync"`, `"recovery"`, `"reshape"` or `"check"`.
    pub action: String,
    /// Completion percentage.
    pub percent: f32,
    /// Estimated minutes until completion.
    pub finish_minutes: Option<f32>,
    /// Current speed in KiB/s.
    pub speed_kib_per_sec: Option<u64>,
}

/// Traffic of one network interface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkStats {
    /// Interface name (e.g. `"enp9s0"`).
    pub name: String,
    /// Receive throughput.
    pub rx_bytes_per_sec: f64,
    /// Transmit throughput.
    pub tx_bytes_per_sec: f64,
    /// Bytes received since the interface came up.
    pub rx_total_bytes: u64,
    /// Bytes transmitted since the interface came up.
    pub tx_total_bytes: u64,
    /// Negotiated link speed in Mbit/s (wired links), if known.
    pub link_speed_mbps: Option<u32>,
    /// Whether the link is up, if the OS reports it.
    pub link_up: Option<bool>,
    /// Wi-Fi link details, for wireless interfaces.
    pub wireless: Option<WirelessLink>,
}

/// State of a Wi-Fi link.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WirelessLink {
    /// Current receive bit rate in Mbit/s.
    pub rx_bitrate_mbps: Option<f32>,
    /// Current transmit bit rate in Mbit/s.
    pub tx_bitrate_mbps: Option<f32>,
    /// Signal strength in dBm.
    pub signal_dbm: Option<i32>,
}

/// One process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// Parent process id.
    pub parent_pid: Option<u32>,
    /// Short name (executable name).
    pub name: String,
    /// Full command line joined by spaces.
    pub command: String,
    /// Owner user name (or numeric uid when it cannot be resolved).
    pub user: String,
    /// Scheduling state (e.g. "Run", "Sleep", "Zombie").
    pub state: String,
    /// CPU usage; 100 means one full core, so multi-threaded processes may exceed 100.
    pub cpu_percent: f32,
    /// Resident memory.
    pub memory_bytes: u64,
    /// Number of threads, where known.
    pub threads: Option<u32>,
    /// Disk read throughput.
    pub read_bytes_per_sec: f64,
    /// Disk write throughput.
    pub write_bytes_per_sec: f64,
    /// Seconds since the process started.
    pub run_time_secs: u64,
    /// Kernel thread (e.g. `kworker`), which front-ends may hide.
    pub kernel_thread: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            cpu_percent: cpu,
            ..Default::default()
        }
    }

    #[test]
    fn truncate_keeps_top_cpu_processes_in_order() {
        let mut snapshot = Snapshot {
            processes: vec![
                process(1, 5.0),
                process(2, 50.0),
                process(3, 0.0),
                process(4, 20.0),
            ],
            process_count: 4,
            ..Default::default()
        };
        snapshot.truncate_processes(2);
        let pids: Vec<u32> = snapshot.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![2, 4]);
        // The total is preserved for display ("2 of 4 processes").
        assert_eq!(snapshot.process_count, 4);
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let snapshot = Snapshot {
            timestamp_ms: 42,
            processes: vec![process(7, 1.5)],
            ..Default::default()
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.timestamp_ms, 42);
        assert_eq!(back.processes[0].pid, 7);
    }

    #[test]
    fn device_matching_handles_nvme_namespaces() {
        assert!(device_matches_disk("sda", "sda"));
        assert!(device_matches_disk("nvme0", "nvme0n1"));
        assert!(device_matches_disk("nvme1", "nvme1n1"));
        assert!(!device_matches_disk("nvme1", "nvme10n1"));
        assert!(!device_matches_disk("sd", "sda"));
        assert!(!device_matches_disk("sda", "sdb"));
    }

    #[test]
    fn smart_attention_flags() {
        let healthy = SmartDevice {
            passed: Some(true),
            media_errors: Some(0),
            available_spare_percent: Some(100),
            available_spare_threshold_percent: Some(10),
            ..Default::default()
        };
        assert!(!healthy.needs_attention());
        let worn = SmartDevice {
            available_spare_percent: Some(5),
            ..healthy.clone()
        };
        assert!(worn.needs_attention());
        let reallocated = SmartDevice {
            reallocated_sectors: Some(8),
            ..healthy
        };
        assert!(reallocated.needs_attention());
    }

    #[test]
    fn md_array_degraded_detection() {
        let mut array = MdArray {
            expected_devices: Some(2),
            working_devices: Some(2),
            ..Default::default()
        };
        assert!(!array.is_degraded());
        array.working_devices = Some(1);
        assert!(array.is_degraded());
    }
}
