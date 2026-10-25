//! Front-end helpers shared by the TUI and the GUI: graph history, process
//! table sorting/filtering joined with GPU usage, and small naming helpers.
//!
//! Nothing here depends on a UI toolkit.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use crate::config::AlertsConfig;
use crate::snapshot::{ProcessInfo, Snapshot};
use crate::source::ProcessSignal;

/// Number of samples kept for graphs: over an hour at the default interval
/// of 1 s, and wider than any realistic terminal in braille dots (2 samples
/// per cell).
pub const HISTORY_LEN: usize = 4096;

/// Interface name prefixes skipped when choosing the default network
/// interface (container bridges and virtual links).
const VIRTUAL_IFACE_PREFIXES: &[&str] = &["lo", "docker", "veth", "br-", "virbr"];

/// Fixed-size history of samples, newest last.
#[derive(Debug, Default, Clone)]
pub struct History {
    samples: VecDeque<f64>,
}

impl History {
    /// Appends a sample, dropping the oldest when full.
    pub fn push(&mut self, value: f64) {
        if self.samples.len() == HISTORY_LEN {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    /// The newest `n` samples (or fewer), oldest first.
    pub fn last(&self, n: usize) -> Vec<f64> {
        let skip = self.samples.len().saturating_sub(n);
        self.samples.iter().skip(skip).copied().collect()
    }

    /// Largest of the newest `n` samples.
    pub fn max_of_last(&self, n: usize) -> f64 {
        self.last(n).into_iter().fold(0.0, f64::max)
    }

    /// Values for a graph `columns` wide, oldest first. [`GraphSpan::Fit`]
    /// gives the newest sample per column; a time span gives the samples of
    /// that span (one every `interval`) spread over the columns, averaged
    /// where several share a column and interpolated where there are fewer
    /// samples than columns. While the history is shorter than the span,
    /// fewer values come back, so the graph fills up from the right.
    pub fn graph(&self, columns: usize, span: GraphSpan, interval: Duration) -> Vec<f64> {
        let GraphSpan::Secs(secs) = span else {
            return self.last(columns);
        };
        let wanted = (secs as f64 / interval.as_secs_f64().max(0.001)).round() as usize;
        let samples = self.last(wanted.max(1));
        if columns == 0 || samples.is_empty() {
            return Vec::new();
        }
        // The share of the columns that the available samples cover.
        let shown = (columns as f64 * samples.len() as f64 / wanted.max(1) as f64).round() as usize;
        let shown = shown.clamp(1, columns);
        let per_column = samples.len() as f64 / shown as f64;
        if per_column < 1.0 {
            // Stretched: a straight line between neighbouring samples.
            let last = samples.len() - 1;
            return (0..shown)
                .map(|i| {
                    let at = ((i as f64 + 0.5) * per_column - 0.5).clamp(0.0, last as f64);
                    let (below, frac) = (at.floor() as usize, at.fract());
                    let above = (below + 1).min(last);
                    samples[below] * (1.0 - frac) + samples[above] * frac
                })
                .collect();
        }
        (0..shown)
            .map(|i| {
                let end = (((i + 1) as f64 * per_column) as usize).clamp(1, samples.len());
                let start = ((i as f64 * per_column) as usize).min(end - 1);
                let part = &samples[start..end];
                part.iter().sum::<f64>() / part.len() as f64
            })
            .collect()
    }
}

/// How much time the graphs cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphSpan {
    /// One sample per graph column, so the time covered grows with the
    /// width and the interval.
    #[default]
    Fit,
    /// The last this many seconds, spread over the width.
    Secs(u64),
}

impl GraphSpan {
    /// The choices [`GraphSpan::next`] steps through.
    pub const CHOICES: [GraphSpan; 5] = [
        GraphSpan::Fit,
        GraphSpan::Secs(60),
        GraphSpan::Secs(5 * 60),
        GraphSpan::Secs(15 * 60),
        GraphSpan::Secs(60 * 60),
    ];

    /// The choice after this one, back to the first after the last.
    pub fn next(self) -> Self {
        let index = Self::CHOICES
            .iter()
            .position(|c| *c == self)
            .map_or(0, |i| i + 1);
        Self::CHOICES[index % Self::CHOICES.len()]
    }

