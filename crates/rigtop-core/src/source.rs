//! Data sources: anything that can produce [`Snapshot`]s.
//!
//! Front-ends only talk to the [`DataSource`] trait. The local machine is one
//! implementation ([`LocalSource`]); a remote host reached through
//! `ssh host rigtop --agent` will be another, so remote monitoring needs no
//! changes in the front-ends.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::gpu::NvmlCollector;
use crate::smart::SmartReader;
#[cfg(target_os = "linux")]
use crate::snapshot::device_matches_disk;
use crate::snapshot::{PowerStats, Snapshot};
use crate::system::SystemCollector;

#[cfg(target_os = "linux")]
use crate::linux::{
    self, SysRoots, diskstats::DiskStatsMeter, net::NetLinkReader, rapl::RaplMeter,
};

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
    /// The remote agent reported a problem or does not speak our protocol.
    #[error("remote: {0}")]
    Remote(String),
    /// Sending a signal to a process failed (no such process, permission denied, ...).
    #[error("cannot signal process {pid}: {source}")]
    Signal {
        /// Target process id.
        pid: u32,
        /// Underlying OS error.
        source: std::io::Error,
    },
}

/// Signals a front-end can send to a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProcessSignal {
    /// Ask the process to exit (SIGTERM).
    Terminate,
    /// Force the process to exit (SIGKILL).
    Kill,
}

/// A producer of snapshots, local or remote.
pub trait DataSource {
    /// Takes a new measurement.
    ///
    /// Rates (CPU %, throughput, power) cover the time since the previous
    /// call, so callers should poll at a steady interval of at least
    /// [`MIN_POLL_INTERVAL`].
    fn poll(&mut self) -> Result<Snapshot, SourceError>;

    /// Sends `signal` to process `pid` on the monitored host, with the
    /// privileges of the user the source runs as.
    fn signal_process(&mut self, pid: u32, signal: ProcessSignal) -> Result<(), SourceError>;

    /// Tells the source how often it will be polled. Remote sources forward
    /// this to the agent, which samples on its own schedule; local sources
    /// need nothing.
    fn set_interval(&mut self, _interval: Duration) {}

    /// Limits snapshots to the top `limit` processes by CPU (`None`: all).
    /// Remote sources use this to save bandwidth for hosts in an overview.
    fn set_process_limit(&mut self, _limit: Option<usize>) {}

    /// Processes to always include in snapshots, whatever the process
    /// limit, and to list in [`Snapshot::watched_pids`], so that the client
    /// can tell when they exit. Replaces the previous list.
    fn watch_processes(&mut self, _pids: Vec<u32>) {}

    /// Like [`DataSource::watch_processes`] for every process matching one
    /// of `names` (see [`crate::view::process_matches`]) while it runs, and
    /// in the snapshot after it exited, so that the client sees it go.
    /// Replaces the previous list.
    fn watch_process_names(&mut self, _names: Vec<String>) {}
}

/// Boxed sources, so front-ends can choose local or remote at run time.
impl<T: DataSource + ?Sized> DataSource for Box<T> {
    fn poll(&mut self) -> Result<Snapshot, SourceError> {
        (**self).poll()
    }

    fn signal_process(&mut self, pid: u32, signal: ProcessSignal) -> Result<(), SourceError> {
        (**self).signal_process(pid, signal)
    }

    fn set_interval(&mut self, interval: Duration) {
        (**self).set_interval(interval);
    }

    fn set_process_limit(&mut self, limit: Option<usize>) {
        (**self).set_process_limit(limit);
    }

    fn watch_processes(&mut self, pids: Vec<u32>) {
        (**self).watch_processes(pids);
    }

    fn watch_process_names(&mut self, names: Vec<String>) {
        (**self).watch_process_names(names);
    }
}

