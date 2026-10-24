//! Application state, the resizable tile layout and the per-frame entry point.

use std::time::{Duration, Instant};

use egui::{
    Color32, CornerRadius, FontId, Id, Margin, RichText, Sense, Stroke, TextStyle, Ui, Visuals,
};
use egui_tiles::{Container, SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse};
use rigtop_core::view::{
    MetricHistory, ProcessQuery, ProcessRow, SortKey, process_rows, signal_name,
};
use rigtop_core::worker::{Command, Event, Worker};
use rigtop_core::{Config, LocalSource, ProcessSignal, Snapshot};

use crate::panels;
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
            status: None,
        }
    }

    /// Applies a message from the worker.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Snapshot(snapshot) => {
                self.history.record(&snapshot);
                self.snapshot = Some(*snapshot);
            }
            Event::SignalResult {
                pid,
                signal,
                result,
            } => match result {
                Ok(()) => self.set_status(format!("sent {} to {pid}", signal_name(signal))),
                Err(err) => self.set_status(format!("failed: {err}")),
            },
            Event::Error(err) => self.set_status(format!("collection error: {err}")),
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

/// The eframe application.
pub struct RigtopApp {
    pub state: GuiState,
    tree: Tree<Pane>,
    /// `None` in tests, which feed snapshots directly.
    worker: Option<Worker>,
    /// Whether the theme (and restored zoom) has been applied to the context.
    styled: bool,
    /// Zoom restored from the previous run, applied on the first frame.
    initial_zoom: f32,
}

impl RigtopApp {
    /// Starts collecting on a worker thread that wakes the UI on new data.
    ///
    /// The zoom factor and panel layout of the previous run are restored from
    /// eframe's storage when available.
    pub fn new(cc: &eframe::CreationContext<'_>, config: Config, interval: Duration) -> Self {
        let source_config = config.clone();
        let repaint = cc.egui_ctx.clone();
        let worker = Worker::spawn_with_notify(
            move || LocalSource::with_config(&source_config),
            interval,
            move || repaint.request_repaint(),
        );
        let storage = cc.storage;
        let initial_zoom = storage
            .and_then(|s| eframe::get_value::<f32>(s, ZOOM_KEY))
            .map_or(1.0, |z| z.clamp(MIN_ZOOM, MAX_ZOOM));
        let tree = storage
            .and_then(|s| eframe::get_value::<Tree<Pane>>(s, LAYOUT_KEY))
            .filter(is_complete_layout)
            .unwrap_or_else(default_tree);
        Self {
            state: GuiState::new(config),
            tree,
            worker: Some(worker),
            styled: false,
            initial_zoom,
        }
    }

    /// Creates the app without a worker (snapshots come from `state.apply`).
    #[cfg(test)]
    pub fn without_worker(config: Config) -> Self {
        Self {
            state: GuiState::new(config),
            tree: default_tree(),
            worker: None,
            styled: false,
            initial_zoom: 1.0,
        }
    }

    /// Draws one frame.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.styled {
            setup_style(&ctx);
            ctx.set_zoom_factor(self.initial_zoom);
            self.styled = true;
        }
        // Ctrl + mouse wheel (or pinch) zooms the whole UI; egui already
        // handles Ctrl + "+" / "-" / "0" itself.
        let wheel_zoom = ctx.input(|i| i.zoom_delta());
        if wheel_zoom != 1.0 {
            set_zoom(&ctx, ctx.zoom_factor() * wheel_zoom);
        }
        self.state.zoom = ctx.zoom_factor();
        if let Some(worker) = &self.worker {
            while let Some(event) = worker.try_recv() {
                self.state.apply(event);
            }
        }

        egui::Frame::new()
            .fill(BACKGROUND)
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                if self.state.snapshot.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("collecting…").color(DIM))
                    });
                } else {
                    let mut behavior = Behavior {
                        state: &mut self.state,
                    };
                    self.tree.ui(&mut behavior, ui);
                }
            });
        panels::confirm_dialog(ui.ctx(), &mut self.state);

        for action in std::mem::take(&mut self.state.view_actions) {
            match action {
                ViewAction::ZoomIn => set_zoom(&ctx, ctx.zoom_factor() + ZOOM_STEP),
                ViewAction::ZoomOut => set_zoom(&ctx, ctx.zoom_factor() - ZOOM_STEP),
                ViewAction::ZoomReset => set_zoom(&ctx, 1.0),
                ViewAction::ResetLayout => self.tree = default_tree(),
            }
        }

        if let Some(worker) = &self.worker {
            for command in self.state.commands.drain(..) {
                worker.send(command);
            }
        }
        // Repaint at the next full second so the clock ticks; new data
        // triggers repaints through the worker's notification.
        let subsec = chrono::Local::now().timestamp_subsec_millis() as u64;
        ui.ctx()
            .request_repaint_after(Duration::from_millis(1000 - subsec.min(999)));
    }
}

impl eframe::App for RigtopApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, ZOOM_KEY, &self.state.zoom);
        eframe::set_value(storage, LAYOUT_KEY, &self.tree);
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
            app.state.apply(Event::Snapshot(Box::new(snapshot.clone())));
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
        assert_eq!(harness.state().state.sort_key, SortKey::Pid);
        assert!(!harness.state().state.sort_descending);

        harness.get_by_label("python").click_secondary();
        harness.run();
        harness.get_by_label("Kill (SIGKILL)").click();
        harness.run();
        // Nothing is sent until the dialog is confirmed.
        assert!(harness.state().state.commands.is_empty());
        let confirm = harness
            .state()
            .state
            .confirm
            .clone()
            .expect("confirmation dialog");
        assert_eq!((confirm.pid, confirm.signal), (4242, ProcessSignal::Kill));
        harness.get_by_label("Cancel").click();
        harness.run();
        assert!(harness.state().state.confirm.is_none());
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

    /// Renders the real machine to a PNG for visual checks:
    /// `RIGTOP_GUI_SCREENSHOT=/tmp/gui.png cargo test -p rigtop-gui -- --ignored screenshot`
    ///
    /// Optional: `RIGTOP_GUI_FILTER` (process filter, e.g. to keep personal
    /// processes out of published images) and `RIGTOP_GUI_SAMPLES` (number of
    /// samples taken 300 ms apart, to fill the graphs; default 20) and
    /// `RIGTOP_GUI_SIZE` (window size such as `1000x700`; default 1400x900).
    #[test]
    #[ignore]
    fn screenshot_local_machine() {
        use rigtop_core::{DataSource, LocalSource};
        let path = std::env::var("RIGTOP_GUI_SCREENSHOT").expect("RIGTOP_GUI_SCREENSHOT");
        let samples: usize = std::env::var("RIGTOP_GUI_SAMPLES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        let mut source = LocalSource::new();
        let mut app = RigtopApp::without_worker(Config::load_default().unwrap_or_default());
        for _ in 0..samples {
            std::thread::sleep(Duration::from_millis(300));
            app.state
                .apply(Event::Snapshot(Box::new(source.poll().unwrap())));
        }
        if let Ok(filter) = std::env::var("RIGTOP_GUI_FILTER") {
            app.state.filter = filter;
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
        harness
            .render()
            .expect("render")
            .save(&path)
            .expect("save PNG");
    }
}