    /// Short name: "fit", "1m", "1h".
    pub fn label(self) -> String {
        match self {
            GraphSpan::Fit => "fit".to_owned(),
            GraphSpan::Secs(secs) if secs > 0 && secs % 3600 == 0 => format!("{}h", secs / 3600),
            GraphSpan::Secs(secs) if secs > 0 && secs % 60 == 0 => format!("{}m", secs / 60),
            GraphSpan::Secs(secs) => format!("{secs}s"),
        }
    }

    /// Seconds covered, 0 for [`GraphSpan::Fit`] (for storing the choice).
    pub fn as_secs(self) -> u64 {
        match self {
            GraphSpan::Fit => 0,
            GraphSpan::Secs(secs) => secs,
        }
    }

    /// The inverse of [`GraphSpan::as_secs`].
    pub fn from_secs(secs: u64) -> Self {
        if secs == 0 {
            GraphSpan::Fit
        } else {
            GraphSpan::Secs(secs)
        }
    }
}

/// Columns the process table can be sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Pid,
    Name,
    Threads,
    User,
    Memory,
    Cpu,
    Gpu,
    Vram,
}

impl SortKey {
    /// Sort keys in column order, used to step left/right.
    pub const ALL: [SortKey; 8] = [
        SortKey::Pid,
        SortKey::Name,
        SortKey::Threads,
        SortKey::User,
        SortKey::Memory,
        SortKey::Cpu,
        SortKey::Gpu,
        SortKey::Vram,
    ];

    /// Whether "descending" is the natural order (largest consumers first).
    pub fn descending_by_default(self) -> bool {
        !matches!(self, SortKey::Pid | SortKey::Name | SortKey::User)
    }
}

/// One row of the process table: the process plus its GPU usage.
#[derive(Debug, Clone)]
pub struct ProcessRow<'a> {
    pub info: &'a ProcessInfo,
    /// Highest per-GPU SM utilization of the process.
    pub gpu_percent: Option<f32>,
    /// VRAM used by the process across all GPUs.
    pub vram_bytes: Option<u64>,
}

/// Graph histories of one host, updated from each snapshot.
#[derive(Debug, Default, Clone)]
pub struct MetricHistory {
    /// Total CPU usage (%).
    pub cpu: History,
    /// Used memory (% of total).
    pub memory: History,
    /// Utilization (%) per GPU index.
    pub gpus: Vec<History>,
    /// Receive rate of `net_interface` (bytes/s).
    pub net_rx: History,
    /// Transmit rate of `net_interface` (bytes/s).
    pub net_tx: History,
    /// Interface whose traffic is recorded (`None`: chosen on the next snapshot).
    pub net_interface: Option<String>,
}

impl MetricHistory {
    /// Appends the values of a new snapshot.
    pub fn record(&mut self, snapshot: &Snapshot) {
        self.cpu.push(snapshot.cpu.total_percent as f64);
        let memory = &snapshot.memory;
        if memory.total_bytes > 0 {
            self.memory
                .push(memory.used_bytes as f64 / memory.total_bytes as f64 * 100.0);
        }
        self.gpus.resize_with(snapshot.gpus.len(), History::default);
        for (history, gpu) in self.gpus.iter_mut().zip(&snapshot.gpus) {
            history.push(gpu.utilization_percent.unwrap_or(0.0) as f64);
        }
        if self.net_interface.is_none() {
            self.net_interface = default_interface(snapshot);
        }
        let (rx, tx) = self
            .net_interface
            .as_deref()
            .and_then(|name| snapshot.networks.iter().find(|n| n.name == name))
            .map(|n| (n.rx_bytes_per_sec, n.tx_bytes_per_sec))
            .unwrap_or((0.0, 0.0));
        self.net_rx.push(rx);
        self.net_tx.push(tx);
    }

    /// Switches the recorded network interface, clearing its history.
    pub fn select_interface(&mut self, name: &str) {
        if self.net_interface.as_deref() != Some(name) {
            self.net_interface = Some(name.to_owned());
            // The old samples belong to another interface.
            self.net_rx = History::default();
            self.net_tx = History::default();
        }
    }

