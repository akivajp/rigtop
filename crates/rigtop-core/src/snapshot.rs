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
    /// Processes the client asked to hear about when they exit (see
    /// [`crate::DataSource::watch_processes`]): listed in `processes`
    /// whatever the limit while they run, so one of them missing has
    /// exited. Empty from agents before 0.19.
    pub watched_pids: Vec<u32>,
    /// Linux: systemd units in the failed state (e.g. `"nginx.service"`).
    pub failed_units: Vec<String>,
    /// Linux: how much of the time tasks waited for CPU, memory or I/O
    /// (pressure stall information). `None` without it (other platforms,
    /// kernels built or booted without PSI, agents before 0.21).
    pub pressure: Option<Pressure>,
    /// Windows: why LibreHardwareMonitor, the source of temperatures, fans
    /// and CPU power, could not be read. `None` when it was read, when it is
    /// disabled in the configuration, and on other platforms.
    pub lhm_error: Option<String>,
}

impl Snapshot {
    /// Keeps only the `limit` processes with the highest CPU usage.
    ///
    /// Used to reduce the payload when a host is shown in an overview rather
    /// than in detail. Kernel threads are dropped first: the overview does not
    /// show them, and busy `kworker`s would otherwise take places in the top
    /// `limit` and leave the list short. `process_count` keeps the original
    /// total. Processes in `watched_pids` stay, after the top `limit`.
    pub fn truncate_processes(&mut self, limit: usize) {
        self.processes.retain(|p| !p.kernel_thread);
        if self.processes.len() <= limit {
            return;
        }
        let (watched, mut others): (Vec<ProcessInfo>, Vec<ProcessInfo>) =
            std::mem::take(&mut self.processes)
                .into_iter()
                .partition(|p| self.watched_pids.contains(&p.pid));
        if others.len() > limit {
            // Partial sort is enough: only the top `limit` entries matter.
            others.select_nth_unstable_by(limit, |a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
            others.truncate(limit);
        }
        others.sort_unstable_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        others.extend(watched);
        self.processes = others;
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
    /// Version of the rigtop that took the snapshot (the agent's, for a
    /// remote host). Agents before 0.9.1 leave it out; the remote source
    /// fills it in from the agent's greeting.
    pub rigtop_version: String,
    /// A reboot is needed to finish an update (Linux), with the reason
    /// (possibly empty), e.g. "updated: linux-image-7.0.0-39-generic".
    pub reboot_required: Option<String>,
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
    /// `Package id 0` on Intel. Per-core readings that could be matched to
    /// logical cores are in [`CpuStats::per_core_celsius`] instead.
    pub temperatures: Vec<Temperature>,
    /// Temperature of the physical core of each logical core, in OS order
    /// (Intel's coretemp). Empty when the CPU has no per-core sensors.
    pub per_core_celsius: Vec<Option<f32>>,
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
    /// Processes the kernel's OOM killer stopped in the last hour, seen
    /// since rigtop started (Linux), oldest first.
    pub oom_kills: Vec<OomKill>,
}

/// Pressure stall information: the share of time tasks were stalled
/// waiting for a resource, from `/proc/pressure/`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pressure {
    pub cpu: PressureLevels,
    pub memory: PressureLevels,
    pub io: PressureLevels,
}

/// Stall percentages of one resource. `some`: at least one task waited
/// (others may have run); `full`: every task that wanted to run waited, so
/// the time was lost. The kernel's 10- and 60-second averages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PressureLevels {
    pub some_avg10: f32,
    pub some_avg60: f32,
    pub full_avg10: f32,
    pub full_avg60: f32,
}

/// One process stopped by the kernel for lack of memory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OomKill {
    /// When it was noticed (milliseconds since the Unix epoch).
    pub at_ms: u64,
    /// Name of the process that most likely was stopped: the largest one
    /// that went away at the same time (the kernel log saying which needs
    /// root).
    pub likely_process: Option<String>,
}

