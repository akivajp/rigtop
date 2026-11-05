//! The overview: one card per host, each showing the sections the user
//! cares about most, in priority order, as far as the card has room.

use std::time::Duration;

use egui::text::LayoutJob;
use egui::{Align, Color32, FontId, Layout, RichText, Sense, TextFormat, Ui, vec2};
use rigtop_core::config::OverviewSection;
use rigtop_core::format;
use rigtop_core::snapshot::Snapshot;
use rigtop_core::view::{Severity, attention_rank};

use crate::app::{HostEntry, OverviewSettings, ViewAction};
use crate::widgets::{self, BAD, BORDER, DIM, OK, TEXT, WARN, percent_color, temperature_color};

/// Height of one text row on a card.
const ROW: f32 = 17.0;
/// Narrowest card; more hosts than fit side by side wrap to new rows.
const CARD_MIN_WIDTH: f32 = 430.0;
/// Shortest card before the overview scrolls instead of shrinking cards.
const CARD_MIN_HEIGHT: f32 = 230.0;
/// Cards shorter than this drop their lower sections (network, storage);
/// then narrower cards side by side are the better trade.
const CARD_GOOD_HEIGHT: f32 = 380.0;
/// The narrowest a card may get for that.
const CARD_TIGHT_WIDTH: f32 = 340.0;
/// Data older than this marks a host as stale.
const STALE_AFTER: Duration = Duration::from_secs(10);
/// Alerts listed on a card (the rest are summarized as "+N more").
const MAX_ALERTS: usize = 3;

fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text).color(DIM)
}

fn colored(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text).color(color)
}

/// Status color and description of a host, for dots in tabs and cards.
pub fn host_status(host: &HostEntry) -> (Color32, String) {
    let state = &host.state;
    if let Some(error) = &state.connection_error {
        return match state.offline_note() {
            Some(note) => (BAD, format!("{note}: {error}")),
            None => (BAD, error.clone()),
        };
    }
    match state.last_update {
        None => (DIM, host.waiting_message()),
        Some(at) if at.elapsed() > STALE_AFTER => {
            (WARN, format!("no data for {}s", at.elapsed().as_secs()))
        }
        Some(_) => {
            let critical = state.snapshot.as_ref().is_some_and(|s| {
                state
                    .alerts(s)
                    .iter()
                    .any(|a| a.severity == Severity::Critical)
            });
            if critical {
                (BAD, "needs attention".to_owned())
            } else {
                (OK, "up to date".to_owned())
            }
        }
    }
}

/// The host's [`attention_rank`], for ordering the overview.
fn attention(host: &HostEntry) -> u8 {
    let state = &host.state;
    let alerts = state
        .snapshot
        .as_ref()
        .map(|s| state.alerts(s))
        .unwrap_or_default();
    let stale = state
        .last_update
        .is_some_and(|at| at.elapsed() > STALE_AFTER);
    attention_rank(&alerts, state.connection_error.is_some(), stale)
}

/// A colored "●" followed by plain text, as one label text.
pub fn dot_label(color: Color32, text: &str, strong: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font = FontId::monospace(13.0);
    job.append("● ", 0.0, TextFormat::simple(font.clone(), color));
    let text_color = if strong {
        TEXT
    } else {
        TEXT.gamma_multiply(0.85)
    };
    job.append(text, 0.0, TextFormat::simple(font, text_color));
    job
}

/// "A- 100% A+" (laid out right to left: call inside a right-to-left layout).
pub fn zoom_controls(ui: &mut Ui, zoom: f32, actions: &mut Vec<ViewAction>) {
    let small = |text: &str| egui::Button::new(dim(text)).frame(false);
    if ui
        .add(small("A+"))
        .on_hover_text("zoom in (Ctrl + + / Ctrl + wheel)")
        .clicked()
    {
        actions.push(ViewAction::ZoomIn);
    }
    let percent = format!("{:.0}%", zoom * 100.0);
    if ui
        .add(small(&percent))
        .on_hover_text("reset zoom (Ctrl + 0)")
        .clicked()
    {
        actions.push(ViewAction::ZoomReset);
    }
    if ui
        .add(small("A-"))
        .on_hover_text("zoom out (Ctrl + - / Ctrl + wheel)")
        .clicked()
    {
        actions.push(ViewAction::ZoomOut);
    }
}

/// "hosts…": opens the window for adding and editing remote hosts.
pub fn hosts_button(ui: &mut Ui, actions: &mut Vec<ViewAction>) {
    if ui
        .add(egui::Button::new(dim("hosts…")).frame(false))
        .on_hover_text("add, edit or remove remote hosts")
        .clicked()
    {
        actions.push(ViewAction::EditHosts);
    }
}

/// "config…": a menu to open the configuration file (created with every
/// setting commented out if there is none) or its folder.
pub fn config_button(ui: &mut Ui, path: Option<&std::path::Path>, actions: &mut Vec<ViewAction>) {
    ui.menu_button(dim("config…"), |ui| {
        let Some(path) = path else {
            ui.label(dim("no place for a configuration file on this system"));
            return;
        };
        ui.label(dim(path.display().to_string()));
        if ui
            .button("open the file")
            .on_hover_text("in the default app; saved changes apply at once")
            .clicked()
        {
            actions.push(ViewAction::OpenConfig);
            ui.close();
        }
        if ui.button("show the folder").clicked() {
            actions.push(ViewAction::OpenConfigFolder);
            ui.close();
        }
    })
    .response
    .on_hover_text("open the configuration file");
}

