//! Front-end helpers shared by the TUI and the GUI: graph history, process
//! table sorting/filtering joined with GPU usage, and small naming helpers.
//!
//! Nothing here depends on a UI toolkit.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};

use crate::config::AlertsConfig;
use crate::format;
use crate::snapshot::{ProcessInfo, Snapshot};
use crate::source::ProcessSignal;

/// Number of samples kept for graphs: over an hour at the default interval
/// of 1 s, and wider than any realistic terminal in braille dots (2 samples
/// per cell).
pub const HISTORY_LEN: usize = 4096;

/// Interface name prefixes skipped when choosing the default network
/// interface (container bridges and virtual links).
const VIRTUAL_IFACE_PREFIXES: &[&str] = &["lo", "docker", "veth", "br-", "virbr"];

/// Samples averaged into one point of the coarse history (a minute at the
/// default interval of 1 s).
pub const COARSE_FACTOR: usize = 60;
/// Points kept in the coarse history (a day at the default interval).
pub const COARSE_LEN: usize = 1440;

/// Fixed-size history of samples, newest last, plus a coarse history of
/// averages for long time spans.
#[derive(Debug, Default, Clone)]
pub struct History {
    samples: VecDeque<f64>,
    coarse: VecDeque<f64>,
    /// Sum and count of the samples not yet averaged into `coarse`.
    pending: (f64, usize),
}

