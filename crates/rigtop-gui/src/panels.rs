//! Contents of each panel. They read the latest snapshot from [`GuiState`]
//! and queue user actions (signals) as commands.

use std::time::{SystemTime, UNIX_EPOCH};

use egui::{Align, Color32, FontId, Layout, RichText, Sense, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use rigtop_core::ProcessSignal;
use rigtop_core::format;
use rigtop_core::snapshot::{
    BtrfsFilesystem, GpuStats, NetworkStats, SmartDevice, Snapshot, device_matches_disk,
};
use rigtop_core::view::{LHM_HINT, SortKey, is_virtual_interface, lhm_help, signal_name};
use rigtop_core::worker::Command;

use crate::app::{GuiState, Pane, ViewAction};
use crate::widgets::{self, BAD, DIM, OK, TEXT, WARN, percent_color, temperature_color};

/// Height of one text row in the custom-painted parts.
const ROW_HEIGHT: f32 = 16.0;
/// Per-core cell styles, richest first: (narrowest cell width, has a meter).
/// The grid uses the first one whose rows fit in the panel height.
const CORE_CELL_STYLES: [(f32, bool); 4] = [
    (150.0, true),
    (110.0, true),
    (70.0, false),
    // The tightest "C12 34%" cell, for many cores in a short, narrow panel.
    (56.0, false),
];
/// SMART results older than this are flagged as stale.
const SMART_STALE_SECS: u64 = 3600;
/// Scrubs older than this are flagged (a monthly scrub is the common advice).
const SCRUB_STALE_SECS: u64 = 35 * 86_400;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text).color(DIM)
}

fn colored(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text).color(color)
}

/// Extra information on the right of a panel title.
pub fn title_extras(ui: &mut Ui, pane: Pane, state: &mut GuiState) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    match pane {
        Pane::Cpu => {
            let host = &snapshot.host;
            let uptime = format!("up {}", format::duration(host.uptime_secs));
            let hostname = host.hostname.clone();
            let title_end = ui.cursor().left();
            let controls_start = ui
                .with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(dim(uptime));
                    ui.label(colored(hostname, TEXT));
                    ui.add_space(12.0);
                    view_controls(ui, state);
                    ui.min_rect().left()
                })
                .inner;
            // Clock in the middle of the title bar, like btop, when the
            // title and the controls leave room for it.
            let font = FontId::monospace(14.0);
            let half = ui.fonts_mut(|f| f.glyph_width(&font, '0')) * 4.0 + 6.0;
            let rect = ui.max_rect();
            if rect.center().x - half > title_end && rect.center().x + half < controls_start {
                let clock = chrono::Local::now().format("%H:%M:%S").to_string();
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    clock,
                    font,
                    TEXT,
                );
            }
        }
        Pane::Memory => {
            let total = snapshot.memory.total_bytes;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(dim(format!("total {}", format::bytes(total))));
            });
        }
        Pane::Network => network_title(ui, state),
        Pane::Processes => {
            let shown = state.process_rows().len();
            let total = snapshot.process_count;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(dim(format!("{shown} of {total}")));
            });
        }
        Pane::Gpu | Pane::Storage => {}
    }
}

/// Zoom buttons and layout reset (right-to-left layout: drawn right first).
fn view_controls(ui: &mut Ui, state: &mut GuiState) {
    let small = |text: &str| egui::Button::new(RichText::new(text).color(DIM)).frame(false);
    if ui
        .add(small("reset layout"))
        .on_hover_text("restore the default panel layout")
        .clicked()
    {
        state.view_actions.push(ViewAction::ResetLayout);
    }
    if state.show_zoom {
        crate::overview::zoom_controls(ui, state.zoom, &mut state.view_actions);
        // Without a tab bar (a single host), the hosts window opens here.
        crate::overview::hosts_button(ui, &mut state.view_actions);
        if let Some(visible) = state.widgets_visible {
            crate::overview::widgets_button(ui, visible, &mut state.view_actions);
        }
    }
}

/// Draws the body of a panel.
pub fn pane_contents(ui: &mut Ui, pane: Pane, state: &mut GuiState) {
    if state.snapshot.is_none() {
        return;
    }
    match pane {
        Pane::Cpu => {
            if cpu(ui, state) {
                state.lhm_setup.open();
            }
        }
        Pane::Memory => memory(ui, state),
        Pane::Gpu => gpus(ui, state),
        Pane::Storage => storage(ui, state),
        Pane::Network => network(ui, state),
        Pane::Processes => processes(ui, state),
    }
}

// ---------------------------------------------------------------- CPU

