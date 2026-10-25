//! Application state, the resizable tile layout and the per-frame entry point.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use egui::{
    Color32, CornerRadius, FontId, Id, Margin, RichText, Sense, Stroke, TextStyle, Ui, Visuals,
};
use egui_tiles::{Container, SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse};
use rigtop_core::config::{HostConfig, OverviewSection, SensorConfig};
use rigtop_core::hosts::{HostKind, HostSpec, host_specs};
use rigtop_core::remote::RemoteSource;
use rigtop_core::view::{
    GraphSpan, MetricHistory, ProcessQuery, ProcessRow, SortKey, process_rows, signal_name,
};
use rigtop_core::worker::{Command, Event, Worker};
use rigtop_core::{Config, DataSource, LocalSource, ProcessSignal, Snapshot};

use crate::hosts_dialog::{self, HostsDialog};
use crate::lhm_setup::{self, LhmSetup};
use crate::notify::{self, Notifier};
use crate::overview;
use crate::panels;
use crate::sensors_dialog::{self, SensorsDialog};
use crate::widget_panel;
use crate::widgets::{self, BACKGROUND, BORDER, DIM, TEXT};

/// How long a status message stays visible.
const STATUS_TTL: Duration = Duration::from_secs(5);

/// Allowed UI zoom range (1.0 = 100%).
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;
/// Zoom change per button press or Ctrl +/- (egui's own step).
const ZOOM_STEP: f32 = 0.1;

/// Keys of the values eframe persists between runs.
const ZOOM_KEY: &str = "zoom";
const LAYOUT_KEY: &str = "layout";
const OVERVIEW_KEY: &str = "overview";
const WIDGETS_KEY: &str = "widgets_visible";
/// eframe storage key for the time the graphs cover (seconds, 0 to fit).
const GRAPH_SPAN_KEY: &str = "graph_span";

/// One panel of the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    Cpu,
    Memory,
    Gpu,
    Storage,
    Network,
    Processes,
}

impl Pane {
    /// Every panel, used to validate a restored layout.
    pub const ALL: [Pane; 6] = [
        Pane::Cpu,
        Pane::Memory,
        Pane::Gpu,
        Pane::Storage,
        Pane::Network,
        Pane::Processes,
    ];

    /// Title shown in the panel border.
    pub fn title(self) -> &'static str {
        match self {
            Pane::Cpu => "cpu",
            Pane::Memory => "mem",
            Pane::Gpu => "gpu",
            Pane::Storage => "storage",
            Pane::Network => "net",
            Pane::Processes => "proc",
        }
    }

    /// Title color, as in the TUI.
    pub fn color(self) -> Color32 {
        match self {
            Pane::Cpu => widgets::CPU_TITLE,
            Pane::Memory => widgets::MEM_TITLE,
            Pane::Gpu => widgets::GPU_TITLE,
            Pane::Storage => widgets::STORAGE_TITLE,
            Pane::Network => widgets::NET_TITLE,
            Pane::Processes => widgets::PROC_TITLE,
        }
    }
}

/// A signal waiting for the user's confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmSignal {
    pub pid: u32,
    pub name: String,
    pub signal: ProcessSignal,
}

/// View changes requested from inside the panels, applied after drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewAction {
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ResetLayout,
    /// Open the window for adding and editing hosts.
    EditHosts,
    /// Show or hide the widget column.
    ToggleWidgets,
    /// Switch the graphs to the next time span.
    NextGraphSpan,
    /// Open the window for naming and hiding sensors.
    EditSensors,
}

/// Everything the panels read and modify.
pub struct GuiState {
    pub config: Config,
    pub snapshot: Option<Snapshot>,
    pub history: MetricHistory,
    pub sort_key: SortKey,
    pub sort_descending: bool,
    pub filter: String,
    pub show_kernel_threads: bool,
    /// Selected process, tracked by pid so it survives re-sorting.
    pub selected_pid: Option<u32>,
    pub confirm: Option<ConfirmSignal>,
    /// Commands queued for the worker during this frame.
    pub commands: Vec<Command>,
    /// View changes queued during this frame.
    pub view_actions: Vec<ViewAction>,
    /// Current UI zoom factor, for display in the panels.
    pub zoom: f32,
    /// Latest collection error, until the next snapshot (connection lost).
    pub connection_error: Option<String>,
    /// When the latest snapshot arrived.
    pub last_update: Option<Instant>,
    /// Whether the CPU panel title shows zoom buttons (single-host mode).
    pub show_zoom: bool,
    /// Whether the widget column is shown, or `None` without widgets (for
    /// the toggle in the CPU panel title in single-host mode).
    pub widgets_visible: Option<bool>,
    /// How much time the graphs cover.
    pub graph_span: GraphSpan,
    /// Collection interval, to turn a time span into samples.
    pub interval: Duration,
    /// Whether this is the machine rigtop runs on (setup offers apply only
    /// to it).
    pub is_local: bool,
    /// The LibreHardwareMonitor setup window (Windows).
    pub lhm_setup: LhmSetup,
    status: Option<(String, Instant)>,
}

impl GuiState {
    /// Creates an empty state.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            snapshot: None,
            history: MetricHistory::default(),
            sort_key: SortKey::Cpu,
            sort_descending: true,
            filter: String::new(),
            show_kernel_threads: false,
            selected_pid: None,
            confirm: None,
            commands: Vec::new(),
            view_actions: Vec::new(),
            zoom: 1.0,
            connection_error: None,
            last_update: None,
            show_zoom: true,
            widgets_visible: None,
            graph_span: GraphSpan::default(),
            interval: Duration::from_secs(1),
            is_local: false,
            lhm_setup: LhmSetup::default(),
            status: None,
        }
    }

    /// Applies a message from the worker.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Snapshot(snapshot) => {
                self.history.record(&snapshot);
                self.snapshot = Some(*snapshot);
                self.connection_error = None;
                self.last_update = Some(Instant::now());
            }
            Event::SignalResult {
                pid,
                signal,
                result,
            } => match result {
                Ok(()) => self.set_status(format!("sent {} to {pid}", signal_name(signal))),
                Err(err) => self.set_status(format!("failed: {err}")),
            },
            Event::Error(err) => {
                self.set_status(format!("collection error: {err}"));
                self.connection_error = Some(err);
            }
        }
    }

    /// Processes after filtering and sorting.
    pub fn process_rows(&self) -> Vec<ProcessRow<'_>> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        process_rows(
            snapshot,
            &ProcessQuery {
                sort_key: self.sort_key,
                descending: self.sort_descending,
                filter: &self.filter,
                show_kernel_threads: self.show_kernel_threads,
            },
        )
    }

    /// Sorts by `key`, or reverses the order if it already is the sort key.
    pub fn toggle_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_key = key;
            self.sort_descending = key.descending_by_default();
        }
    }

    /// Asks for confirmation before sending `signal` to `pid`.
    pub fn ask_signal(&mut self, pid: u32, name: &str, signal: ProcessSignal) {
        self.confirm = Some(ConfirmSignal {
            pid,
            name: name.to_owned(),
            signal,
        });
    }

    /// Current status message, if still fresh.
    pub fn status(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, at)| at.elapsed() < STATUS_TTL)
            .map(|(msg, _)| msg.as_str())
    }

    /// Shows a status message for a few seconds.
    pub fn set_status(&mut self, message: String) {
        self.status = Some((message, Instant::now()));
    }
}