/// A single GPU.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuStats {
    /// Position in the GPU list: NVIDIA GPUs first (their NVML index),
    /// then AMD and Intel ones.
    pub index: u32,
    /// Vendor name ("NVIDIA", "AMD", ...).
    pub vendor: String,
    /// Marketing name of the device.
    pub name: String,
    /// Integrated graphics (an APU or Intel iGPU), whose power is part of
    /// the CPU package and not reported here.
    pub integrated: bool,
    /// Core utilization (unknown for Intel without privileges).
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
    /// Why the clocks are held down right now (NVIDIA): "power cap",
    /// "thermal", "hw thermal", "power brake" or "hw slowdown". Empty when
    /// the GPU runs freely or idles. Text, so that newer agents can add
    /// reasons older clients do not know.
    pub throttle_reasons: Vec<String>,
    /// ECC memory errors since the driver loaded (NVIDIA, with ECC on).
    pub ecc: Option<EccErrors>,
    /// The PCIe link (NVIDIA).
    pub pcie: Option<PcieLink>,
    /// Processes currently using this GPU.
    pub processes: Vec<GpuProcess>,
}

impl GpuStats {
    /// Throttle reasons that mean the hardware is slowing the GPU down, as
    /// opposed to running at its power limit as designed.
    pub fn slowdown_reasons(&self) -> impl Iterator<Item = &str> {
        self.throttle_reasons
            .iter()
            .map(String::as_str)
            .filter(|r| *r != "power cap")
    }
}

/// ECC memory error counts of a GPU.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EccErrors {
    /// Errors the ECC corrected.
    pub corrected: u64,
    /// Errors it could not correct (data was corrupted).
    pub uncorrected: u64,
}

/// A GPU's PCIe link: what it runs at now (lower when idle, to save power)
/// and the most the GPU and its slot allow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PcieLink {
    pub generation: u32,
    pub width: u32,
    pub max_generation: u32,
    pub max_width: u32,
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
    /// Power an AMD APU's graphics driver reports (amdgpu `PPT`). Per the
    /// kernel documentation it covers the whole SoC, CPU cores included, so
    /// it overlaps the CPU package and only stands in for it when RAPL cannot
    /// be read (see [`PowerStats::cpu_or_soc_w`]).
    pub igpu_w: Option<f32>,
    /// `cpu_package_w` (or `igpu_w` without it) `+ gpus_w`, counting only the
    /// parts that are available.
    pub total_w: Option<f32>,
    /// Laptop battery, if there is one.
    pub battery: Option<BatteryStats>,
}

/// Below this, an APU reading is not whole-SoC power: desktop Ryzen CPUs
/// with a small iGPU report only a few milliwatts there, while a laptop
/// APU's SoC never idles under a watt.
const MIN_SOC_W: f32 = 1.0;

impl PowerStats {
    /// The CPU figure to show: `("CPU", RAPL)`, else `("SoC", APU PPT)`.
    pub fn cpu_or_soc_w(&self) -> (&'static str, Option<f32>) {
        match (self.cpu_package_w, self.igpu_w) {
            (Some(w), _) => ("CPU", Some(w)),
            (None, Some(w)) if w >= MIN_SOC_W => ("SoC", Some(w)),
            _ => ("CPU", None),
        }
    }
}

/// Charge and power of the system's batteries (summed when there are
/// several), from `/sys/class/power_supply` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryStats {
    /// Charge in percent.
    pub percent: Option<f32>,
    /// Kernel status: `Charging`, `Discharging`, `Full`, `Not charging`...
    pub status: String,
    /// Charge or discharge power (not signed; see `status`).
    pub power_w: Option<f32>,
    /// Until empty while discharging, until full while charging, at the
    /// recent (smoothed) rate.
    pub time_left_secs: Option<u64>,
    /// Full charge as a share of the design capacity (wear), in percent.
    pub health_percent: Option<f32>,
}

impl BatteryStats {
    /// Short form for status lines: `72% -12W 3h10m` while discharging
    /// (with the time left when known), `+20W` while charging.
    pub fn summary(&self) -> String {
        let percent = self
            .percent
            .map_or("--%".to_owned(), |p| format!("{p:.0}%"));
        let sign = match self.status.as_str() {
            "Discharging" => "-",
            "Charging" => "+",
            _ => return percent,
        };
        let mut text = match self.power_w {
            Some(w) if w >= 0.5 => format!("{percent} {sign}{w:.0}W"),
            _ => percent,
        };
        if let Some(secs) = self.time_left_secs {
            text.push(' ');
            text.push_str(&crate::format::hours_minutes(secs));
        }
        text
    }

    /// The health worth showing next to the charge: below 80% of the design
    /// capacity, where batteries are usually considered worn.
    pub fn health_note(&self) -> Option<String> {
        self.health_percent
            .filter(|h| *h < 80.0)
            .map(|h| format!("health {h:.0}%"))
    }