/// Returns true when the LibreHardwareMonitor setup was asked for.
fn cpu(ui: &mut Ui, state: &GuiState) -> bool {
    let Some(snapshot) = &state.snapshot else {
        return false;
    };
    let cpu = &snapshot.cpu;
    let area = ui.available_rect_before_wrap();
    let graph_width = (area.width() * 0.38).max(120.0);
    // Two label rows (load and temperatures, power and fans) that wrap on a
    // narrow panel: reserve as many lines as they need, estimated from
    // their length, so the last one is not cut off.
    let info_width = (area.width() - graph_width - 12.0).max(1.0);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let glyph = ui.fonts_mut(|f| f.glyph_width(&font, '0')).max(1.0);
    let lines = |chars: usize| (chars as f32 * glyph / info_width).ceil().max(1.0);
    let fans = snapshot.sensors.iter().map(|c| c.fans.len()).sum::<usize>();
    let first = 20 + cpu.temperatures.len() * 11;
    let second = 32
        + fans * 15
        + if snapshot.power.battery.is_some() {
            16
        } else {
            0
        };
    let info_height = (lines(first) + lines(second)) * (ROW_HEIGHT + 3.0) + 4.0;
    let graph_rect = egui::Rect::from_min_size(area.min, vec2(graph_width, area.height()));
    let grid_rect = egui::Rect::from_min_max(
        pos2(graph_rect.right() + 12.0, area.top()),
        pos2(area.right(), area.bottom() - info_height),
    );
    ui.allocate_rect(area, Sense::hover());

    // Total usage graph with the current value and model on top.
    let samples = state.history.cpu.last(graph_width as usize);
    widgets::paint_area_graph(ui, graph_rect, &samples, 100.0, widgets::heat_color, false);
    ui.painter().text(
        graph_rect.left_top(),
        egui::Align2::LEFT_TOP,
        format!("{:.0}%", cpu.total_percent),
        FontId::monospace(13.0),
        percent_color(cpu.total_percent),
    );
    // The model gets the rest of the graph's width, cut to fit, so it never
    // runs into the core grid of a narrow panel.
    let model_font = FontId::monospace(12.0);
    let glyph = ui.fonts_mut(|f| f.glyph_width(&model_font, 'M')).max(1.0);
    let model_chars = ((graph_width - 52.0) / glyph).floor().max(0.0) as usize;
    ui.painter().text(
        graph_rect.left_top() + vec2(48.0, 0.0),
        egui::Align2::LEFT_TOP,
        format::truncate(&cpu.model, model_chars),
        model_font,
        DIM,
    );

    let grid_height = core_grid(ui, grid_rect, &cpu.per_core_percent, &cpu.per_core_mhz);

    // Load and power go right under the grid, so spare height ends up at
    // the bottom rather than as a gap in the middle.
    let info_rect = egui::Rect::from_min_max(
        pos2(grid_rect.left(), grid_rect.top() + grid_height + 6.0),
        area.max,
    );
    let mut info = ui.new_child(egui::UiBuilder::new().max_rect(info_rect));
    cpu_info(&mut info, state, snapshot)
}

/// Layout of the per-core grid in a `width` x `height` area.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CoreGrid {
    columns: usize,
    rows: usize,
    cell_width: f32,
    meters: bool,
}

/// Chooses the richest cell style whose rows fit in the height, using as
/// few columns as the height allows so that meters stay long. When even the
/// compact style does not fit, it is used anyway (the grid is clipped).
fn core_grid_layout(cores: usize, width: f32, height: f32) -> CoreGrid {
    let max_rows = (height / ROW_HEIGHT).floor().max(1.0) as usize;
    let layout = |(min_width, meters): (f32, bool)| {
        let max_columns = (width / min_width).floor().max(1.0) as usize;
        let columns = cores.div_ceil(max_rows).clamp(1, max_columns);
        let rows = cores.div_ceil(columns);
        CoreGrid {
            columns,
            rows,
            cell_width: width / columns as f32,
            meters,
        }
    };
    CORE_CELL_STYLES
        .into_iter()
        .map(layout)
        .find(|grid| grid.rows <= max_rows)
        .unwrap_or_else(|| layout(CORE_CELL_STYLES[CORE_CELL_STYLES.len() - 1]))
}

/// Per-core meters, column-major like btop. Returns the height used.
fn core_grid(ui: &Ui, rect: egui::Rect, cores: &[f32], mhz: &[u64]) -> f32 {
    if cores.is_empty() || rect.width() < 60.0 || rect.height() < ROW_HEIGHT {
        return 0.0;
    }
    let grid = core_grid_layout(cores.len(), rect.width(), rect.height());
    let painter = ui.painter_at(rect);
    let font = FontId::monospace(12.0);
    let show_freq = grid.meters && grid.cell_width > 230.0 && !mhz.is_empty();
    for (index, &usage) in cores.iter().enumerate() {
        let (col, row) = (index / grid.rows, index % grid.rows);
        let x = rect.left() + col as f32 * grid.cell_width;
        let y = rect.top() + row as f32 * ROW_HEIGHT;
        painter.text(
            pos2(x, y),
            egui::Align2::LEFT_TOP,
            format!("C{index}"),
            font.clone(),
            DIM,
        );
        let percent = format!("{usage:.0}%");
        if !grid.meters {
            // Compact cell: "C12  34%".
            painter.text(
                pos2(x + grid.cell_width - 8.0, y),
                egui::Align2::RIGHT_TOP,
                percent,
                font.clone(),
                percent_color(usage),
            );
            continue;
        }
        let value_width = if show_freq { 92.0 } else { 44.0 };
        let meter_rect = egui::Rect::from_min_max(
            pos2(x + 34.0, y + 2.0),
            pos2(
                x + grid.cell_width - value_width - 8.0,
                y + ROW_HEIGHT - 2.0,
            ),
        );
        widgets::paint_meter(ui, meter_rect, usage / 100.0);
        painter.text(
            pos2(meter_rect.right() + 40.0, y),
            egui::Align2::RIGHT_TOP,
            percent,
            font.clone(),
            percent_color(usage),
        );
        if show_freq {
            let ghz = mhz.get(index).copied().unwrap_or(0) as f64 / 1000.0;
            painter.text(
                pos2(meter_rect.right() + 88.0, y),
                egui::Align2::RIGHT_TOP,
                format!("{ghz:.1}GHz"),
                font.clone(),
                DIM,
            );
        }
    }
    (grid.rows as f32 * ROW_HEIGHT).min(rect.height())
}