/// One monitored host: its collector and the state its panels show.
pub struct HostEntry {
    pub spec: HostSpec,
    /// `None` in tests, which feed snapshots directly.
    worker: Option<Worker>,
    pub state: GuiState,
    /// Process limit last sent to the worker (`None` = all processes).
    sent_limit: Option<Option<usize>>,
}

impl HostEntry {
    /// Applies queued events; returns true if a snapshot was among them.
    fn drain(&mut self) -> bool {
        let mut snapshot = false;
        if let Some(worker) = &self.worker {
            while let Some(event) = worker.try_recv() {
                snapshot |= matches!(event, Event::Snapshot(_));
                self.state.apply(event);
            }
        }
        snapshot
    }

    /// Sends queued commands and the wanted process limit to the worker.
    fn flush(&mut self, limit: Option<usize>) {
        let Some(worker) = &self.worker else {
            self.state.commands.clear();
            return;
        };
        for command in self.state.commands.drain(..) {
            worker.send(command);
        }
        if self.sent_limit != Some(limit) {
            worker.send(Command::SetProcessLimit(limit));
            self.sent_limit = Some(limit);
        }
    }

    /// What to show before the first snapshot arrives.
    pub fn waiting_message(&self) -> String {
        if self.spec.is_local() {
            "collecting…".to_owned()
        } else {
            format!("connecting to {}…", self.spec.name)
        }
    }
}

/// What the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// All hosts side by side.
    Overview,
    /// The detailed panels of one host (index into the host list).
    Host(usize),
}

/// The eframe application.
pub struct RigtopApp {
    pub hosts: Vec<HostEntry>,
    pub view: View,
    pub overview: OverviewSettings,
    /// The loaded configuration; `hosts` changes when edited in the GUI.
    config: Config,
    /// Where host changes are saved (`None` if there is no config directory).
    config_path: Option<PathBuf>,
    /// Hosts given with `--host`, which are not saved.
    cli_hosts: Vec<String>,
    remote_command: Option<String>,
    interval: Duration,
    /// Context for waking the UI from new workers; `None` in tests, where
    /// hosts get no worker.
    repaint: Option<egui::Context>,
    pub hosts_dialog: HostsDialog,
    pub sensors_dialog: SensorsDialog,
    /// Desktop notifications for new alerts.
    notifier: Notifier,
    /// The widget column (`[[widgets]]`), empty without any.
    pub(crate) widgets: Vec<rigtop_core::widget::Widget>,
    /// Whether the widget column is shown (it can be hidden for room).
    show_widgets: bool,
    graph_span: GraphSpan,
    tree: Tree<Pane>,
    /// Processes per overview card (hosts not shown in detail send only these).
    overview_processes: usize,
    /// Whether the theme (and restored zoom) has been applied to the context.
    styled: bool,
    /// Zoom restored from the previous run, applied on the first frame.
    initial_zoom: f32,
    /// Current zoom, saved on exit.
    zoom: f32,
}

