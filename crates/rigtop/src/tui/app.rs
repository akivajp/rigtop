//! UI state: latest snapshot, history for graphs, process table state and
//! key handling. Kept free of drawing code so it can be unit tested.

use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;
use rigtop_core::view::{
    self, GraphSpan, MetricHistory, ProcessQuery, ProcessRow, Severity, SortKey, signal_name,
};
use rigtop_core::worker::{Command, Event};
use rigtop_core::{Config, ProcessSignal, Snapshot};

/// Rows moved per mouse wheel step in the process table.
const WHEEL_STEP: isize = 3;

/// How long a status message stays visible.
const STATUS_TTL: Duration = Duration::from_secs(5);

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
    /// Graph histories, including the interface shown in the network box.
    pub history: MetricHistory,
    pub sort_key: SortKey,
    pub sort_descending: bool,
    pub show_kernel_threads: bool,
    /// Processes under their parents (`e`).
    pub tree: bool,
    pub filter: String,
    /// Whether keystrokes currently edit the filter.
    pub editing_filter: bool,
    /// Selected process, tracked by pid so the selection follows the
    /// process when the order changes.
    pub selected_pid: Option<u32>,
    pub table_state: TableState,
    pub confirm: Option<ConfirmSignal>,
    /// Whether the selected process's details are open (`Enter`).
    pub details: bool,
    /// Process table geometry from the last frame.
    pub proc_hit: Option<ProcTableHit>,
    status: Option<(String, Instant)>,
    pub interval: Duration,
    /// How much time the graphs cover.
    pub graph_span: GraphSpan,
    pub should_quit: bool,
    /// Shown until the first snapshot arrives ("connecting to zefat…").
    pub waiting_message: String,
    /// Latest collection error, until the next snapshot (connection lost).
    pub connection_error: Option<String>,
    /// When the latest snapshot arrived.
    pub last_update: Option<Instant>,
    /// The same, in wall-clock time.
    last_data_at: Option<SystemTime>,
    /// Since when the host sends nothing: its last data, or the first failed
    /// attempt if it never answered. `None` while connected.
    offline_since: Option<SystemTime>,
    /// Unread alerts of every host, shown in the CPU box title (`None`
    /// where the tab bar shows them instead).
    pub alerts_unread: Option<(usize, Option<Severity>)>,
    /// A new release of rigtop, or one waiting for a restart, for the CPU
    /// box title.
    pub release_note: Option<String>,
}

impl App {
    /// Creates the state with a configuration and the initial poll interval.
    pub fn new(config: Config, interval: Duration) -> Self {
        Self {
            config,
            snapshot: None,
            history: MetricHistory::default(),
            sort_key: SortKey::Cpu,
            sort_descending: true,
            show_kernel_threads: false,
            tree: false,
            filter: String::new(),
            editing_filter: false,
            selected_pid: None,
            table_state: TableState::default(),
            confirm: None,
            details: false,
            proc_hit: None,
            status: None,
            interval,
            graph_span: GraphSpan::default(),
            should_quit: false,
            waiting_message: "collecting…".to_owned(),
            connection_error: None,
            last_update: None,
            last_data_at: None,
            offline_since: None,
            alerts_unread: None,
            release_note: None,
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
            Event::Error(err) => {
                self.set_status(format!("collection error: {err}"));
                self.connection_error = Some(err);
                if self.offline_since.is_none() {
                    self.offline_since = Some(self.last_data_at.unwrap_or_else(SystemTime::now));
                }
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.history.record(&snapshot);
        self.snapshot = Some(snapshot);
        self.connection_error = None;
        self.last_update = Some(Instant::now());
        self.last_data_at = Some(SystemTime::now());
        self.offline_since = None;
    }

    /// "offline for 5m (since 14:03)" while the host cannot be reached.
    pub fn offline_note(&self) -> Option<String> {
        let since = self.offline_since?;
        Some(format!(
            "offline for {}",
            view::since_note(since, SystemTime::now())
        ))
    }

    /// Current status message, if still fresh.
    pub fn status(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, at)| at.elapsed() < STATUS_TTL)
            .map(|(msg, _)| msg.as_str())
    }

    pub(super) fn set_status(&mut self, message: String) {
        self.status = Some((message, Instant::now()));
    }

    /// Processes after filtering and sorting, joined with GPU usage.
    pub fn process_rows(&self) -> Vec<ProcessRow<'_>> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        view::process_rows(
            snapshot,
            &ProcessQuery {
                sort_key: self.sort_key,
                descending: self.sort_descending,
                filter: &self.filter,
                show_kernel_threads: self.show_kernel_threads,
                tree: self.tree,
            },
        )
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

        if self.details {
            // Esc or Enter closes the details; other keys work underneath
            // (moving the selection shows the next process's details).
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                self.details = false;
                return None;
            }
        }
        let rows_len = self.process_rows().len();
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Enter => self.details = self.selected_pid.is_some(),
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
            KeyCode::Char('e') => {
                self.tree = !self.tree;
                self.set_status(if self.tree { "tree view" } else { "list view" }.to_owned());
            }
            KeyCode::Char('n') => self.cycle_interface(),
            KeyCode::Char('g') => {
                self.graph_span = self.graph_span.next();
                let span = match self.graph_span {
                    GraphSpan::Fit => "fit to width".to_owned(),
                    span => format!("last {}", span.label()),
                };
                self.set_status(format!("graphs: {span}"));
            }
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
        if let Some(next) = self.history.next_interface(snapshot) {
            self.set_status(format!("network: {next}"));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rigtop_core::snapshot::{GpuProcess, GpuStats, NetworkStats, ProcessInfo};
    use rigtop_core::view::{HISTORY_LEN, History};

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
    fn offline_time_runs_from_the_last_data() {
        let mut app = app_with(Vec::new());
        assert!(app.offline_note().is_none());
        app.apply(Event::Error("ssh: Connection refused".to_owned()));
        let note = app.offline_note().expect("offline after an error");
        assert!(note.starts_with("offline for 0s (since "), "{note}");
        // Later errors keep the start of the outage.
        let since = app.offline_since;
        app.apply(Event::Error("still down".to_owned()));
        assert_eq!(app.offline_since, since);
        app.apply(Event::Snapshot(Box::default()));
        assert!(app.offline_note().is_none(), "connected again");
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
        // Step from "cpu" past "disk" to "gpu" sorting: the GPU user comes first.
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.sort_key, SortKey::Disk);
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
    fn enter_opens_and_closes_details() {
        let mut app = app_with(vec![process(30, "python", 2.0, false)]);
        // Nothing selected: nothing to show.
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.details);
        app.selected_pid = Some(30);
        app.handle_key(key(KeyCode::Enter));
        assert!(app.details);
        // Other keys still work underneath; Esc closes.
        app.handle_key(key(KeyCode::Down));
        assert!(app.details);
        app.handle_key(key(KeyCode::Esc));
        assert!(!app.details);
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
        assert_eq!(app.history.net_interface.as_deref(), Some("enp9s0"));
        assert_eq!(app.history.net_rx.last(1), vec![1000.0]);
    }
}