/// "widgets": shows or hides the widget column, for room on small screens.
pub fn widgets_button(ui: &mut Ui, visible: bool, actions: &mut Vec<ViewAction>) {
    let hint = if visible {
        "hide the widget column"
    } else {
        "show the widget column"
    };
    if ui
        .add(egui::Button::selectable(visible, dim("widgets")))
        .on_hover_text(hint)
        .clicked()
    {
        actions.push(ViewAction::ToggleWidgets);
    }
}

/// Cards side by side: as many as fit at [`CARD_MIN_WIDTH`], one more when
/// that leaves the cards too short for their sections and they can still be
/// [`CARD_TIGHT_WIDTH`] wide (e.g. next to the widget column).
fn card_columns(cards: usize, width: f32, height: f32, gap: f32) -> usize {
    let size = |columns: usize| {
        let rows = cards.div_ceil(columns);
        (
            (width - gap * (columns - 1) as f32) / columns as f32,
            (height - gap * (rows - 1) as f32) / rows as f32,
        )
    };
    let columns = ((width / CARD_MIN_WIDTH).floor() as usize).clamp(1, cards.max(1));
    if columns < cards {
        let (_, card_height) = size(columns);
        let (wider_width, wider_height) = size(columns + 1);
        if card_height < CARD_GOOD_HEIGHT
            && wider_width >= CARD_TIGHT_WIDTH
            && wider_height > card_height
        {
            return columns + 1;
        }
    }
    columns
}

/// Draws the overview. Returns the index of a host whose card title was
/// clicked (to open its detail tab).
pub fn show(
    ui: &mut Ui,
    hosts: &mut [HostEntry],
    settings: &mut OverviewSettings,
    processes: usize,
) -> Option<usize> {
    let configured = hosts
        .first()
        .is_some_and(|h| h.state.config.overview.alerts_first);
    let mut alerts_first = settings.alerts_first.unwrap_or(configured);
    ui.horizontal(|ui| {
        ui.checkbox(&mut settings.include_local, "include this machine");
        if ui
            .checkbox(&mut alerts_first, "alerts first")
            .on_hover_text(
                "put hosts that are unreachable or have alerts first; \
                 otherwise cards keep the configured order",
            )
            .changed()
        {
            settings.alerts_first = Some(alerts_first);
        }
        ui.label(dim(
            "· click a host name for details · \"sections\" sets what each card shows",
        ));
    });
    ui.add_space(2.0);

    let mut visible: Vec<usize> = (0..hosts.len())
        .filter(|&i| settings.include_local || !hosts[i].spec.is_local())
        .collect();
    if alerts_first {
        // Stable: hosts of the same rank keep the configured order.
        visible.sort_by_key(|&i| attention(&hosts[i]));
    }
    if visible.is_empty() {
        ui.label(dim(
            "no hosts to show: add [[hosts]] to the configuration or use --host",
        ));
        return None;
    }

    let width = ui.available_width();
    let gap = 6.0;
    let columns = card_columns(visible.len(), width, ui.available_height(), gap);
    let rows = visible.len().div_ceil(columns);
    let card_width = (width - gap * (columns - 1) as f32) / columns as f32;
    let fit_height = (ui.available_height() - gap * (rows - 1) as f32) / rows as f32;
    let card_height = fit_height.max(CARD_MIN_HEIGHT);

    let mut open = None;
    let mut changes = Vec::new();
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            for row in visible.chunks(columns) {
                // The cards of a row are laid out together so their sections
                // line up.
                let sections: Vec<Vec<OverviewSection>> = row
                    .iter()
                    .map(|&i| settings.sections_for(&hosts[i].spec))
                    .collect();
                let cards: Vec<RowCard> = row
                    .iter()
                    .zip(&sections)
                    .map(|(&i, sections)| RowCard {
                        host: &hosts[i],
                        sections,
                    })
                    .collect();
                let mut planner = RowPlanner::new(ui.ctx(), cards, processes);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (position, &index) in row.iter().enumerate() {
                        let host = &hosts[index];
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(card_width, card_height), Sense::hover());
                        let mut card_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .layout(Layout::top_down(Align::Min))
                                .id_salt(("card", index)),
                        );
                        let response = card(&mut card_ui, host, &mut planner, position);
                        if response.open {
                            open = Some(index);
                        }
                        if let Some(new_sections) = response.sections {
                            changes.push((host.spec.name.clone(), new_sections));
                        }
                    }
                });
                planner.remember(ui.ctx());
                ui.add_space(gap);
            }
        });
    for (name, sections) in changes {
        match sections {
            Some(sections) => {
                settings.sections.insert(name, sections);
            }
            None => {
                settings.sections.remove(&name);
            }
        }
    }
    open
}

