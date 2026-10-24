//! UI state: latest snapshot, history for graphs, process table state and
//! key handling. Kept free of drawing code so it can be unit tested.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;
use rigtop_core::snapshot::ProcessInfo;
use rigtop_core::{Config, ProcessSignal, Snapshot};

use super::worker::{Command, Event};

/// Number of samples kept for graphs (wider than any realistic terminal in
/// braille dots: 2 samples per cell).
const HISTORY_LEN: usize = 1024;

/// Rows moved per mouse wheel step in the process table.
const WHEEL_STEP: isize = 3;

/// How long a status message stays visible.
const STATUS_TTL: Duration = Duration::from_secs(5);

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
    fn descending_by_default(self) -> bool {
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

/// A pending confirmation before sending a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmSignal {
    pub pid: u32,
    pub name: String,
    pub signal: ProcessSignal,
}

/// Where the process table was drawn in the last frame, for mouse hit-testing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcTableHit {
    /// Screen row of the column headers.
    pub header_y: u16,
    /// Area of the data rows.
    pub rows: Rect,
    /// Horizontal extent `(x, width)` and sort key of each column.
    pub columns: Vec<(u16, u16, Option<SortKey>)>,
    /// Index of the first visible row.
    pub offset: usize,
}

/// Whole UI state.
pub struct App {
    pub config: Config,
    pub snapshot: Option<Snapshot>,
    pub cpu_history: History,
    pub memory_history: History,
    /// Utilization history per GPU index.
    pub gpu_history: Vec<History>,
    /// Receive/transmit history of the selected interface.
    pub net_rx_history: History,
    pub net_tx_history: History,
    /// Interface shown in the network box (`None`: choose automatically).
    pub net_interface: Option<String>,
    pub sort_key: SortKey,
    pub sort_descending: bool,
    pub show_kernel_threads: bool,
    pub filter: String,
    /// Whether keystrokes currently edit the filter.
    pub editing_filter: bool,
    /// Selected process, tracked by pid so the selection follows the
    /// process when the order changes.
    pub selected_pid: Option<u32>,
    pub table_state: TableState,
    pub confirm: Option<ConfirmSignal>,
    /// Process table geometry from the last frame.
    pub proc_hit: Option<ProcTableHit>,
    status: Option<(String, Instant)>,
    pub interval: Duration,
    pub should_quit: bool,
}

impl App {
    /// Creates the state with a configuration and the initial poll interval.
    pub fn new(config: Config, interval: Duration) -> Self {
        Self {
            config,
            snapshot: None,
            cpu_history: History::default(),
            memory_history: History::default(),
            gpu_history: Vec::new(),
            net_rx_history: History::default(),
            net_tx_history: History::default(),
            net_interface: None,
            sort_key: SortKey::Cpu,
            sort_descending: true,
            show_kernel_threads: false,
            filter: String::new(),
            editing_filter: false,
            selected_pid: None,
            table_state: TableState::default(),
            confirm: None,
            proc_hit: None,
            status: None,
            interval,
            should_quit: false,
        }
    }