    /// Switches to the interface after the current one in `snapshot` and
    /// returns its name.
    pub fn next_interface(&mut self, snapshot: &Snapshot) -> Option<String> {
        let names: Vec<&str> = snapshot.networks.iter().map(|n| n.name.as_str()).collect();
        if names.is_empty() {
            return None;
        }
        let current = self
            .net_interface
            .as_deref()
            .and_then(|name| names.iter().position(|n| *n == name));
        let next = names[current.map_or(0, |i| (i + 1) % names.len())].to_owned();
        self.select_interface(&next);
        Some(next)
    }
}

/// How urgent an [`Alert`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth a look (e.g. a filesystem filling up, a RAID resync).
    Warning,
    /// Data at risk (errors, a missing device, a failing drive).
    Critical,
}

/// A condition on a host that needs attention, for overviews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub severity: Severity,
    pub text: String,
}

/// [`alerts_with`] the default thresholds.
pub fn alerts(snapshot: &Snapshot) -> Vec<Alert> {
    alerts_with(snapshot, &AlertsConfig::default())
}

/// Conditions in `snapshot` that need attention, most severe first:
/// storage errors and failing drives, degraded RAID, full filesystems, hot
/// CPUs, GPUs and drives, judged by `limits` (0 turns a threshold off).
pub fn alerts_with(snapshot: &Snapshot, limits: &AlertsConfig) -> Vec<Alert> {
    // A threshold of 0 (or less) is turned off.
    let reached = |value: f64, limit: f64| limit > 0.0 && value >= limit;
    let mut alerts = Vec::new();
    let mut add = |severity, text: String| alerts.push(Alert { severity, text });
    for fs in &snapshot.raid.btrfs {
        let name = fs
            .mount_points
            .first()
            .map_or(fs.fsid.as_str(), String::as_str);
        let missing = fs.device_stats.iter().filter(|d| d.missing).count();
        if missing > 0 {
            add(
                Severity::Critical,
                format!("btrfs {name}: {missing} device(s) missing"),
            );
        }
        let errors: u64 = fs.device_stats.iter().map(|d| d.total_errors()).sum();
        if errors > 0 {
            // Which drives: the one to replace.
            let devices: Vec<String> = fs
                .device_stats
                .iter()
                .filter(|d| d.total_errors() > 0)
                .map(|d| d.label())
                .collect();
            add(
                Severity::Critical,
                format!("btrfs {name}: {errors} I/O errors ({})", devices.join(", ")),
            );
        }
        if let Some(scrub) = &fs.scrub {
            if scrub.uncorrectable > 0 {
                add(
                    Severity::Critical,
                    format!(
                        "btrfs {name}: scrub found {} uncorrectable errors",
                        scrub.uncorrectable
                    ),
                );
            } else if scrub.errors > 0 {
                add(
                    Severity::Warning,
                    format!("btrfs {name}: scrub corrected {} errors", scrub.corrected),
                );
            }
        }
    }
    for md in &snapshot.raid.md {
        if md.active && md.is_degraded() {
            add(
                Severity::Critical,
                format!("{} ({}) degraded", md.name, md.level),
            );
        } else if !md.active {
            add(Severity::Warning, format!("{} inactive", md.name));
        }
        if let Some(sync) = &md.sync {
            add(
                Severity::Warning,
                format!("{} {} {:.0}%", md.name, sync.action, sync.percent),
            );
        }
    }
    for smart in &snapshot.smart {
        if let Some(error) = &smart.error {
            add(
                Severity::Warning,
                format!("{}: SMART unreadable ({error})", smart.device),
            );
        } else if smart.passed == Some(false) {
            add(
                Severity::Critical,
                format!("{}: SMART failed", smart.device),
            );
        } else if smart.needs_attention() {
            add(
                Severity::Critical,
                format!("{}: SMART warning", smart.device),
            );
        }
    }
    for fs in &snapshot.filesystems {
        let used = fs.total_bytes.saturating_sub(fs.available_bytes);
        let percent = used as f64 / fs.total_bytes.max(1) as f64 * 100.0;
        let name = fs
            .mount_points
            .first()
            .map_or(fs.device.as_str(), String::as_str);
        if reached(percent, limits.filesystem_critical_percent) {
            add(Severity::Critical, format!("{name} {percent:.0}% full"));
        } else if reached(percent, limits.filesystem_warning_percent) {
            add(Severity::Warning, format!("{name} {percent:.0}% full"));
        }
    }
    let hottest_cpu = snapshot
        .cpu
        .temperatures
        .iter()
        .map(|t| t.celsius)
        .fold(f32::NAN, f32::max);
    if reached(hottest_cpu as f64, limits.cpu_warning_celsius as f64) {
        add(Severity::Warning, format!("CPU at {hottest_cpu:.0}°C"));
    }
    for gpu in &snapshot.gpus {
        if let Some(t) = gpu
            .temperature_c
            .filter(|t| reached(*t as f64, limits.gpu_warning_celsius as f64))
        {
            add(Severity::Warning, format!("GPU {} at {t:.0}°C", gpu.index));
        }
    }
    for (device, celsius) in drive_temperatures(snapshot) {
        if reached(celsius as f64, limits.drive_warning_celsius as f64) {
            add(Severity::Warning, format!("{device} at {celsius:.0}°C"));
        }
    }
    // Stable sort keeps the order above within each severity.
    alerts.sort_by_key(|a| std::cmp::Reverse(a.severity));
    alerts
}