/// What happened on a card.
struct CardResponse {
    /// The host name was clicked.
    open: bool,
    /// New section order (`Some(None)`: back to the configured default).
    sections: Option<Option<Vec<OverviewSection>>>,
}

fn card(ui: &mut Ui, host: &HostEntry, planner: &mut RowPlanner, position: usize) -> CardResponse {
    let sections = planner.cards[position].sections;
    let mut response = CardResponse {
        open: false,
        sections: None,
    };
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.set_max_height(ui.available_height());
            let (color, status) = host_status(host);
            ui.horizontal(|ui| {
                let name = ui
                    .add(
                        egui::Label::new(dot_label(color, &host.spec.name, true))
                            .sense(Sense::click()),
                    )
                    .on_hover_text(format!("{status}\nclick for details"))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if name.clicked() {
                    response.open = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    response.sections = sections_menu(ui, host, sections);
                    if let Some(snapshot) = &host.state.snapshot {
                        ui.label(dim(format!(
                            "up {}",
                            format::duration(snapshot.host.uptime_secs)
                        )));
                        if let Some((text, slow)) = rigtop_core::view::link_rtt_text(snapshot) {
                            ui.label(if slow { colored(text, WARN) } else { dim(text) })
                                .on_hover_text(rigtop_core::view::LINK_RTT_HINT);
                        }
                        if let Some(reason) = &snapshot.host.reboot_required {
                            ui.label(colored(rigtop_core::view::REBOOT_NOTE, WARN))
                                .on_hover_text(rigtop_core::view::reboot_hint(reason));
                        }
                        if snapshot.host.hostname != host.spec.name {
                            ui.label(dim(snapshot.host.hostname.clone()));
                        }
                        if let Some(note) = rigtop_core::view::version_note(snapshot) {
                            ui.label(colored(note, WARN))
                                .on_hover_text(rigtop_core::view::version_hint(snapshot));
                        }
                    }
                });
            });

            let Some(snapshot) = &host.state.snapshot else {
                ui.label(dim(host.waiting_message()));
                if let Some(note) = host.state.offline_note() {
                    ui.label(colored(note, BAD));
                }
                if let Some(error) = &host.state.connection_error {
                    ui.label(colored(error.clone(), WARN));
                }
                return;
            };
            let plan = planner.plan(ui.available_height()).remove(position);
            // Each part is padded to the height planned for the row, and
            // what it took beyond its estimate (wrapped lines) is measured
            // for the next frame's plan.
            let pad_to = |ui: &mut Ui, bottom: f32| {
                let gap = bottom - ui.cursor().top();
                if gap > 0.0 {
                    ui.add_space(gap);
                }
            };
            let top = ui.cursor().top();
            if let Some(error) = &host.state.connection_error {
                let note = host
                    .state
                    .offline_note()
                    .unwrap_or_else(|| "disconnected".to_owned());
                ui.label(colored(format!("{note}: {error}"), WARN));
            }
            alert_lines(ui, snapshot, &host.state);
            let card = &planner.cards[position];
            let estimate = card.head_rows() as f32 * ROW;
            let mut measured = [0.0; EXTRAS];
            measured[0] = (ui.cursor().top() - top - estimate).max(0.0);
            pad_to(ui, top + plan.head);
            for slot in &plan.slots {
                let top = ui.cursor().top();
                if sections.contains(&slot.section) {
                    let layout = &plan.layout;
                    let graph = match slot.section {
                        OverviewSection::Cpu => {
                            cpu_section(ui, host, snapshot, layout.cpu, layout.cpu_model);
                            layout.cpu
                        }
                        OverviewSection::Gpu => {
                            gpu_section(ui, snapshot, layout.gpu_names, card.integrated_gpu());
                            0.0
                        }
                        OverviewSection::Memory => {
                            memory_section(ui, snapshot);
                            0.0
                        }
                        OverviewSection::Processes => {
                            process_section(ui, snapshot, plan.process_rows);
                            0.0
                        }
                        OverviewSection::Network => {
                            network_section(ui, host, snapshot, layout.network)
                        }
                        OverviewSection::Storage => {
                            storage_section(ui, snapshot, layout.disks);
                            0.0
                        }
                    };
                    let estimate =
                        card.text_rows(slot.section, layout, plan.process_rows) as f32 * ROW;
                    measured[extra_index(slot.section)] =
                        (ui.cursor().top() - top - graph - estimate).max(0.0);
                }
                pad_to(ui, top + slot.text + slot.graph + SECTION_GAP);
            }
            planner.measured[position] = measured;
        });
    response
}

/// What a card shows beyond the essentials, by the room it has: detail
/// rows first, then the history graphs.
#[derive(Debug, Clone, Default, PartialEq)]
struct CardLayout {
    /// Heights of the CPU and network history graphs (0: none).
    cpu: f32,
    network: f32,
    /// The CPU model under its meter.
    cpu_model: bool,
    /// Each GPU's name and power above its meter.
    gpu_names: bool,
    /// Each disk's I/O under the filesystems.
    disks: bool,
}

