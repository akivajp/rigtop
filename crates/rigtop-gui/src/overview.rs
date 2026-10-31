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
    ui.horizontal(|ui| {
        ui.checkbox(&mut settings.include_local, "include this machine");
        ui.label(dim(
            "· click a host name for details · \"sections\" sets what each card shows",
        ));
    });
    ui.add_space(2.0);

    let mut visible: Vec<usize> = (0..hosts.len())
        .filter(|&i| settings.include_local || !hosts[i].spec.is_local())
        .collect();
    if visible
        .first()
        .is_some_and(|&i| hosts[i].state.config.overview.alerts_first)
    {
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
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for &index in row {
                        let host = &hosts[index];
                        let sections = settings.sections_for(&host.spec);
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(card_width, card_height), Sense::hover());
                        let mut card_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .layout(Layout::top_down(Align::Min))
                                .id_salt(("card", index)),
                        );
                        let response = card(&mut card_ui, host, &sections, processes);
                        if response.open {
                            open = Some(index);
                        }
                        if let Some(new_sections) = response.sections {
                            changes.push((host.spec.name.clone(), new_sections));
                        }
                    }
                });
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

fn card(
    ui: &mut Ui,
    host: &HostEntry,
    sections: &[OverviewSection],
    processes: usize,
) -> CardResponse {
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
            if let Some(error) = &host.state.connection_error {
                let note = host
                    .state
                    .offline_note()
                    .unwrap_or_else(|| "disconnected".to_owned());
                ui.label(colored(format!("{note}: {error}"), WARN));
            }
            alert_lines(ui, snapshot, &host.state);
            let graphs = graph_heights(ui.available_height(), snapshot, sections, processes);
            for &section in sections {
                if ui.available_height() < section_min_height(section) {
                    break;
                }
                match section {
                    OverviewSection::Cpu => cpu_section(ui, host, snapshot, graphs.cpu),
                    OverviewSection::Gpu => gpu_section(ui, snapshot),
                    OverviewSection::Memory => memory_section(ui, snapshot),
                    OverviewSection::Processes => process_section(ui, snapshot, processes),
                    OverviewSection::Network => network_section(ui, host, snapshot, graphs.network),
                    OverviewSection::Storage => storage_section(ui, snapshot),
                }
                ui.add_space(3.0);
            }
        });
    response
}

/// Heights of the history graphs on a card.
struct GraphHeights {
    cpu: f32,
    network: f32,
}

/// Rows the text of a section takes (graphs excluded).
fn section_rows(section: OverviewSection, snapshot: &Snapshot, processes: usize) -> usize {
    match section {
        OverviewSection::Cpu => 2,
        OverviewSection::Gpu => snapshot.gpus.len(),
        OverviewSection::Memory => 1 + usize::from(snapshot.memory.swap_total_bytes > 0),
        // Always the full height, so the sections below stay in place.
        OverviewSection::Processes => 1 + processes,
        OverviewSection::Network => 1,
        OverviewSection::Storage => snapshot.filesystems.len().min(3) + 1,
    }
}

/// Splits the height left after the text of all sections between the CPU
/// and network graphs (3:2), so tall cards show longer history instead of
/// empty space. Graphs of sections that are not shown get nothing.
fn graph_heights(
    available: f32,
    snapshot: &Snapshot,
    sections: &[OverviewSection],
    processes: usize,
) -> GraphHeights {
    let rows: usize = sections
        .iter()
        .map(|&s| section_rows(s, snapshot, processes))
        .sum();
    let text = rows as f32 * ROW + sections.len() as f32 * 3.0;
    let spare = (available - text - 8.0).max(0.0);
    let has_cpu = sections.contains(&OverviewSection::Cpu);
    let has_net = sections.contains(&OverviewSection::Network);
    let (cpu_share, net_share) = match (has_cpu, has_net) {
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
    GraphHeights {
        cpu: size(cpu_share, 400.0),
        network: size(net_share, 260.0),
    }
}

/// Rows a section needs at least; smaller cards skip it.
fn section_min_height(section: OverviewSection) -> f32 {
    match section {
        OverviewSection::Processes => ROW * 3.0,
        OverviewSection::Storage => ROW * 2.0,
        _ => ROW,
    }
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

fn cpu_section(ui: &mut Ui, host: &HostEntry, snapshot: &Snapshot, graph_height: f32) {
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

fn gpu_section(ui: &mut Ui, snapshot: &Snapshot) {
    if snapshot.gpus.is_empty() {
        return;
    }
    for gpu in &snapshot.gpus {
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

/// The top `limit` processes by CPU (fewer if the card is short), leaving
/// the rest of the card to the following sections. The rows are reserved
/// even when there are fewer processes, so later sections do not jump.
fn process_section(ui: &mut Ui, snapshot: &Snapshot, limit: usize) {
    let mut processes: Vec<_> = snapshot
        .processes
        .iter()
        .filter(|p| !p.kernel_thread)
        .collect();
    processes.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
    let rows = ((ui.available_height() / ROW).floor() as usize)
        .saturating_sub(1)
        .min(limit);
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

fn network_section(ui: &mut Ui, host: &HostEntry, snapshot: &Snapshot, graph_height: f32) {
    let history = &host.state.history;
    let Some(name) = history.net_interface.as_deref() else {
        return;
    };
    let Some(net) = snapshot.networks.iter().find(|n| n.name == name) else {
        return;
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
    }
}

fn storage_section(ui: &mut Ui, snapshot: &Snapshot) {
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
            crate::panels::zfs_state(ui, pool);
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