/// (device, °C) of each drive: SMART results, then NVMe drives without
/// them from their hwmon sensor ("Composite", the drive's own summary).
fn drive_temperatures(snapshot: &Snapshot) -> Vec<(String, f32)> {
    let mut drives: Vec<(String, f32)> = snapshot
        .smart
        .iter()
        .filter_map(|s| Some((s.device.clone(), s.temperature_c?)))
        .collect();
    for chip in snapshot.sensors.iter().filter(|c| c.name == "nvme") {
        // The chip key is "nvme:<device>".
        let device = chip.key.split_once(':').map_or(chip.key.as_str(), |k| k.1);
        if drives.iter().any(|(d, _)| d == device) {
            continue;
        }
        let composite = chip
            .temperatures
            .iter()
            .find(|t| t.label == "Composite")
            .or(chip.temperatures.first());
        if let Some(t) = composite {
            drives.push((device.to_owned(), t.celsius));
        }
    }
    drives
}

/// How the process table is filtered and sorted.
#[derive(Debug, Clone, Copy)]
pub struct ProcessQuery<'q> {
    /// Column to sort by.
    pub sort_key: SortKey,
    /// Largest first when true.
    pub descending: bool,
    /// Case-insensitive substring of name, command or user, or an exact pid.
    pub filter: &'q str,
    /// Include kernel threads (`kworker` and friends).
    pub show_kernel_threads: bool,
}