/// Disks listed with their I/O when a card has room.
const MAX_DISKS: usize = 4;
/// Space after each section.
const SECTION_GAP: f32 = 3.0;
/// Measured extra heights of a card: its alert lines, then each section.
const EXTRAS: usize = 1 + OverviewSection::ALL.len();
type Extras = [f32; EXTRAS];

/// Where a section's extra height is kept in [`Extras`].
fn extra_index(section: OverviewSection) -> usize {
    1 + OverviewSection::ALL
        .iter()
        .position(|&s| s == section)
        .unwrap_or(0)
}

/// One card of a row of cards laid out together.
struct RowCard<'a> {
    host: &'a HostEntry,
    sections: &'a [OverviewSection],
}

impl RowCard<'_> {
    fn snapshot(&self) -> Option<&Snapshot> {
        self.host.state.snapshot.as_ref()
    }

    /// How this host's integrated GPU is shown (the viewer's setting).
    fn integrated_gpu(&self) -> rigtop_core::config::IntegratedGpu {
        self.host.state.config.gpu.integrated
    }

    /// Lines above the sections: a lost connection and the alerts.
    fn head_rows(&self) -> usize {
        let Some(snapshot) = self.snapshot() else {
            return 0;
        };
        let alerts = self.host.state.alerts(snapshot).len();
        usize::from(self.host.state.connection_error.is_some())
            + alerts.min(MAX_ALERTS)
            + usize::from(alerts > MAX_ALERTS)
    }

    /// Rows the text of a section takes on this card (graphs excluded);
    /// 0 if the card does not show it.
    fn text_rows(&self, section: OverviewSection, layout: &CardLayout, processes: usize) -> usize {
        let Some(snapshot) = self.snapshot() else {
            return 0;
        };
        if !self.sections.contains(&section) {
            return 0;
        }
        match section {
            OverviewSection::Cpu => {
                2 + usize::from(layout.cpu_model && !snapshot.cpu.model.trim().is_empty())
            }
            OverviewSection::Gpu => {
                rigtop_core::view::card_gpus(snapshot, self.integrated_gpu()).count()
                    * (1 + usize::from(layout.gpu_names))
            }
            OverviewSection::Memory => 1 + usize::from(snapshot.memory.swap_total_bytes > 0),
            // Always the full height, so the sections below stay in place.
            OverviewSection::Processes => 1 + processes,
            OverviewSection::Network => usize::from(self.has_network(snapshot)),
            OverviewSection::Storage => {
                let disks = if layout.disks {
                    snapshot.disks.len().min(MAX_DISKS)
                } else {
                    0
                };
                snapshot.filesystems.len().min(3) + disks + 1
            }
        }
    }

    /// Whether the network section has an interface to show.
    fn has_network(&self, snapshot: &Snapshot) -> bool {
        self.host
            .state
            .history
            .net_interface
            .as_deref()
            .is_some_and(|name| snapshot.networks.iter().any(|n| n.name == name))
    }

    /// The layout with the details of `section` turned on, if it has any.
    fn with_details(&self, section: OverviewSection, layout: &CardLayout) -> Option<CardLayout> {
        let snapshot = self.snapshot()?;
        if !self.sections.contains(&section) {
            return None;
        }
        let mut detailed = layout.clone();
        match section {
            OverviewSection::Cpu if !snapshot.cpu.model.trim().is_empty() => {
                detailed.cpu_model = true
            }
            OverviewSection::Gpu
                if rigtop_core::view::card_gpus(snapshot, self.integrated_gpu())
                    .next()
                    .is_some() =>
            {
                detailed.gpu_names = true
            }
            OverviewSection::Storage if !snapshot.disks.is_empty() => detailed.disks = true,
            _ => return None,
        }
        Some(detailed)
    }
}

/// A section's place on the cards of a row: the height of its text on the
/// card that needs the most, then its graph.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    section: OverviewSection,
    text: f32,
    graph: f32,
}

/// How one card of a row is laid out.
#[derive(Debug, Clone, PartialEq)]
struct CardPlan {
    /// Height kept for the alert lines.
    head: f32,
    slots: Vec<Slot>,
    layout: CardLayout,
    /// Process rows shown under the header row.
    process_rows: usize,
}

/// Plans the cards of a row once (on the first card drawn, which knows the
/// height left under the title) and collects what each card measured.
struct RowPlanner<'a> {
    cards: Vec<RowCard<'a>>,
    processes: usize,
    /// Extra heights each card measured on the previous frame.
    extras: Vec<Extras>,
    /// Extra heights each card measures on this frame.
    measured: Vec<Extras>,
    plans: Option<Vec<CardPlan>>,
}

impl<'a> RowPlanner<'a> {
    fn new(ctx: &egui::Context, cards: Vec<RowCard<'a>>, processes: usize) -> Self {
        let extras: Vec<Extras> = cards
            .iter()
            .map(|card| {
                ctx.data(|d| d.get_temp(extras_id(card.host)))
                    .unwrap_or([0.0; EXTRAS])
            })
            .collect();
        let measured = vec![[0.0; EXTRAS]; cards.len()];
        Self {
            cards,
            processes,
            extras,
            measured,
            plans: None,
        }
    }