/// Load, temperatures, power and fans under the core grid.
/// Returns true when the LibreHardwareMonitor setup was asked for.
fn cpu_info(ui: &mut Ui, state: &GuiState, snapshot: &Snapshot) -> bool {
    let mut setup = false;
    let cpu = &snapshot.cpu;
    let config = &state.config;
    ui.horizontal_wrapped(|ui| {
        if let Some(load) = cpu.load_average {
            ui.label(dim("Load"));
            ui.label(format!("{:.2} {:.2} {:.2}", load[0], load[1], load[2]));
        }
        // CPU sensors, plus other sensors once the user has named them.
        let named = snapshot
            .sensors
            .iter()
            .flat_map(|chip| chip.temperatures.iter())
            .filter(|t| {
                config.label_for(&t.key).is_some()
                    && !cpu.temperatures.iter().any(|c| c.key == t.key)
            });
        for temp in cpu.temperatures.iter().chain(named) {
            if config.is_hidden(&temp.key) {
                continue;
            }
            ui.label(dim(format!(" {}", config.temperature_label(temp))))
                .on_hover_text(&temp.key);
            ui.label(colored(
                format!("{:.0}°C", temp.celsius),
                temperature_color(temp.celsius),
            ));
        }
        if let Some(error) = &snapshot.lhm_error {
            ui.label(colored(LHM_HINT, WARN))
                .on_hover_text(lhm_help(error));
            // Setting up is possible only on this machine.
            if state.is_local && ui.small_button("set up…").clicked() {
                setup = true;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let power = &snapshot.power;
        let watts = |w: Option<f32>| w.map_or("--".to_owned(), |w| format!("{w:.0}W"));
        // Without RAPL, an AMD APU's SoC power stands in for the CPU.
        let (cpu_label, cpu_w) = power.cpu_or_soc_w();
        ui.label(dim(format!("Power {cpu_label}")));
        ui.label(watts(cpu_w));
        ui.label(dim("GPU"));
        ui.label(watts(power.gpus_w));
        ui.label(dim("Σ"));
        ui.label(colored(watts(power.total_w), TEXT).strong());
        if let Some(battery) = &power.battery {
            ui.label(dim("   Bat"));
            ui.label(battery.summary());
        }
        let fans: Vec<_> = snapshot
            .sensors
            .iter()
            .flat_map(|chip| chip.fans.iter())
            .filter(|f| {
                !config.is_hidden(&f.key) && (f.rpm > 0 || config.label_for(&f.key).is_some())
            })
            .collect();
        if !fans.is_empty() {
            ui.label(dim("   Fans"));
            for fan in fans {
                ui.label(dim(config.fan_label(fan))).on_hover_text(&fan.key);
                ui.label(format!("{} rpm", fan.rpm));
            }
        }
    });
    setup
}

// ---------------------------------------------------------------- memory

fn memory(ui: &mut Ui, state: &GuiState) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    let memory = &snapshot.memory;
    let total = memory.total_bytes.max(1) as f32;
    let mut rows: Vec<(&str, u64, f32)> = vec![
        ("Used", memory.used_bytes, memory.used_bytes as f32 / total),
        (
            "Avail",
            memory.available_bytes,
            memory.available_bytes as f32 / total,
        ),
    ];
    if let Some(cached) = memory.cached_bytes {
        rows.push(("Cached", cached, cached as f32 / total));
    }
    rows.push(("Free", memory.free_bytes, memory.free_bytes as f32 / total));
    if memory.swap_total_bytes > 0 {
        let swap = memory.swap_total_bytes as f32;
        rows.push((
            "Swap",
            memory.swap_used_bytes,
            memory.swap_used_bytes as f32 / swap,
        ));
    }
    for (label, bytes, ratio) in rows {
        labeled_meter(
            ui,
            label,
            ratio,
            &format!("{:>6} {:>4.0}%", format::bytes(bytes), ratio * 100.0),
        );
    }
}

/// "Label  ■■■■■■■■  value" on one row.
fn labeled_meter(ui: &mut Ui, label: &str, ratio: f32, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized([56.0, ROW_HEIGHT], egui::Label::new(dim(label)));
        // Room for the whole value text; the meter takes what is left.
        let font = egui::TextStyle::Body.resolve(ui.style());
        let glyph = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let value_width = (value.chars().count() as f32 * glyph + 12.0).max(110.0);
        let width = (ui.available_width() - value_width).max(20.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        widgets::paint_meter(ui, rect, ratio);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(value));
    });
}