    /// Applies a message from the worker.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Snapshot(snapshot) => self.apply_snapshot(*snapshot),
            Event::SignalResult {
                pid,
                signal,
                result,
            } => {
                let name = signal_name(signal);
                match result {
                    Ok(()) => self.set_status(format!("sent {name} to {pid}")),
                    Err(err) => self.set_status(format!("failed: {err}")),
                }
            }
            Event::Error(err) => self.set_status(format!("collection error: {err}")),
        }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.cpu_history.push(snapshot.cpu.total_percent as f64);
        let memory = &snapshot.memory;
        if memory.total_bytes > 0 {
            self.memory_history
                .push(memory.used_bytes as f64 / memory.total_bytes as f64 * 100.0);
        }
        self.gpu_history
            .resize_with(snapshot.gpus.len(), History::default);
        for (history, gpu) in self.gpu_history.iter_mut().zip(&snapshot.gpus) {
            history.push(gpu.utilization_percent.unwrap_or(0.0) as f64);
        }
        if self.net_interface.is_none() {
            self.net_interface = default_interface(&snapshot);
        }
        let (rx, tx) = self
            .net_interface
            .as_deref()
            .and_then(|name| snapshot.networks.iter().find(|n| n.name == name))
            .map(|n| (n.rx_bytes_per_sec, n.tx_bytes_per_sec))
            .unwrap_or((0.0, 0.0));
        self.net_rx_history.push(rx);
        self.net_tx_history.push(tx);
        self.snapshot = Some(snapshot);
    }

    /// Current status message, if still fresh.
    pub fn status(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, at)| at.elapsed() < STATUS_TTL)
            .map(|(msg, _)| msg.as_str())
    }

    fn set_status(&mut self, message: String) {
        self.status = Some((message, Instant::now()));
    }

    /// Processes after filtering and sorting, joined with GPU usage.
    pub fn process_rows(&self) -> Vec<ProcessRow<'_>> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
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

        let filter = self.filter.to_lowercase();
        let mut rows: Vec<ProcessRow<'_>> = snapshot
            .processes
            .iter()
            .filter(|p| self.show_kernel_threads || !p.kernel_thread)
            .filter(|p| {
                filter.is_empty()
                    || p.name.to_lowercase().contains(&filter)
                    || p.command.to_lowercase().contains(&filter)
                    || p.user.to_lowercase().contains(&filter)
                    || p.pid.to_string() == filter
            })
            .map(|info| {
                let (gpu_percent, vram_bytes) =
                    gpu_usage.get(&info.pid).copied().unwrap_or_default();
                ProcessRow {
                    info,
                    gpu_percent,
                    vram_bytes,
                }
            })
            .collect();

        let key = self.sort_key;
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
            if self.sort_descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        rows
    }

    /// Synchronizes the table selection index with `selected_pid` after the
    /// rows were re-sorted or filtered. Call before drawing.
    pub fn sync_selection(&mut self) {
        let pids: Vec<u32> = self.process_rows().iter().map(|r| r.info.pid).collect();
        if pids.is_empty() {
            self.table_state.select(None);
            return;
        }
        let index = self
            .selected_pid
            .and_then(|pid| pids.iter().position(|p| *p == pid))
            // The selected process exited: keep the cursor at the same row.
            .unwrap_or_else(|| self.table_state.selected().unwrap_or(0).min(pids.len() - 1));
        self.table_state.select(Some(index));
        self.selected_pid = Some(pids[index]);
    }

    /// Handles a key press and returns a command for the worker, if any.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Command> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return None;
        }
        if let Some(confirm) = self.confirm.take() {
            // Only an explicit "y" sends the signal; anything else cancels.
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                return Some(Command::Signal {
                    pid: confirm.pid,
                    signal: confirm.signal,
                });
            }
            self.set_status("cancelled".to_owned());
            return None;
        }
        if self.editing_filter {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.editing_filter = false;
                }
                KeyCode::Enter => self.editing_filter = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            return None;
        }

        let rows_len = self.process_rows().len();
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc if !self.filter.is_empty() => self.filter.clear(),
            KeyCode::Up => self.move_selection(-1, rows_len),
            KeyCode::Down => self.move_selection(1, rows_len),
            KeyCode::PageUp => self.move_selection(-20, rows_len),
            KeyCode::PageDown => self.move_selection(20, rows_len),
            KeyCode::Home => self.move_selection(isize::MIN / 2, rows_len),
            KeyCode::End => self.move_selection(isize::MAX / 2, rows_len),
            KeyCode::Left => self.step_sort(-1),
            KeyCode::Right => self.step_sort(1),
            KeyCode::Char('r') => self.sort_descending = !self.sort_descending,
            KeyCode::Char('/') | KeyCode::Char('f') => self.editing_filter = true,
            KeyCode::Char('t') => {
                self.show_kernel_threads = !self.show_kernel_threads;
                let state = if self.show_kernel_threads {
                    "shown"
                } else {
                    "hidden"
                };
                self.set_status(format!("kernel threads {state}"));
            }
            KeyCode::Char('n') => self.cycle_interface(),
            KeyCode::Char('k') => self.ask_signal(ProcessSignal::Terminate),
            KeyCode::Char('K') => self.ask_signal(ProcessSignal::Kill),
            KeyCode::Char('+') => return Some(self.change_interval(true)),
            KeyCode::Char('-') => return Some(self.change_interval(false)),
            _ => {}
        }
        None
    }

    fn move_selection(&mut self, delta: isize, rows_len: usize) {
        if rows_len == 0 {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, rows_len as isize - 1) as usize;
        self.table_state.select(Some(next));
        // Resolve the pid of the new row so the selection sticks to it.
        self.selected_pid = self.process_rows().get(next).map(|r| r.info.pid);
    }

    fn step_sort(&mut self, delta: isize) {
        let index = SortKey::ALL
            .iter()
            .position(|k| *k == self.sort_key)
            .unwrap_or(0) as isize;
        let len = SortKey::ALL.len() as isize;
        self.set_sort(SortKey::ALL[(index + delta).rem_euclid(len) as usize]);
    }

    /// Sorts by `key` in its natural direction, or reverses the order when
    /// it is already the sort key (like clicking a column header twice).
    fn toggle_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_descending = !self.sort_descending;
        } else {
            self.set_sort(key);
        }
    }

    fn set_sort(&mut self, key: SortKey) {
        self.sort_key = key;
        self.sort_descending = key.descending_by_default();
    }

    /// Handles a mouse event: click a row to select it, click a header to
    /// sort, wheel to move the selection. Only the process table reacts.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        // While a signal confirmation is open, a stray click must not change
        // which process the pending signal is about.
        if self.confirm.is_some() {
            return;
        }
        let Some(hit) = self.proc_hit.clone() else {
            return;
        };
        let position = Position::new(mouse.column, mouse.row);
        let in_columns = mouse.column >= hit.rows.x && mouse.column < hit.rows.right();
        let on_header = mouse.row == hit.header_y && in_columns;
        let in_rows = hit.rows.contains(position);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if on_header => {
                let key = hit
                    .columns
                    .iter()
                    .find(|(x, width, _)| mouse.column >= *x && mouse.column < x + width)
                    .and_then(|(_, _, key)| *key);
                if let Some(key) = key {
                    self.toggle_sort(key);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if in_rows => {
                let index = hit.offset + (mouse.row - hit.rows.y) as usize;
                if let Some(pid) = self.process_rows().get(index).map(|r| r.info.pid) {
                    self.table_state.select(Some(index));
                    self.selected_pid = Some(pid);
                }
            }
            MouseEventKind::ScrollUp if in_rows || on_header => {
                let len = self.process_rows().len();
                self.move_selection(-WHEEL_STEP, len);
            }
            MouseEventKind::ScrollDown if in_rows || on_header => {
                let len = self.process_rows().len();
                self.move_selection(WHEEL_STEP, len);
            }
            _ => {}
        }
    }

    fn cycle_interface(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let names: Vec<&str> = snapshot.networks.iter().map(|n| n.name.as_str()).collect();
        if names.is_empty() {
            return;
        }
        let current = self
            .net_interface
            .as_deref()
            .and_then(|name| names.iter().position(|n| *n == name));
        let next = names[current.map_or(0, |i| (i + 1) % names.len())].to_owned();
        self.set_status(format!("network: {next}"));
        self.net_interface = Some(next);
        // The old history belongs to another interface.
        self.net_rx_history = History::default();
        self.net_tx_history = History::default();
    }

    fn ask_signal(&mut self, signal: ProcessSignal) {
        let Some(pid) = self.selected_pid else {
            return;
        };
        let name = self
            .snapshot
            .as_ref()
            .and_then(|s| s.processes.iter().find(|p| p.pid == pid))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.confirm = Some(ConfirmSignal { pid, name, signal });
    }

    fn change_interval(&mut self, slower: bool) -> Command {
        let ms = self.interval.as_millis() as u64;
        let next = if slower {
            (ms + 250).min(10_000)
        } else {
            ms.saturating_sub(250).max(250)
        };
        self.interval = Duration::from_millis(next);
        self.set_status(format!("interval {next} ms"));
        Command::SetInterval(self.interval)
    }
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
fn default_interface(snapshot: &Snapshot) -> Option<String> {
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
    use rigtop_core::snapshot::{GpuProcess, GpuStats, NetworkStats};

    fn process(pid: u32, name: &str, cpu: f32, kernel: bool) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_owned(),
            cpu_percent: cpu,
            kernel_thread: kernel,
            ..Default::default()
        }
    }

    fn app_with(processes: Vec<ProcessInfo>) -> App {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        let snapshot = Snapshot {
            processes,
            gpus: vec![GpuStats {
                processes: vec![GpuProcess {
                    pid: 30,
                    memory_bytes: Some(4 << 30),
                    sm_percent: Some(80.0),
                }],
                ..Default::default()
            }],
            networks: vec![
                NetworkStats {
                    name: "docker0".to_owned(),
                    rx_total_bytes: 1 << 40,
                    ..Default::default()
                },
                NetworkStats {
                    name: "enp9s0".to_owned(),
                    rx_total_bytes: 1 << 30,
                    rx_bytes_per_sec: 1000.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        app.apply(Event::Snapshot(Box::new(snapshot)));
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn pids(app: &App) -> Vec<u32> {
        app.process_rows().iter().map(|r| r.info.pid).collect()
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
    }

    #[test]
    fn rows_hide_kernel_threads_and_sort_by_cpu() {
        let mut app = app_with(vec![
            process(10, "bash", 1.0, false),
            process(2, "kworker/0:1", 50.0, true),
            process(30, "python", 90.0, false),
        ]);
        assert_eq!(pids(&app), vec![30, 10]);
        app.handle_key(key(KeyCode::Char('t')));
        assert_eq!(pids(&app), vec![30, 2, 10]);
    }

    #[test]
    fn gpu_usage_is_joined_and_sortable() {
        let mut app = app_with(vec![
            process(10, "bash", 5.0, false),
            process(30, "python", 1.0, false),
        ]);
        let rows = app.process_rows();
        let python = rows.iter().find(|r| r.info.pid == 30).unwrap();
        assert_eq!(python.vram_bytes, Some(4 << 30));
        assert_eq!(python.gpu_percent, Some(80.0));
        // Step from "cpu" to "gpu" sorting: the GPU user comes first.
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.sort_key, SortKey::Gpu);
        assert_eq!(pids(&app), vec![30, 10]);
    }

    #[test]
    fn filter_matches_name_and_escape_clears() {
        let mut app = app_with(vec![
            process(10, "bash", 1.0, false),
            process(30, "python", 2.0, false),
        ]);
        app.handle_key(key(KeyCode::Char('/')));
        for c in "pyt".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(pids(&app), vec![30]);
        // 'q' after the filter is closed quits rather than typing.
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(pids(&app), vec![30, 10]);
        assert!(!app.should_quit);
    }

    #[test]
    fn kill_requires_confirmation() {
        let mut app = app_with(vec![
            process(10, "bash", 1.0, false),
            process(30, "python", 2.0, false),
        ]);
        app.selected_pid = Some(30);

        // Any key other than "y" cancels.
        assert!(app.handle_key(key(KeyCode::Char('K'))).is_none());
        assert!(app.confirm.is_some());
        assert!(app.handle_key(key(KeyCode::Char('n'))).is_none());
        assert!(app.confirm.is_none());

        app.handle_key(key(KeyCode::Char('K')));
        match app.handle_key(key(KeyCode::Char('y'))) {
            Some(Command::Signal { pid, signal }) => {
                assert_eq!(pid, 30);
                assert_eq!(signal, ProcessSignal::Kill);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn selection_follows_pid_across_reordering() {
        let mut app = app_with(vec![
            process(10, "bash", 1.0, false),
            process(30, "python", 2.0, false),
        ]);
        app.selected_pid = Some(10);
        app.sync_selection();
        assert_eq!(app.table_state.selected(), Some(1));

        // bash becomes the top CPU user: the cursor moves with it.
        let mut snapshot = app.snapshot.clone().unwrap();
        snapshot.processes[0].cpu_percent = 99.0;
        app.apply(Event::Snapshot(Box::new(snapshot)));
        app.sync_selection();
        assert_eq!(app.table_state.selected(), Some(0));
        assert_eq!(app.selected_pid, Some(10));

        // When the selected process exits, the cursor stays on the same row.
        let mut snapshot = app.snapshot.clone().unwrap();
        snapshot.processes.retain(|p| p.pid != 10);
        app.apply(Event::Snapshot(Box::new(snapshot)));
        app.sync_selection();
        assert_eq!(app.selected_pid, Some(30));
    }

    fn click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A table at x=10..50 with the header on row 5 and rows from row 6.
    fn hit_map() -> ProcTableHit {
        ProcTableHit {
            header_y: 5,
            rows: Rect::new(10, 6, 40, 10),
            columns: vec![
                (10, 7, Some(SortKey::Pid)),
                (18, 15, Some(SortKey::Name)),
                (34, 6, Some(SortKey::Cpu)),
            ],
            offset: 0,
        }
    }

    #[test]
    fn click_selects_row_and_header_sorts() {
        let mut app = app_with(vec![
            process(10, "bash", 1.0, false),
            process(30, "python", 2.0, false),
        ]);
        app.proc_hit = Some(hit_map());
        // Rows are sorted by CPU: python (30) first, bash (10) second.
        app.handle_mouse(click(20, 7));
        assert_eq!(app.selected_pid, Some(10));
        assert_eq!(app.table_state.selected(), Some(1));
        // Clicking below the last process does nothing.
        app.handle_mouse(click(20, 12));
        assert_eq!(app.selected_pid, Some(10));

        // Header: "Program" sorts by name ascending; again reverses.
        app.handle_mouse(click(20, 5));
        assert_eq!(app.sort_key, SortKey::Name);
        assert!(!app.sort_descending);
        app.handle_mouse(click(20, 5));
        assert!(app.sort_descending);
        // Gap between columns: no change.
        app.handle_mouse(click(17, 5));
        assert_eq!(app.sort_key, SortKey::Name);
    }

    #[test]
    fn wheel_moves_selection_and_confirm_blocks_clicks() {
        let processes = (1..=10)
            .map(|pid| process(pid, "p", pid as f32, false))
            .collect();
        let mut app = app_with(processes);
        app.proc_hit = Some(hit_map());
        app.handle_mouse(click(20, 6));
        let first = app.selected_pid;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            ..click(20, 8)
        });
        assert_eq!(app.table_state.selected(), Some(3));

        // With a confirmation pending, clicks leave the selection alone.
        app.handle_key(key(KeyCode::Char('k')));
        assert!(app.confirm.is_some());
        app.handle_mouse(click(20, 6));
        assert_ne!(app.selected_pid, first);
        assert_eq!(app.table_state.selected(), Some(3));
    }

    #[test]
    fn default_interface_skips_virtual_links() {
        let app = app_with(Vec::new());
        assert_eq!(app.net_interface.as_deref(), Some("enp9s0"));
        assert_eq!(app.net_rx_history.last(1), vec![1000.0]);
    }
}