    /// The plan of every card, given the height under the cards' titles.
    fn plan(&mut self, available: f32) -> Vec<CardPlan> {
        self.plans
            .get_or_insert_with(|| row_plan(available, &self.cards, self.processes, &self.extras))
            .clone()
    }

    /// Keeps what the cards measured for the next frame.
    fn remember(&self, ctx: &egui::Context) {
        for (card, measured) in self.cards.iter().zip(&self.measured) {
            if card.snapshot().is_some() {
                ctx.data_mut(|d| d.insert_temp(extras_id(card.host), *measured));
            }
        }
    }
}

fn extras_id(host: &HostEntry) -> egui::Id {
    egui::Id::unique(("overview_extras", host.spec.name.as_str()))
}

/// The order of the sections across `cards` (with data), if every card
/// lists them in an order consistent with it.
fn section_order(cards: &[RowCard]) -> Option<Vec<OverviewSection>> {
    let cards: Vec<&RowCard> = cards.iter().filter(|c| c.snapshot().is_some()).collect();
    let mut order: Vec<OverviewSection> = Vec::new();
    for card in &cards {
        for &section in card.sections {
            if !order.contains(&section) {
                order.push(section);
            }
        }
    }
    let consistent = cards.iter().all(|card| {
        let positions: Vec<usize> = card
            .sections
            .iter()
            .filter_map(|s| order.iter().position(|o| o == s))
            .collect();
        positions.windows(2).all(|w| w[0] < w[1])
    });
    consistent.then_some(order)
}

/// Lays out the cards of a row, `available` high under their titles, so
/// that each section starts at the same height on every card: a section
/// takes the height it needs on the card that needs the most, the others
/// are padded. Whole sections are added while they fit (the process list
/// may be shortened); the height left goes to detail rows (CPU model, GPU
/// names, disk I/O) section by section, then to the CPU and network graphs
/// (3:2), the same on every card of the row.
fn row_plan(
    available: f32,
    cards: &[RowCard],
    processes: usize,
    extras: &[Extras],
) -> Vec<CardPlan> {
    let Some(order) = section_order(cards) else {
        // Sections in different orders cannot line up: each card alone.
        return cards
            .iter()
            .zip(extras)
            .flat_map(|(card, extra)| {
                row_plan(
                    available,
                    std::slice::from_ref(card),
                    processes,
                    std::slice::from_ref(extra),
                )
            })
            .collect();
    };
    let shown = |card: &RowCard| card.snapshot().is_some();
    let head = cards
        .iter()
        .zip(extras)
        .filter(|(card, _)| shown(card))
        .map(|(card, extra)| card.head_rows() as f32 * ROW + extra[0])
        .fold(0.0, f32::max);
    let mut layouts = vec![CardLayout::default(); cards.len()];
    // A card's text height for a section, with its measured extra.
    let text = |card: &RowCard, extra: &Extras, section, layout: &CardLayout, rows| {
        let lines = card.text_rows(section, layout, rows);
        if lines == 0 {
            0.0
        } else {
            lines as f32 * ROW + extra[extra_index(section)]
        }
    };

    let mut slots: Vec<Slot> = Vec::new();
    let mut used = head;
    let mut process_rows = processes;
    for section in order {
        let left = available - used;
        if section == OverviewSection::Processes {
            // Fewer processes when short, but at least two.
            let fit = ((left - SECTION_GAP) / ROW).floor().max(0.0) as usize;
            process_rows = processes.min(fit.saturating_sub(1));
            if process_rows < 2 {
                break;
            }
        }
        let height = cards
            .iter()
            .zip(extras)
            .zip(&layouts)
            .map(|((card, extra), layout)| text(card, extra, section, layout, process_rows))
            .fold(0.0, f32::max);
        if height == 0.0 {
            continue;
        }
        if height + SECTION_GAP > left {
            break;
        }
        used += height + SECTION_GAP;
        slots.push(Slot {
            section,
            text: height,
            graph: 0.0,
        });
    }

    let mut spare = (available - used - 8.0).max(0.0);
    for slot in &mut slots {
        let detailed: Vec<Option<(CardLayout, f32)>> = cards
            .iter()
            .zip(extras)
            .zip(&layouts)
            .map(|((card, extra), layout)| {
                let detailed = card.with_details(slot.section, layout)?;
                let height = text(card, extra, slot.section, &detailed, process_rows);
                Some((detailed, height))
            })
            .collect();
        let full = detailed
            .iter()
            .flatten()
            .map(|(_, height)| *height)
            .fold(slot.text, f32::max);
        let grow = full - slot.text;
        if grow <= spare {
            spare -= grow;
            slot.text = full;
        }
        // Details that fit the section's height, even if it cannot grow:
        // a card with less text than the tallest uses its padding.
        for (layout, detailed) in layouts.iter_mut().zip(detailed) {
            if let Some((detailed, height)) = detailed
                && height <= slot.text
            {
                *layout = detailed;
            }
        }
    }

    let has = |wanted| slots.iter().any(|slot: &Slot| slot.section == wanted);
    let has_net = has(OverviewSection::Network);
    let (cpu_share, net_share) = match (has(OverviewSection::Cpu), has_net) {
        (true, true) => (0.6, 0.4),
        (true, false) => (1.0, 0.0),
        (false, true) => (0.0, 1.0),
        (false, false) => (0.0, 0.0),
    };
    // Below ~18 px a graph is just noise; above these caps it adds little.
    let size = |share: f32, cap: f32| {
        let height = (spare * share).min(cap);
        if height >= 18.0 { height } else { 0.0 }
    };
    let (cpu_graph, net_graph) = (size(cpu_share, 400.0), size(net_share, 260.0));
    for slot in &mut slots {
        slot.graph = match slot.section {
            OverviewSection::Cpu => cpu_graph,
            OverviewSection::Network => net_graph,
            _ => 0.0,
        };
    }
    cards
        .iter()
        .zip(layouts)
        .map(|(card, mut layout)| {
            if card.sections.contains(&OverviewSection::Cpu) {
                layout.cpu = cpu_graph;
            }
            if card.snapshot().is_some_and(|s| card.has_network(s)) {
                layout.network = net_graph;
            }
            CardPlan {
                head,
                slots: slots.clone(),
                layout,
                process_rows,
            }
        })
        .collect()
}