    /// Hover text: what the time means, and the health.
    pub fn details(&self) -> String {
        let mut lines = Vec::new();
        if let Some(secs) = self.time_left_secs {
            let until = if self.status == "Charging" {
                "full"
            } else {
                "empty"
            };
            lines.push(format!(
                "{} until {until} at the recent rate",
                crate::format::hours_minutes(secs)
            ));
        }
        if let Some(health) = self.health_percent {
            lines.push(format!("full charge {health:.0}% of the design capacity"));
        }
        if lines.is_empty() {
            lines.push(self.status.clone());
        }
        lines.join("\n")
    }
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
    /// Average time one request took (queue and service), over the requests
    /// completed since the last sample (Linux). `None` without any.
    pub latency_ms: Option<f32>,
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
    /// Seconds until it is full at the growth of the last half hour; `None`
    /// when usage does not grow, without enough history yet, and from
    /// agents before 0.19 (see [`crate::fill`]).
    pub full_in_secs: Option<u64>,
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
    /// Imported ZFS pools.
    pub zfs: Vec<ZfsPool>,
}

/// A ZFS pool and its health.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ZfsPool {
    /// Pool name (e.g. `"tank"`).
    pub name: String,
    /// Health as `zpool status` shows it: `ONLINE`, `DEGRADED`, `FAULTED`,
    /// `OFFLINE`, `UNAVAIL`, `REMOVED` or `SUSPENDED`.
    pub state: String,
}

impl ZfsPool {
    /// Whether the pool is healthy.
    pub fn is_online(&self) -> bool {
        self.state == "ONLINE"
    }

    /// Whether the pool lost redundancy or data is at risk (worse than an
    /// administratively offline pool).
    pub fn is_failing(&self) -> bool {
        matches!(
            self.state.as_str(),
            "DEGRADED" | "FAULTED" | "UNAVAIL" | "REMOVED" | "SUSPENDED"
        )
    }
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
    /// Per-device error counters, keyed by btrfs device id. Each carries its
    /// device name when it could be looked up (see [`BtrfsDeviceStats::name`]);
    /// the list is not aligned with `devices`.
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
    /// Device name (e.g. `"nvme0n1"`), from the `BTRFS_IOC_DEV_INFO` ioctl
    /// on a mount point; empty when unknown (not mounted, device missing).
    pub name: String,
    /// Size of the device as btrfs uses it.
    pub size_bytes: Option<u64>,
    /// Space allocated on the device.
    pub used_bytes: Option<u64>,
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
    /// Name for display: the device name, else `devid N`.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            format!("devid {}", self.devid)
        } else {
            self.name.clone()
        }
    }

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
    /// The container it runs in (Docker, Podman, ... name, or short id),
    /// if any.
    pub container: Option<String>,
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
    fn cpu_power_falls_back_to_a_laptop_apu_only() {
        let power = |cpu: Option<f32>, igpu: Option<f32>| PowerStats {
            cpu_package_w: cpu,
            igpu_w: igpu,
            ..Default::default()
        };
        assert_eq!(
            power(Some(30.0), Some(6.0)).cpu_or_soc_w(),
            ("CPU", Some(30.0))
        );
        assert_eq!(power(None, Some(6.0)).cpu_or_soc_w(), ("SoC", Some(6.0)));
        // A desktop Ryzen iGPU's few milliwatts are not the SoC.
        assert_eq!(power(None, Some(0.007)).cpu_or_soc_w(), ("CPU", None));
        assert_eq!(power(None, None).cpu_or_soc_w(), ("CPU", None));
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
    fn truncate_leaves_out_kernel_threads() {
        let kworker = |pid: u32, cpu: f32| ProcessInfo {
            kernel_thread: true,
            ..process(pid, cpu)
        };
        let mut snapshot = Snapshot {
            processes: vec![
                kworker(1, 9.0),
                process(2, 5.0),
                kworker(3, 8.0),
                process(4, 1.0),
                process(5, 0.0),
            ],
            process_count: 5,
            ..Default::default()
        };
        snapshot.truncate_processes(3);
        let pids: Vec<u32> = snapshot.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![2, 4, 5], "busy kworkers take no places");
        assert_eq!(snapshot.process_count, 5);
    }

    #[test]
    fn truncate_keeps_watched_processes() {
        let mut snapshot = Snapshot {
            processes: (1..=6).map(|pid| process(pid, pid as f32)).collect(),
            watched_pids: vec![1, 99],
            ..Default::default()
        };
        snapshot.truncate_processes(2);
        let pids: Vec<u32> = snapshot.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![6, 5, 1]);
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
