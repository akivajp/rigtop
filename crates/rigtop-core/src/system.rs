//! Portable statistics via the `sysinfo` crate: CPU load, memory, processes,
//! network traffic and filesystem capacity.
//!
//! Platform-specific extras (sensors, RAID, per-device I/O) live in
//! [`crate::linux`] and are merged by [`crate::source::LocalSource`]. On other
//! platforms this module also provides per-volume I/O and the temperatures
//! `sysinfo` can see.

use std::collections::HashMap;

use sysinfo::{
    CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind,
    ProcessesToUpdate, System, ThreadKind, UpdateKind, Users,
};

use crate::snapshot::{
    CpuStats, FilesystemUsage, HostInfo, MemoryStats, NetworkStats, ProcessInfo,
};
#[cfg(not(target_os = "linux"))]
use crate::snapshot::{DiskIo, SensorChip, Temperature};

/// What to refresh for disks: capacity everywhere; per-volume I/O counters
/// only where `/proc/diskstats` (per physical device) is not available.
fn disk_refresh_kind() -> DiskRefreshKind {
    let kind = DiskRefreshKind::nothing().with_storage();
    if cfg!(target_os = "linux") {
        kind
    } else {
        kind.with_io_usage()
    }
}

/// Filesystem types that are never interesting in a disk panel.
const PSEUDO_FILESYSTEMS: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "squashfs",
    "overlay",
    "efivarfs",
    "ramfs",
    "proc",
    "sysfs",
    "cgroup2",
    "fuse.portal",
    "fuse.snapfuse",
];

/// Wraps the `sysinfo` handles that must persist between refreshes
/// (CPU usage and rates are computed from differences).
pub struct SystemCollector {
    system: System,
    networks: Networks,
    disks: Disks,
    users: Users,
    /// Temperature sensors, where hwmon is not available.
    #[cfg(not(target_os = "linux"))]
    components: sysinfo::Components,
}

impl SystemCollector {
    /// Creates the collector and takes an initial reading.
    pub fn new() -> Self {
        let mut collector = Self {
            system: System::new(),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list_specifics(disk_refresh_kind()),
            users: Users::new_with_refreshed_list(),
            #[cfg(not(target_os = "linux"))]
            components: sysinfo::Components::new_with_refreshed_list(),
        };
        collector.refresh();
        collector
    }