/// Lets the user show, hide and reorder sections for one host.
fn sections_menu(
    ui: &mut Ui,
    host: &HostEntry,
    current: &[OverviewSection],
) -> Option<Option<Vec<OverviewSection>>> {
    let mut result = None;
    ui.menu_button(dim("sections"), |ui| {
        ui.label(dim(format!("{}: shown top to bottom", host.spec.name)));
        let mut order: Vec<OverviewSection> = current.to_vec();
        // Hidden sections are listed after the shown ones.
        let hidden: Vec<OverviewSection> = OverviewSection::ALL
            .into_iter()
            .filter(|s| !current.contains(s))
            .collect();
        let mut changed = false;
        for (position, section) in current.iter().copied().enumerate() {
            ui.horizontal(|ui| {
                let mut shown = true;
                if ui.checkbox(&mut shown, section.label()).changed() {
                    order.retain(|s| *s != section);
                    changed = true;
                }
                if position > 0 && ui.small_button("▲").clicked() {
                    order.swap(position, position - 1);
                    changed = true;
                }
                if position + 1 < current.len() && ui.small_button("▼").clicked() {
                    order.swap(position, position + 1);
                    changed = true;
                }
            });
        }
        for section in hidden {
            let mut shown = false;
            if ui.checkbox(&mut shown, dim(section.label())).changed() {
                order.push(section);
                changed = true;
            }
        }
        ui.separator();
        if ui.button("reset to the configured order").clicked() {
            result = Some(None);
            ui.close();
        }
        if changed {
            result = Some(Some(order));
        }
    });
    result
}

fn alert_lines(ui: &mut Ui, snapshot: &Snapshot, state: &crate::app::GuiState) {
    let alerts = state.alerts(snapshot);
    for alert in alerts.iter().take(MAX_ALERTS) {
        let color = match alert.severity {
            Severity::Critical => BAD,
            Severity::Warning => WARN,
        };
        ui.label(colored(format!("▲ {}", alert.text), color));
    }
    if alerts.len() > MAX_ALERTS {
        ui.label(colored(
            format!("  +{} more", alerts.len() - MAX_ALERTS),
            WARN,
        ));
    }
}

/// "Label ■■■■■■ value" on one row, with the meter filling the middle.
fn meter_row(ui: &mut Ui, label: &str, ratio: f32, value: RichText) {
    ui.horizontal(|ui| {
        ui.add_sized([44.0, ROW - 2.0], egui::Label::new(dim(label)));
        let value_width = 150.0;
        let width = (ui.available_width() - value_width).max(30.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW - 2.0), Sense::hover());
        widgets::paint_meter(ui, rect, ratio);
        ui.label(value);
    });
}

fn cpu_section(ui: &mut Ui, host: &HostEntry, snapshot: &Snapshot, graph_height: f32, model: bool) {
    let cpu = &snapshot.cpu;
    meter_row(
        ui,
        "CPU",
        cpu.total_percent / 100.0,
        colored(
            format!(
                "{:>3.0}%  {} cores",
                cpu.total_percent,
                cpu.per_core_percent.len()
            ),
            percent_color(cpu.total_percent),
        ),
    );
    if model {
        ui.add(egui::Label::new(dim(cpu.model.trim())).truncate());
    }
    ui.horizontal_wrapped(|ui| {
        for temp in &cpu.temperatures {
            if host.state.config.is_hidden(&temp.key) {
                continue;
            }
            ui.label(dim(host.state.config.temperature_label(temp)));
            ui.label(colored(
                format!("{:.0}°C", temp.celsius),
                temperature_color(temp.celsius),
            ));
        }
        let power = &snapshot.power;
        if let Some(total) = power.total_w {
            ui.label(dim(" Σ"));
            ui.label(format!("{total:.0}W"));
        }
        if let Some(battery) = &power.battery {
            ui.label(dim(" bat"));
            ui.label(battery.summary()).on_hover_text(battery.details());
        }
        if let Some(load) = cpu.load_average {
            ui.label(dim(format!(" load {:.2}", load[0])));
        }
    });
    // Usage history, as tall as the card's spare room allows.
    if graph_height > 0.0 {
        let width = ui.available_width();
        let state = &host.state;
        let samples = state
            .history
            .cpu
            .graph(width as usize, state.graph_span, state.interval);
        widgets::area_graph(
            ui,
            vec2(width, graph_height),
            &samples,
            100.0,
            widgets::heat_color,
            false,
        );
    }
}