/// Collects snapshots of the machine rigtop runs on.
pub struct LocalSource {
    system: SystemCollector,
    nvml: NvmlCollector,
    smart: SmartReader,
    last_poll: Option<Instant>,
    /// See [`DataSource::watch_processes`].
    watched_pids: Vec<u32>,
    /// See [`DataSource::watch_process_names`], and the pids they matched
    /// in the last snapshot.
    watched_names: Vec<String>,
    name_matches: Vec<u32>,
    fill: crate::fill::FillTracker,
    #[cfg(target_os = "linux")]
    roots: SysRoots,
    #[cfg(target_os = "linux")]
    rapl: RaplMeter,
    #[cfg(target_os = "linux")]
    diskstats: DiskStatsMeter,
    #[cfg(target_os = "linux")]
    net_links: NetLinkReader,
    #[cfg(target_os = "linux")]
    failed_units: linux::systemd::FailedUnitsReader,
    #[cfg(target_os = "linux")]
    containers: linux::containers::ContainerTracker,
    #[cfg(target_os = "linux")]
    oom: linux::oom::OomTracker,
    /// Directories searched for btrfs scrub status files, in order.
    #[cfg(target_os = "linux")]
    scrub_dirs: Vec<std::path::PathBuf>,
    /// AMD and Intel GPUs (NVIDIA ones come from NVML).
    #[cfg(target_os = "linux")]
    drm_gpus: linux::drm::DrmGpuReader,
    /// Temperatures, fans and CPU power from LibreHardwareMonitor.
    #[cfg(windows)]
    lhm: crate::lhm::LhmReader,
}

impl LocalSource {
    /// Creates a source with the default configuration. See [`LocalSource::with_config`].
    pub fn new() -> Self {
        Self::with_config(&Config::default())
    }

    /// Creates a source and takes a warm-up sample, so that the first
    /// [`DataSource::poll`] (after at least [`MIN_POLL_INTERVAL`]) already
    /// has rates.
    ///
    /// Only collection-related settings (the SMART directory) are used here;
    /// display settings such as sensor labels are applied by front-ends.
    pub fn with_config(config: &Config) -> Self {
        let mut source = Self {
            system: SystemCollector::new(),
            nvml: NvmlCollector::new(),
            smart: SmartReader::new(&config.smart.dir),
            last_poll: None,
            watched_pids: Vec::new(),
            watched_names: Vec::new(),
            name_matches: Vec::new(),
            fill: crate::fill::FillTracker::new(),
            #[cfg(target_os = "linux")]
            roots: SysRoots::default(),
            #[cfg(target_os = "linux")]
            rapl: RaplMeter::new(),
            #[cfg(target_os = "linux")]
            diskstats: DiskStatsMeter::new(),
            #[cfg(target_os = "linux")]
            net_links: NetLinkReader::new(),
            #[cfg(target_os = "linux")]
            failed_units: linux::systemd::FailedUnitsReader::new(),
            #[cfg(target_os = "linux")]
            containers: linux::containers::ContainerTracker::new(),
            #[cfg(target_os = "linux")]
            oom: linux::oom::OomTracker::new(),
            #[cfg(target_os = "linux")]
            drm_gpus: linux::drm::DrmGpuReader::new(),
            // The copy made by the root collector first, then btrfs-progs'
            // own file in case rigtop runs as root.
            #[cfg(windows)]
            lhm: crate::lhm::LhmReader::new(&config.lhm.url),
            #[cfg(target_os = "linux")]
            scrub_dirs: vec![
                config.btrfs.scrub_dir.clone(),
                linux::btrfs::BTRFS_PROGS_STATUS_DIR.into(),
            ],
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
            watched_pids: self.watched_pids.clone(),
            gpus,
            smart: self.smart.read(),
            power: PowerStats {
                gpus_w,
                ..Default::default()
            },
            ..Default::default()
        };

        #[cfg(target_os = "linux")]
        self.collect_linux(&mut snapshot, now);
        #[cfg(not(target_os = "linux"))]
        self.collect_portable(&mut snapshot, elapsed);

        self.fill.annotate(&mut snapshot.filesystems, now);
        if !(self.watched_names.is_empty() && self.name_matches.is_empty()) {
            self.name_matches =
                report_name_matches(&self.watched_names, &self.name_matches, &mut snapshot);
        }
        let power = &mut snapshot.power;
        power.total_w = sum_present([power.cpu_or_soc_w().1, power.gpus_w]);
        snapshot
    }