    /// Refreshes everything this collector reports.
    pub fn refresh(&mut self) {
        self.system
            .refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage().with_frequency());
        self.system
            .refresh_memory_specifics(MemoryRefreshKind::everything());
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_disk_usage()
                // Tasks are on by default and make sysinfo refresh every
                // thread as well (several /proc reads each), most of the
                // collection time on busy desktops. Thread counts come from
                // /proc/<pid>/stat on Linux instead.
                .without_tasks()
                .with_user(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        );
        self.networks.refresh(true);
        self.disks.refresh_specifics(true, disk_refresh_kind());
    }

    /// Host name, OS and uptime.
    pub fn host(&self) -> HostInfo {
        HostInfo {
            hostname: System::host_name().unwrap_or_default(),
            os: System::long_os_version().unwrap_or_default(),
            kernel: System::kernel_version().unwrap_or_default(),
            uptime_secs: System::uptime(),
            rigtop_version: env!("CARGO_PKG_VERSION").to_owned(),
            reboot_required: None,
        }
    }

    /// CPU load and frequencies. Temperatures are filled in by the caller.
    pub fn cpu(&self) -> CpuStats {
        let cpus = self.system.cpus();
        let load = System::load_average();
        CpuStats {
            model: cpus
                .first()
                .map(|c| c.brand().trim().to_owned())
                .unwrap_or_default(),
            total_percent: self.system.global_cpu_usage(),
            per_core_percent: cpus.iter().map(|c| c.cpu_usage()).collect(),
            per_core_mhz: cpus.iter().map(|c| c.frequency()).collect(),
            // Windows has no load average (sysinfo reports zeros).
            load_average: cfg!(unix).then_some([load.one, load.five, load.fifteen]),
            temperatures: Vec::new(),
            per_core_celsius: Vec::new(),
        }
    }

    /// Memory and swap. On Linux `cached_bytes` is filled in by the caller.
    pub fn memory(&self) -> MemoryStats {
        let s = &self.system;
        // On macOS sysinfo's available memory is the kernel's memory-pressure
        // headroom (active + inactive + free pages), which overlaps "used"
        // (Activity Monitor's Memory Used). Show what is left of the total
        // instead, as Activity Monitor does.
        let available_bytes = if cfg!(target_os = "macos") {
            s.total_memory().saturating_sub(s.used_memory())
        } else {
            s.available_memory()
        };
        #[cfg(target_os = "macos")]
        let cached_bytes = macos_cached_bytes();
        #[cfg(not(target_os = "macos"))]
        let cached_bytes = None;
        MemoryStats {
            total_bytes: s.total_memory(),
            used_bytes: s.used_memory(),
            available_bytes,
            free_bytes: s.free_memory(),
            cached_bytes,
            swap_total_bytes: s.total_swap(),
            swap_used_bytes: s.used_swap(),
            oom_kills: Vec::new(),
        }
    }

    /// Network traffic per interface. `elapsed_secs` is the time since the
    /// previous refresh; rates are zero when it is unknown.
    pub fn networks(&self, elapsed_secs: Option<f64>) -> Vec<NetworkStats> {
        let mut networks: Vec<NetworkStats> = self
            .networks
            .iter()
            // Loopback traffic is local IPC, not network activity.
            .filter(|(name, _)| name.as_str() != "lo")
            .map(|(name, data)| NetworkStats {
                name: name.clone(),
                rx_bytes_per_sec: rate(data.received(), elapsed_secs),
                tx_bytes_per_sec: rate(data.transmitted(), elapsed_secs),
                rx_total_bytes: data.total_received(),
                tx_total_bytes: data.total_transmitted(),
                // Link state is platform-specific and filled in by the caller.
                ..Default::default()
            })
            .collect();
        networks.sort_by(|a, b| a.name.cmp(&b.name));
        networks
    }

    /// Mounted filesystems, merged by source device so that btrfs subvolumes
    /// and bind mounts appear once.
    pub fn filesystems(&self) -> Vec<FilesystemUsage> {
        let mut by_device: HashMap<String, FilesystemUsage> = HashMap::new();
        for disk in self.disks.list() {
            let fs_type = disk.file_system().to_string_lossy().into_owned();
            if PSEUDO_FILESYSTEMS.contains(&fs_type.as_str()) || disk.total_space() == 0 {
                continue;
            }
            let device = disk.name().to_string_lossy().into_owned();
            let mount_point = disk.mount_point().to_string_lossy().into_owned();
            // On Unix the name is the source device, which merges btrfs
            // subvolumes and bind mounts. On Windows it is the volume label,
            // often empty or shared, so each drive letter stands alone.
            let key = if cfg!(windows) || device.is_empty() {
                mount_point.clone()
            } else {
                device.clone()
            };
            let entry = by_device.entry(key).or_insert_with(|| FilesystemUsage {
                device,
                fs_type,
                mount_points: Vec::new(),
                total_bytes: disk.total_space(),
                available_bytes: disk.available_space(),
                full_in_secs: None,
            });
            entry.mount_points.push(mount_point);
        }
        let mut filesystems: Vec<FilesystemUsage> = by_device.into_values().collect();
        for fs in &mut filesystems {
            fs.mount_points.sort_by_key(|m| (m.len(), m.clone()));
        }
        // Order by the most prominent mount point ("/" first).
        filesystems.sort_by(|a, b| {
            let key = |f: &FilesystemUsage| f.mount_points.first().cloned().unwrap_or_default();
            (key(a).len(), key(a)).cmp(&(key(b).len(), key(b)))
        });
        filesystems
    }

    /// I/O rates per mounted volume, for platforms without per-device
    /// counters (Linux uses `/proc/diskstats` instead).
    #[cfg(not(target_os = "linux"))]
    pub fn volume_io(&self, elapsed_secs: Option<f64>) -> Vec<DiskIo> {
        let mut volumes: Vec<DiskIo> = self
            .disks
            .list()
            .iter()
            .filter(|disk| disk.total_space() > 0)
            .map(|disk| {
                let usage = disk.usage();
                // "C:\" -> "C:"; elsewhere the mount point as is.
                let mount = disk.mount_point().to_string_lossy();
                let name = mount.trim_end_matches(['\\', '/']);
                DiskIo {
                    name: if name.is_empty() {
                        mount.to_string()
                    } else {
                        name.to_owned()
                    },
                    read_bytes_per_sec: rate(usage.read_bytes, elapsed_secs),
                    write_bytes_per_sec: rate(usage.written_bytes, elapsed_secs),
                    ..Default::default()
                }
            })
            .collect();
        volumes.sort_by(|a, b| a.name.cmp(&b.name));
        volumes.dedup_by(|a, b| a.name == b.name);
        volumes
    }

    /// Temperatures `sysinfo` can read (on Windows only what WMI exposes,
    /// often nothing without vendor drivers), as one sensor chip plus the
    /// readings that look like CPU sensors.
    #[cfg(not(target_os = "linux"))]
    pub fn components(&mut self) -> (Vec<SensorChip>, Vec<Temperature>) {
        self.components.refresh(true);
        let temperatures: Vec<Temperature> = self
            .components
            .list()
            .iter()
            .filter_map(|c| {
                Some(Temperature {
                    key: format!("system/{}", c.label()),
                    label: c.label().to_owned(),
                    celsius: c.temperature()?,
                    max_celsius: c.max(),
                    critical_celsius: c.critical(),
                })
            })
            .collect();
        let cpu: Vec<Temperature> = temperatures
            .iter()
            .filter(|t| is_cpu_sensor_label(&t.label))
            .cloned()
            .collect();
        let chips = if temperatures.is_empty() {
            Vec::new()
        } else {
            vec![SensorChip {
                key: "system".to_owned(),
                name: "system".to_owned(),
                temperatures,
                fans: Vec::new(),
            }]
        };
        (chips, cpu)
    }

    /// All processes (threads excluded). `elapsed_secs` converts the I/O
    /// byte counts since the previous refresh into rates.
    pub fn processes(&mut self, elapsed_secs: Option<f64>) -> Vec<ProcessInfo> {
        // Resolve uids lazily: refresh the user list only when an unknown uid shows up
        // (e.g. a user created after rigtop started).
        let unknown_uid = self.system.processes().values().any(|p| {
            p.user_id()
                .is_some_and(|uid| self.users.get_user_by_id(uid).is_none())
        });
        if unknown_uid {
            self.users.refresh();
        }

        self.system
            .processes()
            .values()
            // With tasks refreshed, sysinfo would also list each thread as
            // an entry; make sure none slip in. Kernel threads (kworker,
            // ...) are kept and flagged, like btop does.
            .filter(|p| p.thread_kind() != Some(ThreadKind::Userland))
            .map(|p| {
                let disk = p.disk_usage();
                let user = p
                    .user_id()
                    .map(|uid| {
                        self.users
                            .get_user_by_id(uid)
                            .map(|u| u.name().to_owned())
                            .unwrap_or_else(|| uid.to_string())
                    })
                    .unwrap_or_default();
                let command = p
                    .cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ");
                ProcessInfo {
                    pid: p.pid().as_u32(),
                    parent_pid: p.parent().map(|pid| pid.as_u32()),
                    name: p.name().to_string_lossy().into_owned(),
                    command,
                    user,
                    state: p.status().to_string(),
                    cpu_percent: p.cpu_usage(),
                    memory_bytes: p.memory(),
                    // Filled in from /proc/<pid>/stat on Linux; sysinfo has
                    // no count without refreshing every thread.
                    threads: None,
                    // Also from /proc/<pid>/stat on Linux.
                    nice: None,
                    read_bytes_per_sec: rate(disk.read_bytes, elapsed_secs),
                    write_bytes_per_sec: rate(disk.written_bytes, elapsed_secs),
                    run_time_secs: p.run_time(),
                    kernel_thread: p.thread_kind() == Some(ThreadKind::Kernel),
                    // Filled in from /proc/<pid>/cgroup on Linux.
                    container: None,
                }
            })
            .collect()
    }
}