fn gpu_section(
    ui: &mut Ui,
    snapshot: &Snapshot,
    names: bool,
    integrated: rigtop_core::config::IntegratedGpu,
) {
    for gpu in rigtop_core::view::card_gpus(snapshot, integrated) {
        if names {
            ui.horizontal(|ui| {
                ui.add_space(48.0);
                ui.add(egui::Label::new(dim(rigtop_core::view::gpu_detail(gpu))).truncate());
            });
        }
        let util = gpu.utilization_percent.unwrap_or(0.0);
        let mut value = format::percent(gpu.utilization_percent);
        if let (Some(used), Some(total)) = (gpu.memory_used_bytes, gpu.memory_total_bytes) {
            value.push_str(&format!(
                " {}/{}",
                format::bytes(used),
                format::bytes(total)
            ));
        }
        if let Some(t) = gpu.temperature_c {
            value.push_str(&format!(" {t:.0}°C"));
        }
        meter_row(
            ui,
            &format!("GPU{}", gpu.index),
            util / 100.0,
            colored(value, percent_color(util)),
        );
    }
}

fn memory_section(ui: &mut Ui, snapshot: &Snapshot) {
    let memory = &snapshot.memory;
    let ratio = memory.used_bytes as f32 / memory.total_bytes.max(1) as f32;
    meter_row(
        ui,
        "Mem",
        ratio,
        RichText::new(format!(
            "{}/{} {:.0}%",
            format::bytes(memory.used_bytes),
            format::bytes(memory.total_bytes),
            ratio * 100.0
        )),
    );
    // Shown whenever there is swap (not only when used), so the card does
    // not change height as swap usage comes and goes.
    if memory.swap_total_bytes > 0 {
        let swap = memory.swap_used_bytes as f32 / memory.swap_total_bytes as f32;
        meter_row(
            ui,
            "Swap",
            swap,
            RichText::new(format!("{:.0}%", swap * 100.0)),
        );
    }
}

/// The top `rows` processes by CPU. The rows are reserved even when there
/// are fewer processes, so later sections do not jump.
fn process_section(ui: &mut Ui, snapshot: &Snapshot, rows: usize) {
    let mut processes: Vec<_> = snapshot
        .processes
        .iter()
        .filter(|p| !p.kernel_thread)
        .collect();
    processes.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
    let has_vram = snapshot.gpus.iter().any(|g| !g.processes.is_empty());
    egui::Grid::new(ui.next_auto_id())
        .num_columns(4)
        .spacing([10.0, 0.0])
        .striped(false)
        .show(ui, |ui| {
            ui.label(dim("Program"));
            ui.label(dim("Cpu%"));
            ui.label(dim("Mem"));
            ui.label(dim(if has_vram { "VRAM" } else { "" }));
            ui.end_row();
            for process in processes.iter().take(rows) {
                ui.add(egui::Label::new(format::truncate(&process.name, 18)));
                let cpu = process.cpu_percent;
                ui.label(colored(
                    format!("{cpu:.1}"),
                    if cpu > 0.0 { percent_color(cpu) } else { DIM },
                ));
                ui.label(format::bytes(process.memory_bytes));
                let vram: u64 = snapshot
                    .gpus
                    .iter()
                    .flat_map(|g| g.processes.iter())
                    .filter(|g| g.pid == process.pid)
                    .filter_map(|g| g.memory_bytes)
                    .sum();
                ui.label(colored(
                    if vram > 0 {
                        format::bytes(vram)
                    } else {
                        String::new()
                    },
                    widgets::GPU_TITLE,
                ));
                ui.end_row();
            }
            // Blank rows up to the reserved height.
            for _ in processes.len().min(rows)..rows {
                ui.label(" ");
                ui.end_row();
            }
        });
}

/// The network line and its graph; the height of the graph drawn.
fn network_section(ui: &mut Ui, host: &HostEntry, snapshot: &Snapshot, graph_height: f32) -> f32 {
    let history = &host.state.history;
    let Some(name) = history.net_interface.as_deref() else {
        return 0.0;
    };
    let Some(net) = snapshot.networks.iter().find(|n| n.name == name) else {
        return 0.0;
    };
    ui.horizontal(|ui| {
        ui.add_sized([44.0, ROW - 2.0], egui::Label::new(dim("Net")));
        ui.label(format::truncate(name, 14));
        if let Some(speed) = net.link_speed_mbps {
            ui.label(dim(format::link_speed(speed as f64)));
        }
        ui.label(format!(
            "▼ {}  ▲ {}",
            format::rate(net.rx_bytes_per_sec),
            format::rate(net.tx_bytes_per_sec)
        ));
    });
    if graph_height > 0.0 {
        let width = ui.available_width();
        let (span, interval) = (host.state.graph_span, host.state.interval);
        let rx = history.net_rx.graph(width as usize, span, interval);
        let tx = history.net_tx.graph(width as usize, span, interval);
        widgets::area_graph(
            ui,
            vec2(width, graph_height),
            &rx,
            crate::panels::graph_max(&rx, &tx),
            widgets::rx_color,
            false,
        );
        return graph_height;
    }
    0.0
}