// ---------------------------------------------------------------- GPU

/// "GPU2 ■■■■ 12% 1.5G/2.0G 46°C" on one line; the name on hover.
fn compact_gpu_row(ui: &mut Ui, gpu: &GpuStats) {
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
    let util = gpu.utilization_percent.unwrap_or(0.0);
    ui.horizontal(|ui| {
        ui.add_sized(
            [56.0, ROW_HEIGHT],
            egui::Label::new(colored(format!("GPU{}", gpu.index), widgets::GPU_TITLE)),
        )
        .on_hover_text(&gpu.name);
        let value_width = 190.0;
        let width = (ui.available_width() - value_width).max(20.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        widgets::paint_meter(ui, rect, util / 100.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(value));
    });
}

fn gpus(ui: &mut Ui, state: &GuiState) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    if snapshot.gpus.is_empty() {
        ui.label(dim("no GPU found"));
        return;
    }
    // A name row, utilization and VRAM per GPU; with less room VRAM joins
    // the utilization row, and with even less (three GPUs in a short panel)
    // each GPU gets one line, as on overview cards.
    let rows_fit = |rows: f32| {
        ui.available_height() >= snapshot.gpus.len() as f32 * (rows * (ROW_HEIGHT + 3.0) + 4.0)
    };
    let vram_row = rows_fit(3.0);
    if !vram_row && !rows_fit(2.0) {
        for gpu in &snapshot.gpus {
            compact_gpu_row(ui, gpu);
        }
        return;
    }
    let per_gpu = (ui.available_height() / snapshot.gpus.len() as f32).max(ROW_HEIGHT * 2.0);
    for (i, gpu) in snapshot.gpus.iter().enumerate() {
        let top = ui.cursor().top();
        // Wraps on narrow panels instead of cutting off power and clocks.
        ui.horizontal_wrapped(|ui| {
            ui.label(colored(gpu.index.to_string(), widgets::GPU_TITLE));
            ui.label(gpu.name.trim_start_matches("NVIDIA "));
            if let Some(t) = gpu.temperature_c {
                ui.label(colored(format!("{t:.0}°C"), temperature_color(t)));
            }
            if let Some(w) = gpu.power_w {
                let limit = gpu
                    .power_limit_w
                    .map_or(String::new(), |l| format!("/{l:.0}"));
                ui.label(dim(format!("{w:.0}{limit}W")));
            }
            if let Some(fan) = gpu.fan_percent {
                ui.label(dim(format!("fan {fan:.0}%")));
            }
            if let Some(clock) = gpu.clock_mhz {
                ui.label(dim(format!("{clock}MHz")));
            }
        });
        let util = gpu.utilization_percent.unwrap_or(0.0);
        let vram = gpu
            .memory_used_bytes
            .zip(gpu.memory_total_bytes)
            .filter(|_| !vram_row)
            .map_or(String::new(), |(used, total)| {
                format!(" {}/{}", format::bytes(used), format::bytes(total))
            });
        labeled_meter(
            ui,
            "util",
            util / 100.0,
            &format!(" {}{vram}", format::percent(gpu.utilization_percent)),
        );
        if let (Some(used), Some(total), true) =
            (gpu.memory_used_bytes, gpu.memory_total_bytes, vram_row)
        {
            let ratio = used as f32 / total.max(1) as f32;
            labeled_meter(
                ui,
                "vram",
                ratio,
                &format!("{}/{}", format::bytes(used), format::bytes(total)),
            );
        }
        // Utilization history fills the rest of this GPU's share.
        let used = ui.cursor().top() - top;
        let graph_height = (per_gpu - used - 6.0).max(0.0);
        if graph_height >= 12.0 {
            let width = ui.available_width();
            let samples = state
                .history
                .gpus
                .get(i)
                .map(|h| h.last(width as usize))
                .unwrap_or_default();
            widgets::area_graph(
                ui,
                vec2(width, graph_height),
                &samples,
                100.0,
                widgets::heat_color,
                false,
            );
        }
        ui.add_space(4.0);
    }
}

// ---------------------------------------------------------------- storage

