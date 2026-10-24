//! Front-end helpers shared by the TUI and the GUI: graph history, process
//! table sorting/filtering joined with GPU usage, and small naming helpers.
//!
//! Nothing here depends on a UI toolkit.

use std::collections::{HashMap, VecDeque};

use crate::snapshot::{ProcessInfo, Snapshot};
use crate::source::ProcessSignal;

/// Number of samples kept for graphs (wider than any realistic terminal in
/// braille dots: 2 samples per cell).
pub const HISTORY_LEN: usize = 1024;

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