/// Processes of `snapshot` after filtering and sorting, joined with GPU usage.
pub fn process_rows<'a>(snapshot: &'a Snapshot, query: &ProcessQuery<'_>) -> Vec<ProcessRow<'a>> {
    // pid -> (max SM %, total VRAM) across GPUs.
    let mut gpu_usage: HashMap<u32, (Option<f32>, Option<u64>)> = HashMap::new();
    for gpu in &snapshot.gpus {
        for p in &gpu.processes {
            let entry = gpu_usage.entry(p.pid).or_default();
            if let Some(sm) = p.sm_percent {
                entry.0 = Some(entry.0.map_or(sm, |v| v.max(sm)));
            }
            if let Some(mem) = p.memory_bytes {
                entry.1 = Some(entry.1.unwrap_or(0) + mem);
            }
        }
    }

    let filter = query.filter.to_lowercase();
    let mut rows: Vec<ProcessRow<'a>> = snapshot
        .processes
        .iter()
        .filter(|p| query.show_kernel_threads || !p.kernel_thread)
        .filter(|p| {
            filter.is_empty()
                || p.name.to_lowercase().contains(&filter)
                || p.command.to_lowercase().contains(&filter)
                || p.user.to_lowercase().contains(&filter)
                || p.pid.to_string() == filter
        })
        .map(|info| {
            let (gpu_percent, vram_bytes) = gpu_usage.get(&info.pid).copied().unwrap_or_default();
            ProcessRow {
                info,
                gpu_percent,
                vram_bytes,
            }
        })
        .collect();

    let key = query.sort_key;
    rows.sort_by(|a, b| {
        let ordering = match key {
            SortKey::Pid => a.info.pid.cmp(&b.info.pid),
            SortKey::Name => a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase()),
            SortKey::Threads => a.info.threads.cmp(&b.info.threads),
            SortKey::User => a.info.user.cmp(&b.info.user),
            SortKey::Memory => a.info.memory_bytes.cmp(&b.info.memory_bytes),
            SortKey::Cpu => a.info.cpu_percent.total_cmp(&b.info.cpu_percent),
            SortKey::Gpu => a
                .gpu_percent
                .unwrap_or(-1.0)
                .total_cmp(&b.gpu_percent.unwrap_or(-1.0)),
            SortKey::Vram => a.vram_bytes.cmp(&b.vram_bytes),
        }
        // Stable tie-breaker so rows do not jump between frames.
        .then(a.info.pid.cmp(&b.info.pid));
        if query.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}

/// Shown where the CPU temperatures go when LibreHardwareMonitor cannot be
/// read (Windows; see [`Snapshot::lhm_error`]).
pub const LHM_HINT: &str = "temperatures need LibreHardwareMonitor";

/// Explanation behind [`LHM_HINT`], given the reason it could not be read.
pub fn lhm_help(error: &str) -> String {
    format!(
        "Windows needs LibreHardwareMonitor for temperatures, fans and CPU power.\n\
         Start it and turn on Options → Remote Web Server → Run.\n\
         ([lhm] url = \"\" in the configuration hides this note.)\n\n\
         {error}"
    )
}

/// Name of a signal as shown to the user.
pub fn signal_name(signal: ProcessSignal) -> &'static str {
    match signal {
        ProcessSignal::Terminate => "SIGTERM",
        ProcessSignal::Kill => "SIGKILL",
    }
}