fn storage(ui: &mut Ui, state: &GuiState) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    let now = now_secs();
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            for fs in &snapshot.filesystems {
                let mount = fs.mount_points.first().cloned().unwrap_or_default();
                let extra = fs.mount_points.len().saturating_sub(1);
                let label = if extra > 0 {
                    format!("{mount} +{extra}")
                } else {
                    mount
                };
                let used = fs.total_bytes.saturating_sub(fs.available_bytes);
                let ratio = used as f32 / fs.total_bytes.max(1) as f32;
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, ROW_HEIGHT], egui::Label::new(label).truncate())
                        .on_hover_text(format!(
                            "{} ({})\n{}",
                            fs.device,
                            fs.fs_type,
                            fs.mount_points.join("\n")
                        ));
                    let value_width = 190.0;
                    let width = (ui.available_width() - value_width).clamp(20.0, 220.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                    widgets::paint_meter(ui, rect, ratio);
                    ui.label(colored(
                        format!("{:>3.0}%", ratio * 100.0),
                        percent_color(ratio * 100.0),
                    ));
                    ui.label(dim(format!(
                        "{} free/{}",
                        format::bytes(fs.available_bytes),
                        format::bytes(fs.total_bytes)
                    )));
                });
            }
            for disk in &snapshot.disks {
                // Wraps on narrow panels so SMART details move to a second
                // line instead of being cut off.
                ui.horizontal_wrapped(|ui| {
                    ui.add_sized([70.0, ROW_HEIGHT], egui::Label::new(&disk.name));
                    ui.label(dim("R"));
                    ui.label(format!("{:>8}", format::rate(disk.read_bytes_per_sec)));
                    ui.label(dim("W"));
                    ui.label(format!("{:>8}", format::rate(disk.write_bytes_per_sec)));
                    if let Some(busy) = disk.busy_percent {
                        ui.label(colored(format!("{busy:>3.0}%"), percent_color(busy)));
                    }
                    if let Some(t) = disk.temperature_c {
                        ui.label(colored(format!("{t:.0}°C"), temperature_color(t)));
                    }
                    if let Some(smart) = snapshot
                        .smart
                        .iter()
                        .find(|s| device_matches_disk(&s.device, &disk.name))
                    {
                        smart_labels(ui, smart, now);
                    }
                });
            }
            // SMART results without a listed block device (RAID controller pass-through).
            for smart in snapshot.smart.iter().filter(|s| {
                !snapshot
                    .disks
                    .iter()
                    .any(|d| device_matches_disk(&s.device, &d.name))
            }) {
                ui.horizontal(|ui| {
                    let label = ui.add_sized([70.0, ROW_HEIGHT], egui::Label::new(&smart.device));
                    if !smart.model.is_empty() {
                        label.on_hover_text(&smart.model);
                    }
                    // No disk row shows this drive's temperature, so show it here.
                    if let Some(t) = smart.temperature_c {
                        ui.label(colored(format!("{t:.0}°C"), temperature_color(t)));
                    }
                    smart_labels(ui, smart, now);
                });
            }
            for fs in &snapshot.raid.btrfs {
                btrfs_rows(ui, fs, now);
            }
            for md in &snapshot.raid.md {
                ui.horizontal(|ui| {
                    ui.label(colored(&md.name, widgets::STORAGE_TITLE));
                    ui.label(&md.level);
                    if let (Some(expected), Some(working)) =
                        (md.expected_devices, md.working_devices)
                    {
                        ui.label(dim(format!("[{working}/{expected}]")));
                    }
                    if !md.active {
                        ui.label(colored("inactive", WARN));
                    } else if md.is_degraded() {
                        ui.label(colored("DEGRADED", BAD).strong());
                    } else {
                        ui.label(colored("ok", OK));
                    }
                    if let Some(sync) = &md.sync {
                        ui.label(colored(
                            format!("{} {:.1}%", sync.action, sync.percent),
                            WARN,
                        ));
                        if let Some(minutes) = sync.finish_minutes {
                            ui.label(dim(format!("eta {minutes:.0}m")));
                        }
                    }
                });
            }
        });
}

fn smart_labels(ui: &mut Ui, smart: &SmartDevice, now: u64) {
    ui.label(dim("SMART"));
    if let Some(error) = &smart.error {
        ui.label(colored("error", BAD)).on_hover_text(error);
        return;
    }
    if smart.needs_attention() {
        let text = if smart.passed == Some(false) {
            "FAIL"
        } else {
            "warn"
        };
        ui.label(colored(text, BAD).strong());
    } else {
        ui.label(colored("ok", OK));
    }
    if let Some(used) = smart.percentage_used {
        ui.label(dim(format!("wear {used}%")));
    }
    for (name, value) in [
        ("realloc", smart.reallocated_sectors),
        ("pending", smart.pending_sectors),
        ("media err", smart.media_errors),
    ] {
        if let Some(n) = value.filter(|n| *n > 0) {
            ui.label(colored(format!("{name} {n}"), BAD));
        }
    }
    if let Some(ms) = smart.collected_at_ms {
        let age = now.saturating_sub(ms / 1000);
        let color = if age > SMART_STALE_SECS { WARN } else { DIM };
        ui.label(colored(format!("({})", format::age(age)), color))
            .on_hover_text("time since the root timer collected this result");
    }
}