impl Default for SystemCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a sensor label from `sysinfo` describes the CPU (e.g. "CPU",
/// "CPU Package", "Tctl", "Core 0").
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn is_cpu_sensor_label(label: &str) -> bool {
    let label = label.to_lowercase();
    ["cpu", "package", "tctl", "tdie", "tccd", "core "]
        .iter()
        .any(|word| label.contains(word))
}

/// File-backed pages on macOS (Activity Monitor's "Cached Files"), the
/// counterpart of Linux's page cache; `None` if the kernel does not answer.
#[cfg(target_os = "macos")]
fn macos_cached_bytes() -> Option<u64> {
    use std::sync::OnceLock;

    // Every mach_host_self() call adds a reference to the port, so keep one.
    static HOST: OnceLock<libc::mach_port_t> = OnceLock::new();
    // SAFETY: mach_host_self takes no arguments and returns a port name.
    #[allow(deprecated)]
    let host = *HOST.get_or_init(|| unsafe { libc::mach_host_self() });

    // SAFETY: an all-zero vm_statistics64 is valid (plain integers).
    let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `stats` is a vm_statistics64 and `count` its size in integers,
    // as HOST_VM_INFO64 requires.
    let result = unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&mut stats as *mut libc::vm_statistics64).cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    // SAFETY: sysconf has no preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u64::try_from(page_size).ok().filter(|&size| size > 0)?;
    Some(u64::from(stats.external_page_count) * page_size)
}

/// Converts a byte count accumulated over `elapsed_secs` into bytes per second.
fn rate(bytes: u64, elapsed_secs: Option<f64>) -> f64 {
    match elapsed_secs {
        Some(secs) if secs > 0.0 => bytes as f64 / secs,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_sensor_labels() {
        for label in [
            "CPU",
            "CPU Package",
            "Tctl",
            "Core 3",
            "coretemp Package id 0",
        ] {
            assert!(is_cpu_sensor_label(label), "{label}");
        }
        for label in ["GPU", "nvme Composite", "acpitz"] {
            assert!(!is_cpu_sensor_label(label), "{label}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_memory_adds_up() {
        let memory = SystemCollector::new().memory();
        assert!(memory.total_bytes > 0);
        assert!(memory.used_bytes + memory.available_bytes <= memory.total_bytes);
        let cached = memory.cached_bytes.expect("host_statistics64 answers");
        assert!(cached > 0 && cached < memory.total_bytes, "{cached}");
    }
}