impl RigtopApp {
    /// Starts one collector per host (this machine, `[[hosts]]` from the
    /// configuration and `extra_hosts` from the command line), each waking
    /// the UI on new data.
    ///
    /// The zoom factor, panel layout and overview settings of the previous
    /// run are restored from eframe's storage when available.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        config_path: Option<PathBuf>,
        interval: Duration,
        extra_hosts: &[String],
        remote_command: Option<&str>,
    ) -> Self {
        let storage = cc.storage;
        let initial_zoom = storage
            .and_then(|s| eframe::get_value::<f32>(s, ZOOM_KEY))
            .map_or(1.0, |z| z.clamp(MIN_ZOOM, MAX_ZOOM));
        let tree = storage
            .and_then(|s| eframe::get_value::<Tree<Pane>>(s, LAYOUT_KEY))
            .filter(is_complete_layout)
            .unwrap_or_else(default_tree);
        let overview = storage
            .and_then(|s| eframe::get_value::<OverviewSettings>(s, OVERVIEW_KEY))
            .unwrap_or_else(|| OverviewSettings::from_config(&config));
        let mut app = Self::assemble(config, tree, overview, initial_zoom);
        app.config_path = config_path;
        app.cli_hosts = extra_hosts.to_vec();
        app.remote_command = remote_command.map(str::to_owned);
        app.interval = interval;
        app.repaint = Some(cc.egui_ctx.clone());
        app.graph_span = storage
            .and_then(|s| eframe::get_value::<u64>(s, GRAPH_SPAN_KEY))
            .map_or(GraphSpan::Fit, GraphSpan::from_secs);
        app.show_widgets = storage
            .and_then(|s| eframe::get_value::<bool>(s, WIDGETS_KEY))
            .unwrap_or(true);
        let repaint = cc.egui_ctx.clone();
        app.widgets = rigtop_core::widget::start_widgets(&app.config.widgets, move || {
            repaint.request_repaint()
        });
        app.update_hosts();
        app
    }

    /// An app without hosts; `update_hosts` (or a test) adds them.
    fn assemble(
        config: Config,
        tree: Tree<Pane>,
        overview: OverviewSettings,
        initial_zoom: f32,
    ) -> Self {
        let notifier = Notifier::new(config.notifications.level);
        Self {
            hosts: Vec::new(),
            view: View::Host(0),
            overview,
            overview_processes: config.overview.processes,
            config,
            config_path: None,
            cli_hosts: Vec::new(),
            remote_command: None,
            interval: Duration::from_secs(1),
            repaint: None,
            hosts_dialog: HostsDialog::default(),
            sensors_dialog: SensorsDialog::default(),
            notifier,
            widgets: Vec::new(),
            show_widgets: true,
            graph_span: GraphSpan::default(),
            tree,
            styled: false,
            initial_zoom,
            zoom: initial_zoom,
        }
    }

    /// Starts collecting from `spec` (no worker in tests).
    fn start_host(&self, spec: HostSpec) -> HostEntry {
        let worker = self.repaint.clone().map(|repaint| {
            let source_config = self.config.clone();
            let kind = spec.kind.clone();
            Worker::spawn_with_notify(
                move || -> Box<dyn DataSource> {
                    match kind {
                        HostKind::Remote(target) => Box::new(RemoteSource::new(target)),
                        HostKind::Local => Box::new(LocalSource::with_config(&source_config)),
                    }
                },
                self.interval,
                move || repaint.request_repaint(),
            )
        });
        let mut state = GuiState::new(self.config.clone());
        state.is_local = spec.is_local();
        HostEntry {
            spec,
            worker,
            state,
            sent_limit: None,
        }
    }

    /// Brings the running hosts in line with the configuration and the
    /// command line: unchanged hosts keep their collector and history, new
    /// ones are started and removed ones stopped.
    fn update_hosts(&mut self) {
        let specs = host_specs(
            &self.config,
            &self.cli_hosts,
            self.remote_command.as_deref(),
        );
        let shown = match self.view {
            View::Host(index) => self.hosts.get(index).map(|h| h.spec.name.clone()),
            View::Overview => None,
        };
        let was_multi = self.hosts.len() > 1;
        let mut old = std::mem::take(&mut self.hosts);
        let mut hosts = Vec::with_capacity(specs.len());
        for spec in specs {
            match old
                .iter()
                .position(|h| h.spec.name == spec.name && h.spec.kind == spec.kind)
            {
                Some(position) => {
                    let mut entry = old.swap_remove(position);
                    entry.spec = spec;
                    hosts.push(entry);
                }
                None => hosts.push(self.start_host(spec)),
            }
        }
        // Stopping a worker joins its thread, which can take a while for a
        // host that is still connecting; do not freeze the UI meanwhile.
        let stopped: Vec<Worker> = old.into_iter().filter_map(|h| h.worker).collect();
        if !stopped.is_empty() {
            thread::spawn(move || drop(stopped));
        }
        self.hosts = hosts;
        // Like at startup, the overview opens once there are several hosts
        // and needs at least two; otherwise stay on the same host if it is
        // still there.
        let multi = self.hosts.len() > 1;
        let position = shown.and_then(|name| self.hosts.iter().position(|h| h.spec.name == name));
        self.view = match position {
            Some(index) if was_multi || !multi => View::Host(index),
            _ if multi => View::Overview,
            _ => View::Host(0),
        };
    }

    /// Writes `hosts` to the configuration file and applies them.
    fn save_hosts(&mut self, hosts: Vec<HostConfig>) {
        let Some(path) = self.config_path.clone() else {
            self.hosts_dialog
                .report("no configuration file to save to".to_owned(), false);
            return;
        };
        match Config::save_hosts(&path, &hosts) {
            Ok(()) => {
                self.config.hosts = hosts;
                self.update_hosts();
                self.hosts_dialog
                    .report(format!("saved to {}", path.display()), true);
            }
            Err(err) => self.hosts_dialog.report(format!("not saved: {err}"), false),
        }
    }

    /// Writes sensor names and hidden sensors to the configuration file and
    /// applies them to every host.
    fn save_sensors(&mut self, sensors: SensorConfig) {
        let Some(path) = self.config_path.clone() else {
            self.sensors_dialog
                .report("no configuration file to save to".to_owned(), false);
            return;
        };
        match Config::save_sensors(&path, &sensors) {
            Ok(()) => {
                for host in &mut self.hosts {
                    host.state.config.sensors = sensors.clone();
                }
                self.config.sensors = sensors;
                self.sensors_dialog
                    .report(format!("saved to {}", path.display()), true);
            }
            Err(err) => self
                .sensors_dialog
                .report(format!("not saved: {err}"), false),
        }
    }

    /// Names that cannot be used for a new host: this machine and hosts
    /// from the command line.
    fn fixed_host_names(&self) -> Vec<String> {
        self.hosts
            .iter()
            .filter(|h| {
                h.spec.is_local() || !self.config.hosts.iter().any(|c| c.name == h.spec.name)
            })
            .map(|h| h.spec.name.clone())
            .collect()
    }

    /// Creates the app for `names` (the first one local) without workers;
    /// snapshots come from `state.apply`.
    #[cfg(test)]
    pub fn for_test(config: Config, names: &[&str]) -> Self {
        let overview = OverviewSettings::from_config(&config);
        let mut app = Self::assemble(config, default_tree(), overview, 1.0);
        app.hosts = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                app.start_host(HostSpec {
                    name: (*name).to_owned(),
                    kind: if i == 0 {
                        HostKind::Local
                    } else {
                        HostKind::Remote(rigtop_core::remote::RemoteTarget::ssh(
                            name,
                            "rigtop --agent",
                        ))
                    },
                    sections: app.config.overview.sections.clone(),
                })
            })
            .collect();
        // With a single host there is nothing to overview: show it directly.
        if app.hosts.len() > 1 {
            app.view = View::Overview;
        }
        app
    }

    /// A single-host app without a worker (tests).
    #[cfg(test)]
    pub fn without_worker(config: Config) -> Self {
        Self::for_test(config, &["local"])
    }

    /// State of the first host (the only one in single-host tests).
    #[cfg(test)]
    pub fn primary(&self) -> &GuiState {
        &self.hosts[0].state
    }

    /// Mutable state of the first host.
    #[cfg(test)]
    pub fn primary_mut(&mut self) -> &mut GuiState {
        &mut self.hosts[0].state
    }

    /// Applies everything the workers have sent since the last call, and
    /// announces alerts that are new in the fresh snapshots.
    fn drain_events(&mut self) {
        for host in &mut self.hosts {
            if !host.drain() {
                continue;
            }
            let Some(snapshot) = &host.state.snapshot else {
                continue;
            };
            let alerts = rigtop_core::view::alerts_with(snapshot, &host.state.config.alerts);
            for alert in self.notifier.new_alerts(&host.spec.name, &alerts) {
                // Tests run without workers (and without notifications).
                if self.repaint.is_some() {
                    // "local" says little in a notification; use the host name.
                    let name = if host.spec.is_local() {
                        &snapshot.host.hostname
                    } else {
                        &host.spec.name
                    };
                    notify::send(name, &alert);
                }
            }
        }
    }

    /// Draws one frame.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.styled {
            setup_style(&ctx);
            // Context style changes apply from the next frame; this frame's
            // Ui needs the theme too (the default font lacks "▼" etc.).
            ui.set_style(ctx.global_style());
            ctx.set_zoom_factor(self.initial_zoom);
            self.styled = true;
        }
        // Ctrl + mouse wheel (or pinch) zooms the whole UI; egui already
        // handles Ctrl + "+" / "-" / "0" itself.
        let wheel_zoom = ctx.input(|i| i.zoom_delta());
        if wheel_zoom != 1.0 {
            set_zoom(&ctx, ctx.zoom_factor() * wheel_zoom);
        }
        self.zoom = ctx.zoom_factor();
        self.drain_events();
        if let View::Host(index) = self.view
            && index >= self.hosts.len()
        {
            self.view = View::Host(0);
        }

        // Cards and panels alike draw their graphs over the same span.
        for host in &mut self.hosts {
            host.state.graph_span = self.graph_span;
            host.state.interval = self.interval;
        }
        let mut actions = Vec::new();
        egui::Frame::new()
            .fill(BACKGROUND)
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                // Widgets get a column on the right in every view.
                if !self.widgets.is_empty() && self.show_widgets {
                    egui::Panel::right("widget_panel")
                        .resizable(true)
                        .default_size(widget_panel::DEFAULT_WIDTH)
                        .size_range(160.0..=900.0)
                        .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(Margin {
                            left: 6,
                            ..Margin::ZERO
                        }))
                        .show(ui, |ui| widget_panel::show(ui, &self.widgets));
                }
                if self.hosts.len() > 1 {
                    self.tab_bar(ui, &mut actions);
                }
                match self.view {
                    View::Overview => {
                        if let Some(open) = overview::show(
                            ui,
                            &mut self.hosts,
                            &mut self.overview,
                            self.overview_processes,
                        ) {
                            self.view = View::Host(open);
                        }
                    }
                    View::Host(index) => {
                        let single = self.hosts.len() == 1;
                        let host = &mut self.hosts[index];
                        host.state.zoom = self.zoom;
                        // With several hosts the zoom buttons live in the tab bar.
                        host.state.show_zoom = single;
                        host.state.widgets_visible =
                            (!self.widgets.is_empty()).then_some(self.show_widgets);
                        if host.state.snapshot.is_none() {
                            waiting_screen(ui, host);
                        } else {
                            let mut behavior = Behavior {
                                state: &mut host.state,
                            };
                            self.tree.ui(&mut behavior, ui);
                        }
                        actions.append(&mut host.state.view_actions);
                    }
                }
            });
        if let View::Host(index) = self.view {
            panels::confirm_dialog(ui.ctx(), &mut self.hosts[index].state);
            lhm_setup::dialog(ui.ctx(), &mut self.hosts[index].state);
        }

        for action in actions {
            match action {
                ViewAction::ZoomIn => set_zoom(&ctx, ctx.zoom_factor() + ZOOM_STEP),
                ViewAction::ZoomOut => set_zoom(&ctx, ctx.zoom_factor() - ZOOM_STEP),
                ViewAction::ZoomReset => set_zoom(&ctx, 1.0),
                ViewAction::ResetLayout => self.tree = default_tree(),
                ViewAction::EditHosts => self.hosts_dialog.open(),
                ViewAction::ToggleWidgets => self.show_widgets = !self.show_widgets,
                ViewAction::NextGraphSpan => self.graph_span = self.graph_span.next(),
                ViewAction::EditSensors => self.sensors_dialog.open(&self.config.sensors),
            }
        }
        let fixed = self.fixed_host_names();
        if let Some(hosts) = hosts_dialog::show(
            &ctx,
            &mut self.hosts_dialog,
            &self.config.hosts,
            &fixed,
            self.config_path.as_deref(),
        ) {
            self.save_hosts(hosts);
        }
        // The sensors of the host shown in detail.
        let sensors = match self.view {
            View::Host(index) => self.hosts[index]
                .state
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    sensors_dialog::show(
                        &ctx,
                        &mut self.sensors_dialog,
                        snapshot,
                        self.config_path.as_deref(),
                    )
                }),
            View::Overview => None,
        };
        if let Some(sensors) = sensors {
            self.save_sensors(sensors);
        }

        // The host shown in detail sends every process; the others only
        // what an overview card lists.
        for (index, host) in self.hosts.iter_mut().enumerate() {
            let limit = if self.view == View::Host(index) {
                None
            } else {
                Some(self.overview_processes)
            };
            host.flush(limit);
        }
        // Repaint at the next full second so the clock ticks; new data
        // triggers repaints through the workers' notifications.
        let subsec = chrono::Local::now().timestamp_subsec_millis() as u64;
        ui.ctx()
            .request_repaint_after(Duration::from_millis(1000 - subsec.min(999)));
    }

    /// "Overview | local | zefat | ..." with a status dot per host, plus the
    /// zoom controls on the right.
    fn tab_bar(&mut self, ui: &mut Ui, actions: &mut Vec<ViewAction>) {
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::selectable(
                    self.view == View::Overview,
                    RichText::new("Overview").strong(),
                ))
                .clicked()
            {
                self.view = View::Overview;
            }
            ui.add_space(6.0);
            for (index, host) in self.hosts.iter().enumerate() {
                let (color, status) = overview::host_status(host);
                let selected = self.view == View::Host(index);
                let label = overview::dot_label(color, &host.spec.name, selected);
                let response = ui
                    .add(egui::Button::selectable(selected, label))
                    .on_hover_text(status);
                if response.clicked() {
                    self.view = View::Host(index);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                overview::zoom_controls(ui, self.zoom, actions);
                ui.add_space(8.0);
                overview::hosts_button(ui, actions);
                if !self.widgets.is_empty() {
                    overview::widgets_button(ui, self.show_widgets, actions);
                }
            });
        });
        ui.add_space(2.0);
    }
}