/// One disk's I/O: "nvme0n1  R 1.2M/s  W 40K/s  3%  0.4ms  52°C".
fn disk_row(ui: &mut Ui, disk: &rigtop_core::snapshot::DiskIo) {
    ui.horizontal(|ui| {
        ui.add_sized([64.0, ROW - 2.0], egui::Label::new(&disk.name).truncate());
        ui.label(dim("R"));
        ui.label(format!("{:>8}", format::rate(disk.read_bytes_per_sec)));
        ui.label(dim("W"));
        ui.label(format!("{:>8}", format::rate(disk.write_bytes_per_sec)));
        if let Some(busy) = disk.busy_percent {
            ui.label(colored(format!("{busy:>3.0}%"), percent_color(busy)));
        }
        if let Some(ms) = disk.latency_ms {
            let color = match rigtop_core::view::latency_severity(ms) {
                Some(Severity::Critical) => BAD,
                Some(Severity::Warning) => WARN,
                None => DIM,
            };
            ui.label(colored(format!("{:>5}", format::latency(ms)), color));
        }
        if let Some(t) = disk.temperature_c {
            ui.label(colored(format!("{t:.0}°C"), temperature_color(t)));
        }
    });
}

fn storage_section(ui: &mut Ui, snapshot: &Snapshot, disks: bool) {
    // Fullest filesystems first.
    let mut filesystems: Vec<_> = snapshot.filesystems.iter().collect();
    filesystems.sort_by(|a, b| {
        let used = |f: &rigtop_core::snapshot::FilesystemUsage| {
            f.total_bytes.saturating_sub(f.available_bytes) as f64 / f.total_bytes.max(1) as f64
        };
        used(b).total_cmp(&used(a))
    });
    for fs in filesystems.iter().take(3) {
        let used = fs.total_bytes.saturating_sub(fs.available_bytes);
        let ratio = used as f32 / fs.total_bytes.max(1) as f32;
        let name = fs.mount_points.first().cloned().unwrap_or_default();
        meter_row(
            ui,
            &format::truncate(&name, 6),
            ratio,
            colored(
                format!(
                    "{:.0}% {} free",
                    ratio * 100.0,
                    format::bytes(fs.available_bytes)
                ),
                percent_color(ratio * 100.0),
            ),
        );
    }
    if disks {
        for disk in snapshot.disks.iter().take(MAX_DISKS) {
            disk_row(ui, disk);
        }
    }
    // One-line health summary: SMART, btrfs, md.
    ui.horizontal_wrapped(|ui| {
        if !snapshot.smart.is_empty() {
            let ok = snapshot
                .smart
                .iter()
                .filter(|s| s.error.is_none() && !s.needs_attention())
                .count();
            let color = if ok == snapshot.smart.len() { OK } else { BAD };
            ui.label(dim("SMART"));
            ui.label(colored(format!("{ok}/{} ok", snapshot.smart.len()), color));
        }
        for fs in &snapshot.raid.btrfs {
            let errors: u64 = fs.device_stats.iter().map(|d| d.total_errors()).sum();
            let profile = fs
                .allocations
                .iter()
                .find(|a| a.kind == "data")
                .map_or("", |a| a.profile.as_str());
            ui.label(dim(format!("btrfs {profile}")));
            ui.label(if errors > 0 || fs.has_problems() {
                colored(format!("{errors} errors"), BAD)
            } else {
                colored("ok", OK)
            });
        }
        for md in &snapshot.raid.md {
            ui.label(dim(md.name.clone()));
            ui.label(if md.is_degraded() {
                colored("degraded", BAD)
            } else {
                colored("ok", OK)
            });
        }
        for pool in &snapshot.raid.zfs {
            ui.label(dim(format!("zfs {}", pool.name)));
            let errors: u64 = pool.troubled_devices().map(|d| d.errors()).sum();
            let data_errors = pool.data_errors.unwrap_or(0);
            if data_errors > 0 {
                ui.label(colored(format!("{data_errors} data errors"), BAD));
            } else if errors > 0 && pool.is_online() {
                ui.label(colored(format!("{errors} errors"), BAD));
            } else {
                crate::panels::zfs_state(ui, pool);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_columns_trade_width_for_height_when_cramped() {
        // Wide window: as many 430px cards as fit.
        assert_eq!(card_columns(3, 1600.0, 880.0, 6.0), 3);
        // Next to the widget column: one column would leave 3 short cards.
        assert_eq!(card_columns(3, 770.0, 880.0, 6.0), 2);
        // Tall enough for one column: keep it.
        assert_eq!(card_columns(2, 770.0, 1000.0, 6.0), 1);
        // Too narrow for two cards.
        assert_eq!(card_columns(3, 600.0, 880.0, 6.0), 1);
    }
}