fn btrfs_rows(ui: &mut Ui, fs: &BtrfsFilesystem, now: u64) {
    let mount = fs
        .mount_points
        .first()
        .cloned()
        .unwrap_or_else(|| fs.fsid.chars().take(8).collect());
    ui.horizontal_wrapped(|ui| {
        ui.label(colored("btrfs", widgets::STORAGE_TITLE));
        ui.label(mount)
            .on_hover_text(format!("{}\n{}", fs.fsid, fs.devices.join(", ")));
        for alloc in fs.allocations.iter().filter(|a| a.kind != "system") {
            let kind = if alloc.kind == "metadata" {
                "meta"
            } else {
                "data"
            };
            ui.label(dim(kind));
            ui.label(&alloc.profile);
            if alloc.kind == "data" {
                ui.label(dim(format!(
                    "{}/{}",
                    format::bytes(alloc.used_bytes),
                    format::bytes(alloc.total_bytes)
                )));
            }
        }
        ui.label(dim(format!("{} devs", fs.devices.len())));
    });
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let missing = fs.device_stats.iter().filter(|d| d.missing).count();
        if missing > 0 {
            ui.label(colored(format!("{missing} device(s) MISSING"), BAD).strong());
        }
        let errors: u64 = fs.device_stats.iter().map(|d| d.total_errors()).sum();
        if errors > 0 {
            let detail: Vec<String> = fs
                .device_stats
                .iter()
                .filter(|d| d.total_errors() > 0)
                .map(|d| format!("{}: {}", d.label(), d.total_errors()))
                .collect();
            ui.label(colored(format!("errors {errors} ({})", detail.join(", ")), BAD));
        } else {
            ui.label(colored("errors 0", OK));
        }
        ui.label(dim("(live) ·")).on_hover_text("error counters only grow when data is read; the scrub time tells when all data was last verified");
        match &fs.scrub {
            None => {
                ui.label(colored("scrub: no record", WARN));
            }
            Some(scrub) => {
                let age = now.saturating_sub(scrub.started_at_ms / 1000);
                let color = if age > SCRUB_STALE_SECS { WARN } else { DIM };
                ui.label(colored(format!("scrub {}", format::age(age)), color));
                let (text, color) = match scrub.state.as_str() {
                    "finished" if scrub.uncorrectable > 0 => (format!("{} uncorrectable", scrub.uncorrectable), BAD),
                    "finished" if scrub.errors > 0 => (format!("{} corrected", scrub.corrected), WARN),
                    "finished" => ("ok".to_owned(), OK),
                    "canceled" => ("canceled".to_owned(), WARN),
                    _ => ("unfinished".to_owned(), WARN),
                };
                ui.label(colored(text, color));
                ui.label(dim(format::duration(scrub.duration_secs)));
            }
        }
    });
}

// ---------------------------------------------------------------- network

/// Link text: "10G" for wired links, "Wi-Fi 576/865M -74dBm" for wireless.
fn link_label(net: &NetworkStats) -> Option<String> {
    if let Some(wifi) = &net.wireless {
        let mut text = "Wi-Fi".to_owned();
        if let (Some(rx), Some(tx)) = (wifi.rx_bitrate_mbps, wifi.tx_bitrate_mbps) {
            text.push_str(&format!(" {:.0}/{}", rx, format::link_speed(tx as f64)));
        }
        if let Some(signal) = wifi.signal_dbm {
            text.push_str(&format!(" {signal}dBm"));
        }
        return Some(text);
    }
    net.link_speed_mbps
        .map(|mbps| format::link_speed(mbps as f64))
}

/// Interface picker on the right of the network title.
fn network_title(ui: &mut Ui, state: &mut GuiState) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    let current = state.history.net_interface.clone().unwrap_or_default();
    let speed = snapshot
        .networks
        .iter()
        .find(|n| n.name == current)
        .and_then(link_label)
        .unwrap_or_default();
    let names: Vec<String> = snapshot.networks.iter().map(|n| n.name.clone()).collect();
    let mut chosen = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(dim(speed));
        egui::ComboBox::from_label("interface")
            .selected_text(&current)
            .show_ui(ui, |ui| {
                for name in &names {
                    if ui.selectable_label(*name == current, name).clicked() {
                        chosen = Some(name.clone());
                    }
                }
            });
    });
    if let Some(name) = chosen {
        state.history.select_interface(&name);
    }
}

fn network(ui: &mut Ui, state: &mut GuiState) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    let listed: Vec<&NetworkStats> = snapshot
        .networks
        .iter()
        .filter(|n| !is_virtual_interface(&n.name) && n.link_up != Some(false))
        .collect();
    let list_height = listed.len() as f32 * (ROW_HEIGHT + 2.0) + 4.0;
    let graphs_height = (ui.available_height() - list_height).max(40.0);
    let width = ui.available_width();
    let history = &state.history;
    let max = history
        .net_rx
        .max_of_last(width as usize)
        .max(history.net_tx.max_of_last(width as usize))
        .max(10.0 * 1024.0);
    let selected = history
        .net_interface
        .as_deref()
        .and_then(|name| snapshot.networks.iter().find(|n| n.name == name));
    let (rx_rate, tx_rate, rx_total, tx_total) = selected.map_or((0.0, 0.0, 0, 0), |n| {
        (
            n.rx_bytes_per_sec,
            n.tx_bytes_per_sec,
            n.rx_total_bytes,
            n.tx_total_bytes,
        )
    });

    // Download on top, upload mirrored below, on a shared scale.
    let half = vec2(width, graphs_height / 2.0);
    let rx = widgets::area_graph(
        ui,
        half,
        &history.net_rx.last(width as usize),
        max,
        widgets::rx_color,
        false,
    );
    ui.painter().text(
        rx.rect.left_top(),
        egui::Align2::LEFT_TOP,
        format!(
            "▼ {}   total {}   scale {}",
            format::rate(rx_rate),
            format::bytes(rx_total),
            format::rate(max)
        ),
        FontId::monospace(12.0),
        TEXT,
    );
    let tx = widgets::area_graph(
        ui,
        half,
        &history.net_tx.last(width as usize),
        max,
        widgets::tx_color,
        true,
    );
    ui.painter().text(
        tx.rect.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        format!(
            "▲ {}   total {}",
            format::rate(tx_rate),
            format::bytes(tx_total)
        ),
        FontId::monospace(12.0),
        TEXT,
    );

    // Interface list; clicking one shows it in the graphs.
    let mut chosen = None;
    for net in listed {
        let is_current = Some(&net.name) == state.history.net_interface.as_ref();
        ui.horizontal(|ui| {
            let link = link_label(net).unwrap_or_default();
            // The link column shrinks first; without room it moves to the
            // name's tooltip, keeping the rates visible.
            let link_width = (ui.available_width() - 110.0 - 210.0).min(170.0);
            let mut name = ui.add_sized(
                [110.0, ROW_HEIGHT],
                egui::Button::selectable(is_current, &net.name),
            );
            if link_width < 60.0 && !link.is_empty() {
                name = name.on_hover_text(&link);
            }
            if name.clicked() {
                chosen = Some(net.name.clone());
            }
            if link_width >= 60.0 {
                ui.add_sized(
                    [link_width, ROW_HEIGHT],
                    egui::Label::new(dim(link)).truncate(),
                );
            }
            ui.label(format!(
                "▼{:>9} ▲{:>9}",
                format::rate(net.rx_bytes_per_sec),
                format::rate(net.tx_bytes_per_sec)
            ));
        });
    }
    if let Some(name) = chosen {
        state.history.select_interface(&name);
    }
}