/// "connecting to zefat…" with the latest connection error under it.
fn waiting_screen(ui: &mut Ui, host: &HostEntry) {
    let error = host.state.connection_error.clone();
    let message = host.waiting_message();
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 2.0 - 20.0);
        ui.label(RichText::new(message).color(DIM));
        if let Some(error) = error {
            ui.label(RichText::new(error).color(widgets::WARN));
        }
    });
}

/// Overview choices made in the GUI, persisted between runs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OverviewSettings {
    /// Whether the local machine gets a card.
    pub include_local: bool,
    /// Section order chosen in the GUI per host name, overriding the
    /// configuration.
    pub sections: std::collections::BTreeMap<String, Vec<OverviewSection>>,
}

impl OverviewSettings {
    /// Defaults from the configuration file.
    pub fn from_config(config: &Config) -> Self {
        Self {
            include_local: config.overview.include_local,
            sections: Default::default(),
        }
    }

    /// Sections for a host: the GUI choice, else the configured order.
    pub fn sections_for(&self, spec: &HostSpec) -> Vec<OverviewSection> {
        self.sections
            .get(&spec.name)
            .cloned()
            .unwrap_or_else(|| spec.sections.clone())
    }
}

impl eframe::App for RigtopApp {
    /// Runs before every frame and also while the window is hidden
    /// (minimized, screen locked), when `ui` is not called. Draining here
    /// keeps the graphs' history going and the event queues short.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, ZOOM_KEY, &self.zoom);
        eframe::set_value(storage, LAYOUT_KEY, &self.tree);
        eframe::set_value(storage, OVERVIEW_KEY, &self.overview);
        eframe::set_value(storage, WIDGETS_KEY, &self.show_widgets);
        eframe::set_value(storage, GRAPH_SPAN_KEY, &self.graph_span.as_secs());
    }
}