/// Returns true for container bridges, veth pairs and loopback, which are
/// skipped when choosing and listing interfaces.
pub fn is_virtual_interface(name: &str) -> bool {
    VIRTUAL_IFACE_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Picks the physical interface with the most traffic so far.
pub fn default_interface(snapshot: &Snapshot) -> Option<String> {
    snapshot
        .networks
        .iter()
        .filter(|n| !is_virtual_interface(&n.name))
        .max_by_key(|n| n.rx_total_bytes + n.tx_total_bytes)
        .or_else(|| snapshot.networks.first())
        .map(|n| n.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{GpuProcess, GpuStats, NetworkStats};

    fn process(pid: u32, name: &str, cpu: f32, kernel: bool) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_owned(),
            cpu_percent: cpu,
            kernel_thread: kernel,
            ..Default::default()
        }
    }

    fn query(sort_key: SortKey) -> ProcessQuery<'static> {
        ProcessQuery {
            sort_key,
            descending: sort_key.descending_by_default(),
            filter: "",
            show_kernel_threads: false,
        }
    }

    #[test]
    fn history_keeps_newest_samples() {
        let mut history = History::default();
        for i in 0..(HISTORY_LEN + 5) {
            history.push(i as f64);
        }
        assert_eq!(
            history.last(2),
            vec![(HISTORY_LEN + 3) as f64, (HISTORY_LEN + 4) as f64]
        );
        assert_eq!(history.last(HISTORY_LEN * 2).len(), HISTORY_LEN);
        assert_eq!(history.max_of_last(3), (HISTORY_LEN + 4) as f64);
    }

    #[test]
    fn graph_spans_spread_samples_over_the_width() {
        let second = Duration::from_secs(1);
        let mut history = History::default();
        for i in 0..120 {
            history.push(i as f64);
        }
        // Fit: one sample per column, the newest ones.
        assert_eq!(
            history.graph(3, GraphSpan::Fit, second),
            vec![117.0, 118.0, 119.0]
        );
        // A minute on 30 columns: two samples averaged per column.
        let minute = history.graph(30, GraphSpan::Secs(60), second);
        assert_eq!(minute.len(), 30);
        assert_eq!(minute[0], 60.5);
        assert_eq!(minute[29], 118.5);
        // Fewer samples than columns: interpolated between them.
        let stretched = history.graph(120, GraphSpan::Secs(60), Duration::from_secs(2));
        assert_eq!(stretched.len(), 120);
        assert_eq!(&stretched[..2], &[90.0, 90.0]);
        assert_eq!(&stretched[2..4], &[90.125, 90.375]);
        assert_eq!(stretched[119], 119.0);
        assert!(stretched.windows(2).all(|w| w[0] <= w[1]));
        // An hour with two minutes recorded fills the right 1/30.
        let hour = history.graph(300, GraphSpan::Secs(3600), second);
        assert_eq!(hour.len(), 10);
        assert_eq!(hour[0], 5.5);
        assert!(
            History::default()
                .graph(10, GraphSpan::Secs(60), second)
                .is_empty()
        );
    }

    #[test]
    fn graph_span_choices_cycle() {
        let labels: Vec<String> = std::iter::successors(Some(GraphSpan::Fit), |s| Some(s.next()))
            .take(6)
            .map(GraphSpan::label)
            .collect();
        assert_eq!(labels, ["fit", "1m", "5m", "15m", "1h", "fit"]);
        assert_eq!(
            GraphSpan::from_secs(GraphSpan::Secs(300).as_secs()),
            GraphSpan::Secs(300)
        );
        assert_eq!(GraphSpan::from_secs(0), GraphSpan::Fit);
        // A stored span that is not a choice starts over.
        assert_eq!(GraphSpan::Secs(7).next(), GraphSpan::Fit);
    }

    #[test]
    fn rows_filter_sort_and_join_gpu_usage() {
        let snapshot = Snapshot {
            processes: vec![
                process(10, "bash", 5.0, false),
                process(2, "kworker/0:1", 50.0, true),
                process(30, "python", 1.0, false),
            ],
            gpus: vec![GpuStats {
                processes: vec![GpuProcess {
                    pid: 30,
                    memory_bytes: Some(1 << 30),
                    sm_percent: Some(70.0),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let pids = |q: &ProcessQuery| {
            process_rows(&snapshot, q)
                .iter()
                .map(|r| r.info.pid)
                .collect::<Vec<_>>()
        };
        assert_eq!(pids(&query(SortKey::Cpu)), vec![10, 30]);
        assert_eq!(pids(&query(SortKey::Vram)), vec![30, 10]);
        assert_eq!(
            pids(&ProcessQuery {
                show_kernel_threads: true,
                ..query(SortKey::Cpu)
            }),
            vec![2, 10, 30]
        );
        assert_eq!(
            pids(&ProcessQuery {
                filter: "PYT",
                ..query(SortKey::Pid)
            }),
            vec![30]
        );
        let rows = process_rows(&snapshot, &query(SortKey::Pid));
        assert_eq!(rows[1].vram_bytes, Some(1 << 30));
        assert_eq!(rows[1].gpu_percent, Some(70.0));
    }

    #[test]
    fn metric_history_records_and_switches_interfaces() {
        let snapshot = Snapshot {
            networks: vec![
                NetworkStats {
                    name: "enp9s0".to_owned(),
                    rx_total_bytes: 10,
                    rx_bytes_per_sec: 100.0,
                    ..Default::default()
                },
                NetworkStats {
                    name: "wlp7s0".to_owned(),
                    tx_bytes_per_sec: 5.0,
                    ..Default::default()
                },
            ],
            gpus: vec![GpuStats {
                utilization_percent: Some(40.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut history = MetricHistory::default();
        history.record(&snapshot);
        assert_eq!(history.net_interface.as_deref(), Some("enp9s0"));
        assert_eq!(history.net_rx.last(1), vec![100.0]);
        assert_eq!(history.gpus[0].last(1), vec![40.0]);
        assert_eq!(history.next_interface(&snapshot).as_deref(), Some("wlp7s0"));
        // Switching clears the samples of the previous interface.
        assert!(history.net_rx.last(10).is_empty());
        history.record(&snapshot);
        assert_eq!(history.net_tx.last(1), vec![5.0]);
    }

    #[test]
    fn alerts_flag_storage_problems_first() {
        use crate::snapshot::{
            BtrfsDeviceStats, BtrfsFilesystem, FilesystemUsage, MdArray, RaidStats, SmartDevice,
        };
        let snapshot = Snapshot {
            filesystems: vec![FilesystemUsage {
                mount_points: vec!["/data".to_owned()],
                total_bytes: 100,
                available_bytes: 8,
                ..Default::default()
            }],
            raid: RaidStats {
                btrfs: vec![BtrfsFilesystem {
                    mount_points: vec!["/".to_owned()],
                    device_stats: vec![BtrfsDeviceStats {
                        devid: 1,
                        corruption_errs: 2,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                md: vec![MdArray {
                    name: "md0".to_owned(),
                    active: true,
                    level: "raid1".to_owned(),
                    expected_devices: Some(2),
                    working_devices: Some(1),
                    ..Default::default()
                }],
            },
            smart: vec![SmartDevice {
                device: "sda".to_owned(),
                passed: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        };
        let alerts = alerts(&snapshot);
        let texts: Vec<&str> = alerts.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "btrfs /: 2 I/O errors (devid 1)",
                "md0 (raid1) degraded",
                "/data 92% full"
            ]
        );
        assert_eq!(alerts[0].severity, Severity::Critical);
        assert_eq!(alerts[2].severity, Severity::Warning);
        assert!(super::alerts(&Snapshot::default()).is_empty());
    }

    #[test]
    fn alert_thresholds_come_from_the_configuration() {
        use crate::snapshot::{FilesystemUsage, GpuStats, SensorChip, SmartDevice, Temperature};
        let temp = |label: &str, celsius| Temperature {
            label: label.to_owned(),
            celsius,
            ..Default::default()
        };
        let mut snapshot = Snapshot {
            filesystems: vec![FilesystemUsage {
                mount_points: vec!["/data".to_owned()],
                total_bytes: 100,
                available_bytes: 8,
                ..Default::default()
            }],
            gpus: vec![GpuStats {
                index: 0,
                temperature_c: Some(80.0),
                ..Default::default()
            }],
            smart: vec![SmartDevice {
                device: "nvme0".to_owned(),
                temperature_c: Some(72.0),
                ..Default::default()
            }],
            // nvme0 also has SMART (reported once); nvme1 only its sensor.
            sensors: vec![
                SensorChip {
                    key: "nvme:nvme0".to_owned(),
                    name: "nvme".to_owned(),
                    temperatures: vec![temp("Composite", 71.0)],
                    ..Default::default()
                },
                SensorChip {
                    key: "nvme:nvme1".to_owned(),
                    name: "nvme".to_owned(),
                    temperatures: vec![temp("Sensor 1", 90.0), temp("Composite", 74.0)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        snapshot.cpu.temperatures = vec![temp("Tctl", 97.0), temp("Tccd1", 90.0)];
        let texts = |limits: &AlertsConfig| -> Vec<String> {
            alerts_with(&snapshot, limits)
                .into_iter()
                .map(|a| a.text)
                .collect()
        };
        assert_eq!(
            texts(&AlertsConfig::default()),
            [
                "/data 92% full",
                "CPU at 97°C",
                "nvme0 at 72°C",
                "nvme1 at 74°C"
            ]
        );
        let custom = AlertsConfig {
            filesystem_warning_percent: 0.0,
            cpu_warning_celsius: 0.0,
            gpu_warning_celsius: 75.0,
            drive_warning_celsius: 73.0,
            ..Default::default()
        };
        assert_eq!(texts(&custom), ["GPU 0 at 80°C", "nvme1 at 74°C"]);
        let critical = AlertsConfig {
            filesystem_critical_percent: 90.0,
            ..custom
        };
        assert_eq!(
            alerts_with(&snapshot, &critical)[0],
            Alert {
                severity: Severity::Critical,
                text: "/data 92% full".to_owned()
            }
        );
    }

    #[test]
    fn interface_helpers() {
        assert!(is_virtual_interface("veth5a74356"));
        assert!(!is_virtual_interface("enp9s0"));
        let snapshot = Snapshot {
            networks: vec![
                NetworkStats {
                    name: "docker0".to_owned(),
                    rx_total_bytes: 1 << 40,
                    ..Default::default()
                },
                NetworkStats {
                    name: "enp9s0".to_owned(),
                    rx_total_bytes: 1 << 30,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(default_interface(&snapshot).as_deref(), Some("enp9s0"));
        assert_eq!(signal_name(ProcessSignal::Kill), "SIGKILL");
    }
}