    /// Adds what `sysinfo` offers beyond the basics on non-Linux platforms:
    /// per-volume I/O, temperatures and (on Windows) link speeds.
    #[cfg(not(target_os = "linux"))]
    fn collect_portable(&mut self, snapshot: &mut Snapshot, elapsed: Option<f64>) {
        snapshot.disks = self.system.volume_io(elapsed);
        let (chips, cpu_temperatures) = self.system.components();
        snapshot.sensors = chips;
        snapshot.cpu.temperatures = cpu_temperatures;
        #[cfg(windows)]
        {
            crate::windows::annotate_links(&mut snapshot.networks);
            // LibreHardwareMonitor, when running, knows far more than the
            // ACPI thermal zones sysinfo can see.
            match self.lhm.read(Instant::now()) {
                Some(lhm) => {
                    if !lhm.chips.is_empty() {
                        snapshot.sensors = lhm.chips;
                    }
                    if !lhm.cpu_temperatures.is_empty() {
                        snapshot.cpu.temperatures = lhm.cpu_temperatures;
                    }
                    snapshot.power.cpu_package_w = lhm.cpu_package_w;
                    if !lhm.smart.is_empty() {
                        snapshot.smart = lhm.smart;
                    }
                }
                None => snapshot.lhm_error = self.lhm.problem().map(str::to_owned),
            }
        }
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
        snapshot.memory.oom_kills =
            self.oom
                .sample(&self.roots, &snapshot.processes, snapshot.timestamp_ms);
        snapshot.power.cpu_package_w = self.rapl.sample(&self.roots, now);
        // The APU's amdgpu power covers the whole SoC (see PowerStats).
        // Discrete AMD GPUs are not reported yet.
        snapshot.power.igpu_w = sum_present(
            chips
                .iter()
                .filter(|c| c.is_apu)
                .flat_map(|c| c.power_w.iter().map(|(_, w)| Some(*w))),
        );
        snapshot.power.battery = linux::battery::read_battery(&self.roots);

        // AMD and Intel GPUs after NVIDIA's; discrete ones add to GPU power.
        let first = snapshot.gpus.len() as u32;
        snapshot.gpus.extend(self.drm_gpus.read(&self.roots, first));
        snapshot.power.gpus_w = sum_present(snapshot.gpus.iter().map(|g| g.power_w));

        let mut disks = self.diskstats.sample(&self.roots, now);
        for disk in &mut disks {
            disk.temperature_c = nvme_temperature(&chips, &disk.name);
        }
        snapshot.disks = disks;

        let mounts = linux::mounts::read_mounts(&self.roots);
        snapshot.raid.btrfs = linux::btrfs::read_filesystems(&self.roots, &mounts);
        for fs in &mut snapshot.raid.btrfs {
            fs.scrub = linux::btrfs::read_scrub(&self.scrub_dirs, &fs.fsid);
        }
        self.net_links
            .annotate(&self.roots, &mut snapshot.networks, now);
        snapshot.raid.md = linux::mdstat::read_arrays(&self.roots);
        snapshot.raid.zfs = linux::zfs::read_pools(&self.roots);
        snapshot.failed_units = self.failed_units.read(now);
        snapshot.pressure = linux::pressure::read_pressure(&self.roots);
        let release = std::fs::read_to_string(self.roots.proc.join("sys/kernel/osrelease"))
            .unwrap_or_default();
        snapshot.host.reboot_required = linux::reboot::reboot_required(
            std::path::Path::new("/run"),
            std::path::Path::new("/usr/lib/modules"),
            &release,
        );
        // sysinfo no longer lists threads (see SystemCollector::refresh).
        for process in &mut snapshot.processes {
            process.threads = linux::procstat::thread_count(&self.roots, process.pid);
        }
        self.containers
            .annotate(&self.roots, &mut snapshot.processes, now);

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

    fn signal_process(&mut self, pid: u32, signal: ProcessSignal) -> Result<(), SourceError> {
        send_signal(pid, signal).map_err(|source| SourceError::Signal { pid, source })
    }

    /// With a limit, only the top processes are shown (overview), which do
    /// not include per-process GPU utilization: skip that NVML query.
    fn set_process_limit(&mut self, limit: Option<usize>) {
        self.nvml.set_process_utilization(limit.is_none());
    }

    fn watch_processes(&mut self, pids: Vec<u32>) {
        self.watched_pids = pids;
    }

    fn watch_process_names(&mut self, names: Vec<String>) {
        self.watched_names = names;
    }
}

/// Adds the processes of `snapshot` matching `names` (see
/// [`DataSource::watch_process_names`]) to its `watched_pids`, and once
/// more those of `previous` (last time's matches) that are gone, so the
/// client sees them exit. Returns this time's matches.
fn report_name_matches(names: &[String], previous: &[u32], snapshot: &mut Snapshot) -> Vec<u32> {
    let matches: Vec<u32> = snapshot
        .processes
        .iter()
        .filter(|p| {
            !p.kernel_thread
                && names
                    .iter()
                    .any(|name| crate::view::process_matches(p, name))
        })
        .map(|p| p.pid)
        .collect();
    let gone = previous
        .iter()
        .filter(|pid| !snapshot.processes.iter().any(|p| p.pid == **pid));
    for pid in matches.iter().chain(gone) {
        if !snapshot.watched_pids.contains(pid) {
            snapshot.watched_pids.push(*pid);
        }
    }
    matches
}

/// Sends a signal to a local process.
#[cfg(unix)]
fn send_signal(pid: u32, signal: ProcessSignal) -> std::io::Result<()> {
    // pid 0 and negative values address process groups in kill(2); never
    // let a bad pid turn into "signal everything".
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let sig = match signal {
        ProcessSignal::Terminate => libc::SIGTERM,
        ProcessSignal::Kill => libc::SIGKILL,
    };
    // SAFETY: kill(2) has no memory-safety preconditions.
    if unsafe { libc::kill(pid, sig) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
use crate::windows::send_signal;

/// Sends a signal to a local process (not implemented on this platform yet).
#[cfg(not(any(unix, windows)))]
fn send_signal(_pid: u32, _signal: ProcessSignal) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
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
        device_matches_disk(controller, disk).then_some(())?;
        // The first channel is the "Composite" temperature the drive reports.
        c.chip.temperatures.first().map(|t| t.celsius)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::ProcessInfo;

    #[test]
    fn named_processes_are_reported_until_seen_gone() {
        let process = |pid, name: &str, command: &str| ProcessInfo {
            pid,
            name: name.to_owned(),
            command: command.to_owned(),
            ..Default::default()
        };
        let names = vec!["train.py".to_owned()];
        let mut snapshot = Snapshot {
            processes: vec![
                process(10, "python3", "python3 jobs/train.py --epochs 3"),
                process(11, "vim", "vim notes.txt"),
            ],
            watched_pids: vec![99],
            ..Default::default()
        };
        let matches = report_name_matches(&names, &[], &mut snapshot);
        assert_eq!(matches, [10]);
        assert_eq!(snapshot.watched_pids, [99, 10]);

        // Gone: reported once more, then forgotten.
        let mut later = Snapshot {
            processes: vec![process(11, "vim", "vim notes.txt")],
            ..Default::default()
        };
        let matches = report_name_matches(&names, &matches, &mut later);
        assert!(matches.is_empty());
        assert_eq!(later.watched_pids, [10]);
        let mut after = Snapshot::default();
        assert!(report_name_matches(&names, &matches, &mut after).is_empty());
        assert!(after.watched_pids.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn signal_rejects_group_pids_and_reports_missing_process() {
        assert_eq!(
            send_signal(0, ProcessSignal::Terminate).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            send_signal(u32::MAX, ProcessSignal::Kill)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        // A child that has been reaped no longer exists: ESRCH.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let err = send_signal(pid, ProcessSignal::Terminate).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ESRCH));
    }

    #[cfg(unix)]
    #[test]
    fn signal_terminates_a_child_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        send_signal(child.id(), ProcessSignal::Kill).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

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
            is_apu: false,
        };
        let chips = vec![chip("nvme:nvme1", 40.0), chip("nvme:nvme10", 50.0)];
        assert_eq!(nvme_temperature(&chips, "nvme10n1"), Some(50.0));
        assert_eq!(nvme_temperature(&chips, "nvme1n1"), Some(40.0));
        assert_eq!(nvme_temperature(&chips, "sda"), None);
    }
}