impl History {
    /// Appends a sample, dropping the oldest when full.
    pub fn push(&mut self, value: f64) {
        if self.samples.len() == HISTORY_LEN {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
        self.pending.0 += value;
        self.pending.1 += 1;
        if self.pending.1 == COARSE_FACTOR {
            if self.coarse.len() == COARSE_LEN {
                self.coarse.pop_front();
            }
            self.coarse.push_back(self.pending.0 / COARSE_FACTOR as f64);
            self.pending = (0.0, 0);
        }
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
        let wanted = ((secs as f64 / interval.as_secs_f64().max(0.001)).round() as usize).max(1);
        if wanted <= HISTORY_LEN {
            spread(&self.last(wanted), wanted, columns)
        } else {
            // Longer than the samples kept: from the averages.
            let wanted = wanted.div_ceil(COARSE_FACTOR);
            let skip = self.coarse.len().saturating_sub(wanted);
            let points: Vec<f64> = self.coarse.iter().skip(skip).copied().collect();
            spread(&points, wanted, columns)
        }
    }
}

/// `samples` (the newest of `wanted`) spread over `columns`: averaged where
/// several share a column, interpolated where there are fewer samples than
/// columns, and over a proportional share of the columns at the right while
/// fewer than `wanted` exist.
fn spread(samples: &[f64], wanted: usize, columns: usize) -> Vec<f64> {
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
    pub const CHOICES: [GraphSpan; 7] = [
        GraphSpan::Fit,
        GraphSpan::Secs(60),
        GraphSpan::Secs(5 * 60),
        GraphSpan::Secs(15 * 60),
        GraphSpan::Secs(60 * 60),
        GraphSpan::Secs(6 * 60 * 60),
        GraphSpan::Secs(24 * 60 * 60),
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
    /// Disk reads plus writes.
    Disk,
    Gpu,
    Vram,
}

impl SortKey {
    /// Sort keys in column order, used to step left/right.
    pub const ALL: [SortKey; 9] = [
        SortKey::Pid,
        SortKey::Name,
        SortKey::Threads,
        SortKey::User,
        SortKey::Memory,
        SortKey::Cpu,
        SortKey::Disk,
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
    /// In the tree view, the lines leading to the process ("│ ├ "), to be
    /// put before its name; empty otherwise.
    pub tree_prefix: String,
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

/// Identifies an alert across samples: its severity and text with numbers
/// masked, so "3 I/O errors" becoming "4 I/O errors" or a temperature
/// changing does not count as a new alert.
pub fn alert_key(alert: &Alert) -> String {
    let mut key = format!("{:?}:", alert.severity);
    let mut in_number = false;
    for c in alert.text.chars() {
        if c.is_ascii_digit() || (in_number && c == '.') {
            if !in_number {
                key.push('#');
            }
            in_number = true;
        } else {
            key.push(c);
            in_number = false;
        }
    }
    key
}

/// Whether an [`AlertEvent`] is an alert appearing or going away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertChange {
    Raised,
    Cleared,
}

/// One entry of the [`AlertLog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertEvent {
    /// When the change was seen, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub host: String,
    pub change: AlertChange,
    /// The alert as raised, or as last seen before it cleared.
    pub alert: Alert,
}

/// Events kept by an [`AlertLog`].
const ALERT_LOG_LEN: usize = 500;

/// The alerts of every host as they appear and clear, so that what
/// happened while nobody watched can be looked up later.
#[derive(Debug, Default)]
pub struct AlertLog {
    /// Newest last.
    events: VecDeque<AlertEvent>,
    /// Each host's alerts at its previous update.
    active: HashMap<String, Vec<Alert>>,
    /// Alerts raised since [`AlertLog::mark_read`], and the worst of them.
    unread: (usize, Option<Severity>),
}

impl AlertLog {
    /// Records how the alerts of `host` changed since its previous update.
    pub fn update(&mut self, host: &str, alerts: &[Alert], at_ms: u64) {
        let previous = self.active.remove(host).unwrap_or_default();
        let keys = |list: &[Alert]| -> HashSet<String> { list.iter().map(alert_key).collect() };
        let (before, now) = (keys(&previous), keys(alerts));
        let raised = alerts.iter().filter(|a| !before.contains(&alert_key(a)));
        let cleared = previous.iter().filter(|a| !now.contains(&alert_key(a)));
        let mut changes: Vec<(AlertChange, &Alert)> = raised
            .map(|a| (AlertChange::Raised, a))
            .chain(cleared.map(|a| (AlertChange::Cleared, a)))
            .collect();
        // Cleared first, so that a changed alert reads "went away, came back".
        changes.sort_by_key(|(change, _)| *change == AlertChange::Raised);
        for (change, alert) in changes {
            if change == AlertChange::Raised {
                self.unread.0 += 1;
                self.unread.1 = self.unread.1.max(Some(alert.severity));
            }
            if self.events.len() == ALERT_LOG_LEN {
                self.events.pop_front();
            }
            self.events.push_back(AlertEvent {
                at_ms,
                host: host.to_owned(),
                change,
                alert: alert.clone(),
            });
        }
        self.active.insert(host.to_owned(), alerts.to_vec());
    }

    /// All events, oldest first.
    pub fn events(&self) -> impl DoubleEndedIterator<Item = &AlertEvent> {
        self.events.iter()
    }

    /// Alerts raised since the log was last read, and the most severe one.
    pub fn unread(&self) -> (usize, Option<Severity>) {
        self.unread
    }

    /// Marks every event as read.
    pub fn mark_read(&mut self) {
        self.unread = (0, None);
    }
}

/// How long it has been since `since` (e.g. a host's last data) and when
/// that was, in local time: "5m (since 14:03)", "2d03h (since Tue 14:03)".
pub fn since_note(since: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(since).map_or(0, |d| d.as_secs());
    let span = match secs {
        0..60 => format!("{secs}s"),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h{:02}m", secs / 3_600, secs % 3_600 / 60),
        _ => format!("{}d{:02}h", secs / 86_400, secs % 86_400 / 3_600),
    };
    let at: DateTime<Local> = since.into();
    let today: DateTime<Local> = now.into();
    let clock = if at.date_naive() == today.date_naive() {
        at.format("%H:%M")
    } else if secs < 6 * 86_400 {
        at.format("%a %H:%M")
    } else {
        at.format("%b %d %H:%M")
    };
    format!("{span} (since {clock})")
}

/// "agent 0.8.2" when `snapshot` comes from a rigtop of another version
/// than this one (an agent not updated yet), else `None`.
pub fn version_note(snapshot: &Snapshot) -> Option<String> {
    let version = &snapshot.host.rigtop_version;
    (!version.is_empty() && version != env!("CARGO_PKG_VERSION"))
        .then(|| format!("agent {version}"))
}

/// Hover text explaining a [`version_note`].
pub fn version_hint(snapshot: &Snapshot) -> String {
    format!(
        "this host runs rigtop {}, this machine {}: install the same version on both \
         for every reading",
        snapshot.host.rigtop_version,
        env!("CARGO_PKG_VERSION")
    )
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
    for pool in &snapshot.raid.zfs {
        if pool.is_failing() {
            add(
                Severity::Critical,
                format!("zfs {} {}", pool.name, pool.state),
            );
        } else if !pool.is_online() {
            add(
                Severity::Warning,
                format!("zfs {} {}", pool.name, pool.state),
            );
        }
    }
    for unit in &snapshot.failed_units {
        if !limits.ignored_units.contains(unit) {
            add(Severity::Warning, format!("systemd: {unit} failed"));
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
    /// Children under their parents, siblings sorted by `sort_key`. A
    /// filter keeps the ancestors of what it matches.
    pub tree: bool,
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
    let matches = |p: &ProcessInfo| {
        filter.is_empty()
            || p.name.to_lowercase().contains(&filter)
            || p.command.to_lowercase().contains(&filter)
            || p.user.to_lowercase().contains(&filter)
            || p.pid.to_string() == filter
    };
    let candidates = snapshot
        .processes
        .iter()
        .filter(|p| query.show_kernel_threads || !p.kernel_thread);
    let shown: Vec<&ProcessInfo> = if query.tree && !filter.is_empty() {
        with_ancestors(candidates.collect(), matches)
    } else {
        candidates.filter(|p| matches(p)).collect()
    };
    let mut rows: Vec<ProcessRow<'a>> = shown
        .into_iter()
        .map(|info| {
            let (gpu_percent, vram_bytes) = gpu_usage.get(&info.pid).copied().unwrap_or_default();
            ProcessRow {
                info,
                gpu_percent,
                vram_bytes,
                tree_prefix: String::new(),
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
            SortKey::Disk => disk_rate(a.info).total_cmp(&disk_rate(b.info)),
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
    if query.tree { into_tree(rows) } else { rows }
}

/// The processes `matches` accepts, plus their ancestors among `processes`
/// (so a filtered tree keeps its shape), in the order of `processes`.
fn with_ancestors(
    processes: Vec<&ProcessInfo>,
    matches: impl Fn(&ProcessInfo) -> bool,
) -> Vec<&ProcessInfo> {
    let by_pid: HashMap<u32, &ProcessInfo> = processes.iter().map(|p| (p.pid, *p)).collect();
    let mut keep: HashSet<u32> = HashSet::new();
    for process in processes.iter().filter(|p| matches(p)) {
        let mut current = Some(*process);
        // Stops at a process already kept, which also ends parent loops.
        while let Some(p) = current
            && keep.insert(p.pid)
        {
            current = p.parent_pid.and_then(|pid| by_pid.get(&pid).copied());
        }
    }
    processes
        .into_iter()
        .filter(|p| keep.contains(&p.pid))
        .collect()
}

/// Reorders sorted `rows` depth-first under their parents, keeping the
/// sorted order among siblings, and fills in [`ProcessRow::tree_prefix`].
/// Processes whose parent is not among the rows are roots.
fn into_tree(rows: Vec<ProcessRow<'_>>) -> Vec<ProcessRow<'_>> {
    let index: HashMap<u32, usize> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| (row.info.pid, i))
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut roots = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        match row.info.parent_pid.and_then(|pid| index.get(&pid)) {
            Some(&parent) if parent != i => children[parent].push(i),
            _ => roots.push(i),
        }
    }
    let mut order: Vec<(usize, String)> = Vec::with_capacity(rows.len());
    let mut visited = vec![false; rows.len()];
    // Processes in a parent loop have no root; they start trees of their own.
    let starts: Vec<usize> = roots.into_iter().chain(0..rows.len()).collect();
    for start in starts {
        // (row, lines of the ancestors, whether it is its parent's last child)
        let mut stack: Vec<(usize, String, Option<bool>)> = vec![(start, String::new(), None)];
        while let Some((i, lead, last)) = stack.pop() {
            if std::mem::replace(&mut visited[i], true) {
                continue;
            }
            let (prefix, child_lead) = match last {
                None => (String::new(), String::new()),
                Some(true) => (format!("{lead}└ "), format!("{lead}  ")),
                Some(false) => (format!("{lead}├ "), format!("{lead}│ ")),
            };
            order.push((i, prefix));
            let kids = &children[i];
            for (k, &child) in kids.iter().enumerate().rev() {
                stack.push((child, child_lead.clone(), Some(k + 1 == kids.len())));
            }
        }
    }
    let mut rows: Vec<Option<ProcessRow<'_>>> = rows.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|(i, prefix)| {
            let mut row = rows[i].take()?;
            row.tree_prefix = prefix;
            Some(row)
        })
        .collect()
}

/// Disk reads plus writes of a process (bytes/s), the Disk column.
pub fn disk_rate(process: &ProcessInfo) -> f64 {
    process.read_bytes_per_sec + process.write_bytes_per_sec
}

/// The Disk column's text: the total rate, blank when idle (also where the
/// counters cannot be read, e.g. other users' processes on Linux).
pub fn disk_rate_text(process: &ProcessInfo) -> String {
    let rate = disk_rate(process);
    if rate > 0.0 {
        format::rate(rate)
    } else {
        String::new()
    }
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
            tree: false,
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
    fn long_spans_draw_from_the_averages() {
        let second = Duration::from_secs(1);
        let mut history = History::default();
        // Two hours: the first at 0, the second at 100.
        for i in 0..7200 {
            history.push(if i < 3600 { 0.0 } else { 100.0 });
        }
        // 6 hours with 2 recorded: the right third, half low and half high.
        let six = history.graph(90, GraphSpan::Secs(6 * 3600), second);
        assert_eq!(six.len(), 30);
        assert_eq!((six[0], six[29]), (0.0, 100.0));
        // An hour still comes from the samples themselves.
        let hour = history.graph(60, GraphSpan::Secs(3600), second);
        assert!(hour.iter().all(|v| *v == 100.0));
        // The averages stay bounded.
        for i in 0..(COARSE_LEN * COARSE_FACTOR + 500) {
            history.push(i as f64);
        }
        assert_eq!(history.coarse.len(), COARSE_LEN);
    }

    #[test]
    fn graph_span_choices_cycle() {
        let labels: Vec<String> = std::iter::successors(Some(GraphSpan::Fit), |s| Some(s.next()))
            .take(6)
            .map(GraphSpan::label)
            .collect();
        assert_eq!(labels, ["fit", "1m", "5m", "15m", "1h", "6h"]);
        assert_eq!(GraphSpan::Secs(86_400).label(), "24h");
        assert_eq!(GraphSpan::Secs(86_400).next(), GraphSpan::Fit);
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
    fn since_notes_say_how_long_and_since_when() {
        let now = SystemTime::now();
        let ago = |secs: u64| now - Duration::from_secs(secs);
        assert!(since_note(ago(42), now).starts_with("42s (since "));
        assert!(since_note(ago(5 * 60), now).starts_with("5m (since "));
        assert!(since_note(ago(3 * 3600 + 7 * 60), now).starts_with("3h07m (since "));
        assert!(since_note(ago(2 * 86_400 + 3 * 3600), now).starts_with("2d03h (since "));
        // A clock that moved backwards does not underflow.
        assert!(since_note(now + Duration::from_secs(5), now).starts_with("0s"));
    }

    #[test]
    fn tree_puts_children_under_parents() {
        let child = |pid: u32, parent: u32, name: &str, cpu: f32| ProcessInfo {
            parent_pid: Some(parent),
            ..process(pid, name, cpu, false)
        };
        let snapshot = Snapshot {
            processes: vec![
                process(1, "init", 0.0, false),
                child(10, 1, "sshd", 1.0),
                child(11, 10, "bash", 2.0),
                child(12, 11, "python", 50.0),
                child(20, 1, "cron", 3.0),
                // Parent not listed (e.g. not readable): a root of its own.
                child(30, 999, "orphan", 4.0),
                // A parent loop must not hang or drop the processes.
                child(40, 41, "loop-a", 0.0),
                child(41, 40, "loop-b", 0.0),
            ],
            ..Default::default()
        };
        let tree = |filter| {
            process_rows(
                &snapshot,
                &ProcessQuery {
                    filter,
                    tree: true,
                    ..query(SortKey::Cpu)
                },
            )
            .iter()
            .map(|r| format!("{}{}", r.tree_prefix, r.info.name))
            .collect::<Vec<_>>()
        };
        // Roots and siblings by CPU, largest first (ties: higher pid first).
        assert_eq!(
            tree(""),
            vec![
                "orphan",
                "init",
                "├ cron",
                "└ sshd",
                "  └ bash",
                "    └ python",
                "loop-b",
                "└ loop-a",
            ]
        );
        // A filter keeps the ancestors of what it matches.
        assert_eq!(
            tree("pyth"),
            vec!["init", "└ sshd", "  └ bash", "    └ python"]
        );
    }

    #[test]
    fn disk_column_sums_reads_and_writes() {
        let reader = ProcessInfo {
            read_bytes_per_sec: 3.0 * 1024.0 * 1024.0,
            ..process(10, "reader", 0.0, false)
        };
        let writer = ProcessInfo {
            read_bytes_per_sec: 1024.0 * 1024.0,
            write_bytes_per_sec: 4.0 * 1024.0 * 1024.0,
            ..process(20, "writer", 0.0, false)
        };
        let idle = process(30, "idle", 0.0, false);
        assert_eq!(disk_rate_text(&writer), "5.0M/s");
        assert_eq!(disk_rate_text(&idle), "");
        let snapshot = Snapshot {
            processes: vec![idle, reader, writer],
            ..Default::default()
        };
        let pids: Vec<u32> = process_rows(&snapshot, &query(SortKey::Disk))
            .iter()
            .map(|r| r.info.pid)
            .collect();
        assert_eq!(pids, vec![20, 10, 30]);
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
            ZfsPool,
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
                zfs: vec![
                    ZfsPool {
                        name: "tank".to_owned(),
                        state: "DEGRADED".to_owned(),
                    },
                    ZfsPool {
                        name: "cold".to_owned(),
                        state: "OFFLINE".to_owned(),
                    },
                    ZfsPool {
                        name: "fast".to_owned(),
                        state: "ONLINE".to_owned(),
                    },
                ],
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
                "zfs tank DEGRADED",
                "zfs cold OFFLINE",
                "/data 92% full"
            ]
        );
        assert_eq!(alerts[0].severity, Severity::Critical);
        assert_eq!(alerts[2].severity, Severity::Critical);
        assert_eq!(alerts[3].severity, Severity::Warning);
        assert!(super::alerts(&Snapshot::default()).is_empty());
    }

    #[test]
    fn failed_units_warn_unless_ignored() {
        let snapshot = Snapshot {
            failed_units: vec!["nginx.service".to_owned(), "wait-online.service".to_owned()],
            ..Default::default()
        };
        let limits = AlertsConfig {
            ignored_units: vec!["wait-online.service".to_owned()],
            ..Default::default()
        };
        let alerts = alerts_with(&snapshot, &limits);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].text, "systemd: nginx.service failed");
        assert_eq!(alerts[0].severity, Severity::Warning);
    }

    #[test]
    fn version_notes_only_for_other_versions() {
        let mut snapshot = Snapshot::default();
        assert_eq!(version_note(&snapshot), None);
        snapshot.host.rigtop_version = env!("CARGO_PKG_VERSION").to_owned();
        assert_eq!(version_note(&snapshot), None);
        snapshot.host.rigtop_version = "0.1.0".to_owned();
        assert_eq!(version_note(&snapshot).as_deref(), Some("agent 0.1.0"));
        assert!(version_hint(&snapshot).contains("rigtop 0.1.0"));
    }

    #[test]
    fn alert_log_records_alerts_appearing_and_clearing() {
        let alert = |severity, text: &str| Alert {
            severity,
            text: text.to_owned(),
        };
        let full = alert(Severity::Warning, "/data 91% full");
        let errors = alert(Severity::Critical, "btrfs /: 2 I/O errors (nvme1n1)");
        let mut log = AlertLog::default();
        log.update("nas", std::slice::from_ref(&full), 1_000);
        // Numbers changing is the same alert.
        log.update(
            "nas",
            &[alert(Severity::Warning, "/data 92% full"), errors.clone()],
            2_000,
        );
        log.update("zefat", &[], 2_000);
        assert_eq!(log.unread(), (2, Some(Severity::Critical)));
        log.mark_read();
        log.update("nas", std::slice::from_ref(&errors), 3_000);
        assert_eq!(log.unread(), (0, None));
        let summary: Vec<(u64, AlertChange, &str)> = log
            .events()
            .map(|e| (e.at_ms, e.change, e.alert.text.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (1_000, AlertChange::Raised, "/data 91% full"),
                (
                    2_000,
                    AlertChange::Raised,
                    "btrfs /: 2 I/O errors (nvme1n1)"
                ),
                (3_000, AlertChange::Cleared, "/data 92% full"),
            ]
        );
        for i in 0..(ALERT_LOG_LEN as u64 + 10) {
            log.update(
                "flappy",
                &[alert(Severity::Warning, if i % 2 == 0 { "a" } else { "b" })],
                i,
            );
        }
        assert_eq!(log.events().count(), ALERT_LOG_LEN);
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