/// Sets the UI zoom, clamped and rounded to whole percents.
fn set_zoom(ctx: &egui::Context, zoom: f32) {
    let zoom = (zoom.clamp(MIN_ZOOM, MAX_ZOOM) * 100.0).round() / 100.0;
    ctx.set_zoom_factor(zoom);
}

/// Whether a restored layout still contains every panel exactly once (a
/// layout saved by another version might not).
fn is_complete_layout(tree: &Tree<Pane>) -> bool {
    let panes: Vec<Pane> = tree
        .tiles
        .iter()
        .filter_map(|(_, tile)| match tile {
            Tile::Pane(pane) => Some(*pane),
            Tile::Container(_) => None,
        })
        .collect();
    panes.len() == Pane::ALL.len() && Pane::ALL.iter().all(|p| panes.contains(p))
}

/// Dark theme with monospace text everywhere, like a terminal monitor.
fn setup_style(ctx: &egui::Context) {
    ctx.set_visuals(Visuals::dark());
    ctx.all_styles_mut(|style| {
        let size = |s: f32| FontId::monospace(s);
        style.text_styles = [
            (TextStyle::Small, size(11.0)),
            (TextStyle::Body, size(13.0)),
            (TextStyle::Button, size(13.0)),
            (TextStyle::Monospace, size(13.0)),
            (TextStyle::Heading, size(15.0)),
        ]
        .into();
        style.visuals.panel_fill = BACKGROUND;
        style.visuals.window_fill = Color32::from_rgb(30, 30, 40);
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = widgets::SELECTION;
        // Dense rows, like a terminal monitor.
        style.spacing.item_spacing = egui::vec2(6.0, 1.0);
        style.spacing.interact_size.y = 16.0;
        // Selectable labels would swallow clicks meant for table rows.
        style.interaction.selectable_labels = false;
    });
}

/// btop-like default arrangement:
///
/// ```text
/// cpu (22%)
/// mem / gpu / storage / net (45%) | proc (55%)
/// ```
fn default_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let cpu = tiles.insert_pane(Pane::Cpu);
    let mem = tiles.insert_pane(Pane::Memory);
    let gpu = tiles.insert_pane(Pane::Gpu);
    let storage = tiles.insert_pane(Pane::Storage);
    let net = tiles.insert_pane(Pane::Network);
    let procs = tiles.insert_pane(Pane::Processes);
    let left = tiles.insert_vertical_tile(vec![mem, gpu, storage, net]);
    let bottom = tiles.insert_horizontal_tile(vec![left, procs]);
    let root = tiles.insert_vertical_tile(vec![cpu, bottom]);

    set_shares(&mut tiles, root, &[(cpu, 22.0), (bottom, 78.0)]);
    set_shares(&mut tiles, bottom, &[(left, 45.0), (procs, 55.0)]);
    set_shares(
        &mut tiles,
        left,
        &[(mem, 17.0), (gpu, 27.0), (storage, 27.0), (net, 29.0)],
    );
    Tree::new(Id::unique("rigtop_layout"), root, tiles)
}

/// Sets the relative sizes of the children of a linear container.
fn set_shares(tiles: &mut Tiles<Pane>, container: TileId, shares: &[(TileId, f32)]) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        for &(child, share) in shares {
            linear.shares.set_share(child, share);
        }
    }
}