// ---------------------------------------------------------------- processes

/// Columns of the process table: header, sort key, width (None = remainder).
const PROC_COLUMNS: [(&str, Option<SortKey>, Option<f32>); 9] = [
    ("Pid", Some(SortKey::Pid), Some(70.0)),
    ("Program", Some(SortKey::Name), Some(130.0)),
    ("Command", None, None),
    ("Thr", Some(SortKey::Threads), Some(40.0)),
    ("User", Some(SortKey::User), Some(80.0)),
    ("Mem", Some(SortKey::Memory), Some(64.0)),
    ("Cpu%", Some(SortKey::Cpu), Some(56.0)),
    ("Gpu%", Some(SortKey::Gpu), Some(48.0)),
    ("VRAM", Some(SortKey::Vram), Some(64.0)),
];

/// The command column is only worth showing with at least this much room.
const COMMAND_MIN_WIDTH: f32 = 120.0;
/// Columns given up first when the table is too narrow; Pid, Program, Mem
/// and Cpu% always stay.
const DROP_ORDER: [&str; 5] = ["Command", "Thr", "User", "VRAM", "Gpu%"];

/// Removes columns in [`DROP_ORDER`] until the rest fit in `width`.
fn fit_columns(
    columns: &mut Vec<(&'static str, Option<SortKey>, Option<f32>)>,
    width: f32,
    spacing: f32,
) {
    let needed = |columns: &[(&str, Option<SortKey>, Option<f32>)]| -> f32 {
        columns
            .iter()
            .map(|(_, _, w)| w.unwrap_or(COMMAND_MIN_WIDTH) + spacing)
            .sum()
    };
    for name in DROP_ORDER {
        if needed(columns) <= width {
            break;
        }
        columns.retain(|(n, _, _)| *n != name);
    }
}

fn processes(ui: &mut Ui, state: &mut GuiState) {
    // Measured before anything is drawn: content wider than the panel (the
    // filter row on a narrow one) would widen the layout and hide columns.
    let panel_width = ui.available_width();
    ui.horizontal(|ui| {
        ui.label(dim("filter"));
        ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .hint_text("name, command, user or pid")
                .desired_width((panel_width * 0.35).clamp(80.0, 220.0)),
        );
        ui.checkbox(&mut state.show_kernel_threads, "kernel threads");
        let note = match state.status() {
            Some(status) => colored(status, WARN),
            None => dim("right-click a process for actions"),
        };
        ui.add(egui::Label::new(note).truncate());
    });

    let has_gpus = state.snapshot.as_ref().is_some_and(|s| !s.gpus.is_empty());
    // Thread counts are not available everywhere (e.g. Windows).
    let has_threads = state
        .snapshot
        .as_ref()
        .is_some_and(|s| s.processes.iter().any(|p| p.threads.is_some()));
    let mut columns: Vec<_> = PROC_COLUMNS
        .iter()
        .filter(|(_, key, _)| has_gpus || !matches!(key, Some(SortKey::Gpu | SortKey::Vram)))
        .filter(|(_, key, _)| has_threads || *key != Some(SortKey::Threads))
        .copied()
        .collect();
    fit_columns(&mut columns, panel_width, ui.spacing().item_spacing.x);
    // Without the command column, the last one takes the spare width.
    let remainder = columns
        .iter()
        .position(|(_, _, w)| w.is_none())
        .unwrap_or(columns.len() - 1);

    let mut table = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .auto_shrink(false)
        .cell_layout(Layout::left_to_right(Align::Center));
    for (index, (_, _, width)) in columns.iter().enumerate() {
        table = table.column(match width {
            Some(w) if index != remainder => Column::initial(*w).at_least(30.0).clip(true),
            _ => Column::remainder()
                .at_least(COMMAND_MIN_WIDTH.min(60.0))
                .clip(true),
        });
    }

    let mut sort_clicked = None;
    let mut select = None;
    let mut ask = None;
    // Rows are computed once per frame; the closures below only read them.
    let rows = state.process_rows();
    let (sort_key, descending, selected_pid) =
        (state.sort_key, state.sort_descending, state.selected_pid);
    table
        .header(ROW_HEIGHT + 4.0, |mut header| {
            for (name, key, _) in &columns {
                header.col(|ui| {
                    let mut text = RichText::new(*name).strong();
                    if key.is_some() && *key == Some(sort_key) {
                        let arrow = if descending { "▼" } else { "▲" };
                        text = RichText::new(format!("{name}{arrow}"))
                            .strong()
                            .color(widgets::PROC_TITLE);
                    }
                    let response = ui.add(egui::Label::new(text).sense(Sense::click()));
                    if let Some(key) = key
                        && response.clicked()
                    {
                        sort_clicked = Some(*key);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                let process = &rows[row.index()];
                let p = process.info;
                row.set_selected(selected_pid == Some(p.pid));
                for (name, _, _) in &columns {
                    row.col(|ui| {
                        match *name {
                            "Pid" => ui.label(p.pid.to_string()),
                            "Program" => ui.label(&p.name),
                            "Command" => ui.label(dim(&p.command)),
                            "Thr" => ui.label(p.threads.map_or(String::new(), |t| t.to_string())),
                            "User" => ui.label(&p.user),
                            "Mem" => ui.label(format::bytes(p.memory_bytes)),
                            "Cpu%" => {
                                let cpu = p.cpu_percent;
                                let color = if cpu > 0.0 { percent_color(cpu) } else { DIM };
                                ui.label(colored(format!("{cpu:.1}"), color))
                            }
                            "Gpu%" => ui.label(
                                process
                                    .gpu_percent
                                    .map_or(String::new(), |g| format!("{g:.0}")),
                            ),
                            _ => ui.label(colored(
                                process.vram_bytes.map_or(String::new(), format::bytes),
                                widgets::GPU_TITLE,
                            )),
                        };
                    });
                }
                let response = row.response();
                if response.clicked() {
                    select = Some(p.pid);
                }
                response.context_menu(|ui| {
                    ui.label(dim(format!("{} ({})", p.name, p.pid)));
                    ui.separator();
                    if ui.button("Terminate (SIGTERM)").clicked() {
                        ask = Some((p.pid, p.name.clone(), ProcessSignal::Terminate));
                        ui.close();
                    }
                    if ui.button(colored("Kill (SIGKILL)", BAD)).clicked() {
                        ask = Some((p.pid, p.name.clone(), ProcessSignal::Kill));
                        ui.close();
                    }
                });
            });
        });
    drop(rows);

    if let Some(key) = sort_clicked {
        state.toggle_sort(key);
    }
    if let Some(pid) = select {
        state.selected_pid = Some(pid);
    }
    if let Some((pid, name, signal)) = ask {
        state.selected_pid = Some(pid);
        state.ask_signal(pid, &name, signal);
    }
}

/// Modal confirmation before a signal is sent.
pub fn confirm_dialog(ctx: &egui::Context, state: &mut GuiState) {
    let Some(confirm) = state.confirm.clone() else {
        return;
    };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::unique("confirm_signal")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.heading(colored(
            format!("Send {}?", signal_name(confirm.signal)),
            BAD,
        ));
        ui.add_space(6.0);
        ui.label(format!("Process {} ({})", confirm.pid, confirm.name));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .button(colored(
                    format!("Send {}", signal_name(confirm.signal)),
                    BAD,
                ))
                .clicked()
            {
                decision = Some(true);
            }
            if ui.button("Cancel").clicked() {
                decision = Some(false);
            }
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => {
            state.commands.push(Command::Signal {
                pid: confirm.pid,
                signal: confirm.signal,
            });
            state.confirm = None;
        }
        Some(false) => {
            state.set_status("cancelled".to_owned());
            state.confirm = None;
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_grid_falls_back_to_denser_cells() {
        // Tall enough: wide cells with meters, as few columns as rows allow.
        let roomy = core_grid_layout(32, 810.0, 112.0);
        assert!(roomy.meters && roomy.cell_width >= 150.0);
        assert_eq!((roomy.columns, roomy.rows), (5, 7));
        // Narrow and short: 150px cells need 11 rows; 110px cells fit.
        let narrow = core_grid_layout(32, 600.0, 130.0);
        assert!(narrow.meters && narrow.rows <= 8, "{narrow:?}");
        // Very narrow: compact cells without meters.
        let tiny = core_grid_layout(32, 420.0, 100.0);
        assert!(!tiny.meters && tiny.rows <= 6, "{tiny:?}");
        // Short as well: the tightest cells still fit all cores.
        let squeezed = core_grid_layout(32, 455.0, 70.0);
        assert!(
            squeezed.rows <= 4 && squeezed.cell_width >= 56.0,
            "{squeezed:?}"
        );
    }
}
