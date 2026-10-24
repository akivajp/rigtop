//! Portable statistics via the `sysinfo` crate: CPU load, memory, processes,
//! network traffic and filesystem capacity.
//!
//! Platform-specific extras (sensors, RAID, per-device I/O) live in
//! [`crate::linux`] and are merged by [`crate::source::LocalSource`].

use std::collections::HashMap;

use sysinfo::{
    CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind,
    ProcessesToUpdate, System, ThreadKind, UpdateKind, Users,
};

use crate::snapshot::{
    CpuStats, FilesystemUsage, HostInfo, MemoryStats, NetworkStats, ProcessInfo,
};

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
}

impl SystemCollector {
    /// Creates the collector and takes an initial reading.
    pub fn new() -> Self {
        let mut collector = Self {
            system: System::new(),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list_specifics(
                DiskRefreshKind::nothing().with_storage(),
            ),
            users: Users::new_with_refreshed_list(),
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
                .with_tasks()
                .with_user(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        );
        self.networks.refresh(true);
        self.disks
            .refresh_specifics(true, DiskRefreshKind::nothing().with_storage());
    }

    /// Host name, OS and uptime.
    pub fn host(&self) -> HostInfo {
        HostInfo {
            hostname: System::host_name().unwrap_or_default(),
            os: System::long_os_version().unwrap_or_default(),
            kernel: System::kernel_version().unwrap_or_default(),
            uptime_secs: System::uptime(),
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
            load_average: [load.one, load.five, load.fifteen],
            temperatures: Vec::new(),
        }
    }

    /// Memory and swap. `cached_bytes` is filled in by the caller where available.
    pub fn memory(&self) -> MemoryStats {
        let s = &self.system;
        MemoryStats {
            total_bytes: s.total_memory(),
            used_bytes: s.used_memory(),
            available_bytes: s.available_memory(),
            free_bytes: s.free_memory(),
            cached_bytes: None,
            swap_total_bytes: s.total_swap(),
            swap_used_bytes: s.used_swap(),
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
            let entry = by_device
                .entry(device.clone())
                .or_insert_with(|| FilesystemUsage {
                    device,
                    fs_type,
                    mount_points: Vec::new(),
                    total_bytes: disk.total_space(),
                    available_bytes: disk.available_space(),
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
            // On Linux, sysinfo also lists the threads of each process as
            // entries; drop them. Kernel threads (kworker, ...) are kept and
            // flagged, like btop does.
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
                    // The task set includes the main thread itself.
                    threads: p.tasks().map(|t| t.len().max(1) as u32),
                    read_bytes_per_sec: rate(disk.read_bytes, elapsed_secs),
                    write_bytes_per_sec: rate(disk.written_bytes, elapsed_secs),
                    run_time_secs: p.run_time(),
                    kernel_thread: p.thread_kind() == Some(ThreadKind::Kernel),
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

/// Converts a byte count accumulated over `elapsed_secs` into bytes per second.
fn rate(bytes: u64, elapsed_secs: Option<f64>) -> f64 {
    match elapsed_secs {
        Some(secs) if secs > 0.0 => bytes as f64 / secs,
        _ => 0.0,
    }
}