/// Connects egui_tiles to the panels.
struct Behavior<'a> {
    state: &'a mut GuiState,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let pane = *pane;
        let mut response = UiResponse::None;
        egui::Frame::new()
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                // The title doubles as the drag handle for rearranging panels.
                let title = ui
                    .horizontal(|ui| {
                        let handle = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(pane.title()).color(pane.color()).strong(),
                                )
                                .sense(Sense::drag()),
                            )
                            .on_hover_cursor(egui::CursorIcon::Grab)
                            .on_hover_text("drag to move this panel");
                        panels::title_extras(ui, pane, self.state);
                        handle
                    })
                    .inner;
                if title.drag_started() {
                    response = UiResponse::DragStarted;
                }
                ui.push_id(pane.title(), |ui| {
                    panels::pane_contents(ui, pane, self.state)
                });
            });
        response
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            // Panels carry their own titles; no tab bars unless the user
            // stacks panels into tabs by dragging.
            all_panes_must_have_tabs: false,
            ..Default::default()
        }
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use rigtop_core::snapshot::{
        BtrfsDeviceStats, BtrfsFilesystem, CpuStats, DiskIo, GpuProcess, GpuStats, HostInfo,
        MemoryStats, NetworkStats, ProcessInfo, RaidStats, SmartDevice,
    };

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            host: HostInfo {
                hostname: "testhost".to_owned(),
                uptime_secs: 3600,
                ..Default::default()
            },
            cpu: CpuStats {
                model: "Test CPU 16-Core".to_owned(),
                total_percent: 23.0,
                per_core_percent: (0..32).map(|i| (i * 3 % 100) as f32).collect(),
                per_core_mhz: vec![4200; 32],
                ..Default::default()
            },
            memory: MemoryStats {
                total_bytes: 96 << 30,
                used_bytes: 12 << 30,
                available_bytes: 84 << 30,
                free_bytes: 20 << 30,
                cached_bytes: Some(60 << 30),
                ..Default::default()
            },
            gpus: vec![GpuStats {
                name: "NVIDIA GeForce RTX 4090".to_owned(),
                utilization_percent: Some(55.0),
                memory_used_bytes: Some(4 << 30),
                memory_total_bytes: Some(24 << 30),
                temperature_c: Some(47.0),
                processes: vec![GpuProcess {
                    pid: 4242,
                    memory_bytes: Some(4 << 30),
                    sm_percent: Some(50.0),
                }],
                ..Default::default()
            }],
            disks: vec![DiskIo {
                name: "nvme0n1".to_owned(),
                temperature_c: Some(57.0),
                ..Default::default()
            }],
            smart: vec![SmartDevice {
                device: "nvme0".to_owned(),
                passed: Some(true),
                percentage_used: Some(3),
                ..Default::default()
            }],
            raid: RaidStats {
                btrfs: vec![BtrfsFilesystem {
                    fsid: "acb63bf0".to_owned(),
                    mount_points: vec!["/".to_owned()],
                    devices: vec!["nvme0n1".to_owned()],
                    device_stats: vec![BtrfsDeviceStats {
                        devid: 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                md: Vec::new(),
            },
            networks: vec![NetworkStats {
                name: "enp9s0".to_owned(),
                link_speed_mbps: Some(10_000),
                link_up: Some(true),
                ..Default::default()
            }],
            processes: vec![
                ProcessInfo {
                    pid: 4242,
                    name: "python".to_owned(),
                    command: "python train.py".to_owned(),
                    cpu_percent: 99.0,
                    memory_bytes: 2 << 30,
                    ..Default::default()
                },
                ProcessInfo {
                    pid: 1,
                    name: "systemd".to_owned(),
                    ..Default::default()
                },
            ],
            process_count: 2,
            ..Default::default()
        }
    }

    fn harness_with(snapshot: Snapshot) -> Harness<'static, RigtopApp> {
        let mut app = RigtopApp::without_worker(Config::default());
        for _ in 0..30 {
            app.primary_mut()
                .apply(Event::Snapshot(Box::new(snapshot.clone())));
        }
        Harness::builder()
            .with_size([1400.0, 900.0])
            .build_ui_state(|ui, app: &mut RigtopApp| app.show(ui), app)
    }

    #[test]
    fn renders_every_panel() {
        let mut harness = harness_with(sample_snapshot());
        harness.run();
        for label in [
            "cpu",
            "mem",
            "gpu",
            "storage",
            "net",
            "proc",
            "testhost",
            "python",
            "GeForce RTX 4090",
            "errors 0",
            "scrub: no record",
            "wear 3%",
        ] {
            assert!(
                harness.query_all_by_label_contains(label).next().is_some(),
                "missing {label:?}"
            );
        }
    }

    #[test]
    fn header_click_sorts_and_context_menu_asks_before_killing() {
        let mut harness = harness_with(sample_snapshot());
        harness.run();
        harness.get_by_label("Pid").click();
        harness.run();
        assert_eq!(harness.state().primary().sort_key, SortKey::Pid);
        assert!(!harness.state().primary().sort_descending);

        harness.get_by_label("python").click_secondary();
        harness.run();
        harness.get_by_label("Kill (SIGKILL)").click();
        harness.run();
        // Nothing is sent until the dialog is confirmed.
        assert!(harness.state().primary().commands.is_empty());
        let confirm = harness
            .state()
            .primary()
            .confirm
            .clone()
            .expect("confirmation dialog");
        assert_eq!((confirm.pid, confirm.signal), (4242, ProcessSignal::Kill));
        harness.get_by_label("Cancel").click();
        harness.run();
        assert!(harness.state().primary().confirm.is_none());
    }

    #[test]
    fn layout_validation_and_zoom_controls() {
        assert!(is_complete_layout(&default_tree()));
        // A layout missing a panel (e.g. from another version) is rejected.
        let partial = Tree::new_vertical(Id::unique("partial"), vec![Pane::Cpu, Pane::Memory]);
        assert!(!is_complete_layout(&partial));

        let mut harness = harness_with(sample_snapshot());
        harness.run();
        harness.get_by_label("A+").click();
        harness.run();
        assert!((harness.ctx.zoom_factor() - 1.1).abs() < 1e-4);
        harness.get_by_label("A-").click();
        harness.run();
        harness.get_by_label("A-").click();
        harness.run();
        assert!((harness.ctx.zoom_factor() - 0.9).abs() < 1e-4);
        harness.get_by_label("90%").click();
        harness.run();
        assert!((harness.ctx.zoom_factor() - 1.0).abs() < 1e-4);
    }

    fn multi_host_harness() -> Harness<'static, RigtopApp> {
        let mut app = RigtopApp::for_test(Config::default(), &["local", "zefat", "nas"]);
        for host in &mut app.hosts[..2] {
            for _ in 0..5 {
                host.state
                    .apply(Event::Snapshot(Box::new(sample_snapshot())));
            }
        }
        // "nas" never connects: its card shows the error.
        app.hosts[2].state.apply(Event::Error(
            "nas: ssh: Could not resolve hostname nas".to_owned(),
        ));
        Harness::builder()
            .with_size([1400.0, 900.0])
            .build_ui_state(|ui, app: &mut RigtopApp| app.show(ui), app)
    }

    #[test]
    fn overview_is_the_default_with_several_hosts() {
        let mut harness = multi_host_harness();
        harness.run();
        assert_eq!(harness.state().view, View::Overview);
        for label in [
            "Overview",
            "zefat",
            "nas",
            "Could not resolve hostname",
            "CPU",
            "GPU0",
            "Mem",
        ] {
            assert!(
                harness.query_all_by_label_contains(label).next().is_some(),
                "missing {label:?}"
            );
        }
        // Hosts not shown in detail only get the top processes.
        assert_eq!(
            harness.state().hosts[1].sent_limit,
            None,
            "no worker in tests"
        );
    }

    #[test]
    fn clicking_a_tab_opens_the_detail_view() {
        let mut harness = multi_host_harness();
        harness.run();
        // The tab comes first; the card title has the same text.
        harness
            .query_all_by_label_contains("● zefat")
            .next()
            .expect("zefat tab")
            .click();
        harness.run();
        assert_eq!(harness.state().view, View::Host(1));
        // The detail panels of that host are shown.
        assert!(harness.query_all_by_label_contains("proc").next().is_some());
        harness.get_by_label("Overview").click();
        harness.run();
        assert_eq!(harness.state().view, View::Overview);
    }

    #[test]
    fn overview_settings_follow_the_gui() {
        let mut settings = OverviewSettings::from_config(&Config::default());
        let spec = HostSpec {
            name: "nas".to_owned(),
            kind: HostKind::Local,
            sections: vec![OverviewSection::Cpu],
        };
        assert_eq!(settings.sections_for(&spec), vec![OverviewSection::Cpu]);
        settings.sections.insert(
            "nas".to_owned(),
            vec![OverviewSection::Storage, OverviewSection::Cpu],
        );
        assert_eq!(settings.sections_for(&spec)[0], OverviewSection::Storage);

        // Unticking "include this machine" removes the local card.
        let mut harness = multi_host_harness();
        harness.run();
        harness.get_by_label("include this machine").click();
        harness.run();
        assert!(!harness.state().overview.include_local);
    }

    #[test]
    fn laptop_shows_soc_power_and_battery() {
        let mut snapshot = sample_snapshot();
        snapshot.power.cpu_package_w = None;
        snapshot.power.igpu_w = Some(6.0);
        snapshot.power.battery = Some(rigtop_core::snapshot::BatteryStats {
            percent: Some(72.0),
            status: "Charging".to_owned(),
            power_w: Some(20.0),
        });
        let mut harness = harness_with(snapshot);
        harness.run();
        for label in ["Power SoC", "72% +20W"] {
            assert!(
                harness.query_all_by_label_contains(label).next().is_some(),
                "missing {label:?}"
            );
        }
    }

    #[test]
    fn hints_at_librehardwaremonitor_when_unreachable() {
        let mut snapshot = sample_snapshot();
        snapshot.lhm_error = Some("127.0.0.1:8085: connection refused".to_owned());
        let mut harness = harness_with(snapshot);
        harness.run();
        assert!(
            harness
                .query_all_by_label_contains(rigtop_core::view::LHM_HINT)
                .next()
                .is_some()
        );
    }

    #[test]
    fn librehardwaremonitor_setup_window() {
        let mut snapshot = sample_snapshot();
        snapshot.lhm_error = Some("127.0.0.1:8085: connection refused".to_owned());
        let mut harness = harness_with(snapshot.clone());
        harness.run();
        harness.get_by_label("set up…").click();
        harness.run();
        assert!(harness.query_by_label_contains("127.0.0.1:8085").is_some());
        // The steps run in a thread; outside Windows they fail at once. (On
        // Windows they would really install and start LibreHardwareMonitor.)
        #[cfg(not(windows))]
        {
            harness.get_by_label("Set up").click();
            for _ in 0..50 {
                // step, not run: the spinner keeps repainting.
                harness.step();
                if harness.query_by_label_contains("for Windows").is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            harness.run();
            assert!(harness.query_by_label_contains("for Windows").is_some());
            harness.get_by_label("Close").click();
        }
        #[cfg(windows)]
        harness.get_by_label("Cancel").click();
        harness.run();
        assert!(harness.query_by_label("Set up").is_none());

        // Already running: only where to switch the web server on.
        snapshot.processes.push(rigtop_core::snapshot::ProcessInfo {
            pid: 4242,
            name: "LibreHardwareMonitor.exe".to_owned(),
            ..Default::default()
        });
        let mut harness = harness_with(snapshot);
        harness.run();
        harness.get_by_label("set up…").click();
        harness.run();
        assert!(harness.query_by_label_contains("is running").is_some());
        assert!(harness.query_by_label("Set up").is_none());
    }

    /// A clock and a widget with fixed rows, as a command would print.
    fn sample_widgets() -> Vec<rigtop_core::widget::Widget> {
        use rigtop_core::widget::{Widget, WidgetSource, parse_output};
        let rows = r#"{"rows":[
            {"label":"5h","value":"58% left · 14:30","gauge":0.58,"gauge_style":"level","level":"ok"},
            {"label":"week","value":"22% left · Fri 09:00","gauge":0.22,"gauge_style":"level","level":"warn"},
            {"value":"rigtop","heading":true},
            {"label":"busy","value":"7a · 2 agents","level":"ok"},
            {"label":"idle","value":"06 · waiting on 1 task · 3m","level":"ok"},
            {"value":"my-obsidian-vault","heading":true},
            {"label":"idle","value":"d8 · your turn · 12m","level":"normal"}
        ]}"#;
        vec![
            Widget {
                name: "Clock".to_owned(),
                source: WidgetSource::Clock,
            },
            Widget {
                name: "Claude Code".to_owned(),
                source: WidgetSource::Fixed(parse_output(rows)),
            },
        ]
    }

    #[test]
    fn widget_column_shows_clock_and_rows() {
        let mut app = RigtopApp::for_test(Config::default(), &["local", "zefat"]);
        app.widgets = sample_widgets();
        let mut harness = Harness::builder()
            .with_size([1400.0, 900.0])
            .build_ui_state(|ui, app: &mut RigtopApp| app.show(ui), app);
        harness.run();
        for label in [
            "Clock",
            "Claude Code",
            "58% left · 14:30",
            "rigtop",
            "your turn",
        ] {
            assert!(
                harness.query_all_by_label_contains(label).next().is_some(),
                "missing {label:?}"
            );
        }
    }

    #[test]
    fn hosts_window_adds_and_removes_hosts() {
        use egui::accesskit::Role;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut harness = harness_with(sample_snapshot());
        harness.state_mut().config_path = Some(path.clone());
        harness.run();
        harness.get_by_label("hosts…").click();
        harness.run();
        harness.get_by_label("add host").click();
        harness.run();
        // The first text field is the name; the others are optional.
        // The name field; the process filter is a text field too.
        let name_field = || {
            egui_kittest::kittest::by()
                .role(Role::TextInput)
                .label("Name")
        };
        harness.get(name_field()).click();
        harness.run();
        harness.get(name_field()).type_text("zefat");
        harness.run();
        harness.get_by_label("save").click();
        harness.run();

        let saved = Config::load(&path).unwrap();
        assert_eq!(saved.hosts.len(), 1);
        assert_eq!(saved.hosts[0].name, "zefat");
        assert_eq!(
            saved.hosts[0].ssh, None,
            "the destination defaults to the name"
        );
        let app = harness.state();
        let names: Vec<&str> = app.hosts.iter().map(|h| h.spec.name.as_str()).collect();
        assert_eq!(names, ["local", "zefat"]);
        assert!(
            app.hosts[0].state.snapshot.is_some(),
            "the local host keeps its data"
        );
        assert_eq!(
            app.view,
            View::Overview,
            "several hosts: the overview opens"
        );

        harness.get_by_label("remove").click();
        harness.run();
        assert!(Config::load(&path).unwrap().hosts.is_empty());
        assert_eq!(harness.state().hosts.len(), 1);
        assert_eq!(harness.state().view, View::Host(0));
    }

    #[test]
    fn sensors_window_names_and_hides_sensors() {
        use egui::accesskit::Role;
        use rigtop_core::snapshot::{Fan, SensorChip, Temperature};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut snapshot = sample_snapshot();
        snapshot.sensors = vec![SensorChip {
            key: "nct6686:nct6683.2592".to_owned(),
            name: "nct6686".to_owned(),
            temperatures: vec![Temperature {
                key: "nct6686:nct6683.2592/temp4".to_owned(),
                label: "temp4".to_owned(),
                celsius: -40.0,
                ..Default::default()
            }],
            fans: vec![Fan {
                key: "nct6686:nct6683.2592/fan2".to_owned(),
                label: "fan2".to_owned(),
                rpm: 1200,
            }],
        }];
        let mut harness = harness_with(snapshot);
        harness.state_mut().config_path = Some(path.clone());
        harness.run();
        harness.get_by_label("sensors…").click();
        harness.run();
        let fan_field = || {
            egui_kittest::kittest::by()
                .role(Role::TextInput)
                .label("fan2")
        };
        harness.get(fan_field()).click();
        harness.run();
        harness.get(fan_field()).type_text("Pump ");
        harness.run();
        // The first channel is the unconnected thermistor: hide it.
        harness.get_all_by_label("show").next().unwrap().click();
        harness.run();
        harness.get_by_label("save").click();
        harness.run();

        let saved = Config::load(&path).unwrap();
        assert_eq!(saved.label_for("nct6686:nct6683.2592/fan2"), Some("Pump"));
        assert!(saved.is_hidden("nct6686:nct6683.2592/temp4"));
        // Applied at once: the fan shows under its new name.
        assert_eq!(harness.state().primary().config.sensors, saved.sensors);
        harness.get_by_label("Pump");
    }

    /// Renders the real machine to a PNG for visual checks:
    /// `RIGTOP_GUI_SCREENSHOT=/tmp/gui.png cargo test -p rigtop-gui -- --ignored screenshot`
    ///
    /// Optional: `RIGTOP_GUI_FILTER` (process filter, e.g. to keep personal
    /// processes out of published images) and `RIGTOP_GUI_SAMPLES` (number of
    /// samples taken 300 ms apart, to fill the graphs; default 20),
    /// `RIGTOP_GUI_SIZE` (window size such as `1000x700`; default 1400x900),
    /// `RIGTOP_GUI_HOSTS` (comma-separated names for an overview) and
    /// `RIGTOP_GUI_CLICK` (comma-separated labels clicked in order, e.g.
    /// `hosts…,add host` to show the hosts window).
    #[test]
    #[ignore]
    fn screenshot_local_machine() {
        use rigtop_core::{DataSource, LocalSource};
        let path = std::env::var("RIGTOP_GUI_SCREENSHOT").expect("RIGTOP_GUI_SCREENSHOT");
        let samples: usize = std::env::var("RIGTOP_GUI_SAMPLES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        // RIGTOP_GUI_HOSTS=local,zefat,nas renders the overview with this
        // machine's data under every name.
        let names_env = std::env::var("RIGTOP_GUI_HOSTS").unwrap_or_else(|_| "local".to_owned());
        let names: Vec<&str> = names_env.split(',').collect();
        let mut source = LocalSource::new();
        let mut app = RigtopApp::for_test(Config::load_default().unwrap_or_default(), &names);
        for _ in 0..samples {
            std::thread::sleep(Duration::from_millis(300));
            let snapshot = source.poll().unwrap();
            for host in &mut app.hosts {
                host.state
                    .apply(Event::Snapshot(Box::new(snapshot.clone())));
            }
        }
        if std::env::var("RIGTOP_GUI_WIDGETS").is_ok() {
            app.widgets = sample_widgets();
        }
        if let Ok(filter) = std::env::var("RIGTOP_GUI_FILTER") {
            app.primary_mut().filter = filter;
        }
        let size = std::env::var("RIGTOP_GUI_SIZE")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?])
            })
            .unwrap_or([1400.0, 900.0]);
        let mut harness = Harness::builder()
            .with_size(size)
            .build_ui_state(|ui, app: &mut RigtopApp| app.show(ui), app);
        harness.run();
        if let Ok(clicks) = std::env::var("RIGTOP_GUI_CLICK") {
            for label in clicks.split(',') {
                harness.get_by_label(label).click();
                harness.run();
            }
        }
        harness
            .render()
            .expect("render")
            .save(&path)
            .expect("save PNG");
    }
}
