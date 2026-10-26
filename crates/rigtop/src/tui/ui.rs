//! Drawing: a btop-like layout built from the current [`App`] state.
//!
//! ```text
//! ┌ cpu ─────────────────────────────────────────────────────────┐
//! │ graph                       │ per-core meters, temps, power  │
//! └──────────────────────────────────────────────────────────────┘
//! ┌ mem ────────────────┐┌ proc ─────────────────────────────────┐
//! ├ gpu ────────────────┤│                                       │
//! ├ storage ────────────┤│                                       │
//! ├ net ────────────────┤│                                       │
//! └─────────────────────┘└───────────────────────────────────────┘
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row, Table, TableState};
use rigtop_core::Snapshot;
use rigtop_core::snapshot::{
    BtrfsFilesystem, CpuStats, MemoryStats, NetworkStats, SmartDevice, device_matches_disk,
};

use super::app::{App, ProcTableHit};
use super::graph::{self, BrailleGraph};
use rigtop_core::format;
use rigtop_core::view::{GraphSpan, LHM_HINT, SortKey, is_virtual_interface, signal_name};

/// Box border color.
pub(super) const BORDER: Color = Color::Rgb(85, 85, 100);
/// Secondary text (units, labels, commands).
pub(super) const DIM: Color = Color::Rgb(125, 125, 140);
/// Unfilled part of meters.
pub(super) const METER_EMPTY: Color = Color::Rgb(55, 55, 65);
/// Title colors per box.
pub(super) const CPU_TITLE: Color = Color::Rgb(120, 210, 120);
pub(super) const MEM_TITLE: Color = Color::Rgb(230, 200, 90);
pub(super) const GPU_TITLE: Color = Color::Rgb(200, 130, 230);
pub(super) const STORAGE_TITLE: Color = Color::Rgb(90, 200, 220);
pub(super) const NET_TITLE: Color = Color::Rgb(110, 150, 240);
pub(super) const PROC_TITLE: Color = Color::Rgb(240, 150, 90);
/// Status colors.
pub(super) const OK: Color = Color::Rgb(110, 200, 110);
pub(super) const WARN: Color = Color::Rgb(230, 200, 70);
pub(super) const BAD: Color = Color::Rgb(235, 80, 80);

/// Narrowest per-core cell: "C12 ■■■■■ 100% ".
const CORE_CELL_MIN_WIDTH: u16 = 15;
/// Cells at least this wide also show the core frequency.
const CORE_FREQ_MIN_WIDTH: usize = 28;
/// Compact per-core cell without a meter: "C12  34% ".
const CORE_CELL_COMPACT_WIDTH: u16 = 10;
/// Share of the terminal height the CPU box grows to (btop uses about a third).
const CPU_BOX_SHARE_PERCENT: u16 = 28;
/// The CPU box never takes more than this share of the height; denser cell
/// styles are used instead.
const CPU_BOX_MAX_PERCENT: u16 = 45;
/// Smallest useful heights of the left column boxes (borders included).
const MIN_BOX_HEIGHT: u16 = 3;
/// Lines below the core grid (load/temperatures, power/fans).
const CPU_INFO_LINES: u16 = 2;
/// Text lines per GPU; history graph rows come on top.
const GPU_TEXT_ROWS: u16 = 2;
/// Maximum history graph rows per GPU when there is spare height.
const GPU_GRAPH_MAX_ROWS: u16 = 4;
/// Network box height: preferred minimum, and when space is short.
const NET_MIN_HEIGHT: u16 = 8;
const NET_COMPACT_HEIGHT: u16 = 5;
/// Rows kept for the network graphs before listing interfaces.
const NET_GRAPH_MIN_ROWS: u16 = 4;
/// SMART results older than this are flagged as stale.
const SMART_STALE_SECS: u64 = 3600;
/// Scrubs older than this are flagged (a monthly scrub is the common advice).
const SCRUB_STALE_SECS: u64 = 35 * 86_400;

/// Draws one frame.
#[cfg(test)]
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    draw_in(frame, app, area);
}

/// Draws one host's detailed view into `area`.
pub fn draw_in(frame: &mut Frame, app: &mut App, area: Rect) {
    app.sync_selection();
    // The table state is mutated while rendering rows that borrow `app`;
    // move it out for the duration of the frame.
    let mut table_state = std::mem::take(&mut app.table_state);
    let hit = draw_screen(frame, app, &mut table_state, area);
    app.table_state = table_state;
    app.proc_hit = hit;
}

/// Draws everything and returns where the process table went (for the mouse).
fn draw_screen(
    frame: &mut Frame,
    app: &App,
    table_state: &mut TableState,
    area: Rect,
) -> Option<ProcTableHit> {
    let Some(snapshot) = &app.snapshot else {
        // Connection errors (e.g. "Permission denied (publickey)") show
        // under the waiting message.
        let mut lines = vec![Line::from(dim(app.waiting_message.clone()))];
        if let Some(status) = app.status() {
            lines.push(Line::from(colored(status.to_owned(), WARN)));
        }
        let waiting = Paragraph::new(lines).alignment(Alignment::Center);
        let [_, middle, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(area);
        frame.render_widget(waiting, middle);
        return None;
    };

    let cpu_layout = cpu_layout(snapshot.cpu.per_core_percent.len(), area);
    let [top, bottom] =
        Layout::vertical([Constraint::Length(cpu_layout.height), Constraint::Fill(1)]).areas(area);
    draw_cpu(frame, app, snapshot, top, cpu_layout);

    let left_width = (area.width * 45 / 100).max(44).min(area.width);
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Fill(1)]).areas(bottom);

    let heights = left_column_heights(snapshot, left);
    let [mem, gpu, storage, net] = Layout::vertical([
        Constraint::Length(heights.memory),
        Constraint::Length(heights.gpu),
        Constraint::Length(heights.storage),
        Constraint::Length(heights.network),
    ])
    .areas(left);
    // Boxes that did not fit are left out rather than drawn empty.
    if heights.memory > 0 {
        draw_memory(frame, snapshot, mem);
    }
    if heights.gpu > 0 {
        draw_gpus(frame, app, snapshot, gpu);
    }
    if heights.storage > 0 {
        draw_storage(frame, snapshot, storage);
    }
    if heights.network > 0 {
        draw_network(frame, app, snapshot, net);
    }
    let hit = draw_processes(frame, app, snapshot, right, table_state);

    if let Some(confirm) = &app.confirm {
        draw_confirm(
            frame,
            area,
            confirm.pid,
            &confirm.name,
            signal_name(confirm.signal),
        );
    }
    Some(hit)
}

/// Box heights of the left column; 0 means the box is not shown.
#[derive(Debug, PartialEq, Eq)]
struct LeftHeights {
    memory: u16,
    gpu: u16,
    storage: u16,
    network: u16,
}

/// Distributes the left column by priority:
/// 1. memory, 2. storage (rigtop's main addition over btop, exactly the lines
///    it needs), 3. GPUs (text rows), 4. network.
///
/// A box that does not get its minimum is hidden. Spare height is split
/// between taller GPU history graphs and the network box.
fn left_column_heights(snapshot: &Snapshot, area: Rect) -> LeftHeights {
    let mut remaining = area.height;
    // Takes up to `wanted` rows if at least `minimum` are left.
    let mut take = |wanted: u16, minimum: u16| {
        if wanted == 0 || remaining < minimum.min(wanted) {
            return 0;
        }
        let got = wanted.min(remaining);
        remaining -= got;
        got
    };

    let gpus = snapshot.gpus.len() as u16;
    let memory = take(
        memory_rows(&snapshot.memory).len() as u16 + 2,
        MIN_BOX_HEIGHT,
    );
    let storage_needed = storage_lines(snapshot, area.width.saturating_sub(2)).len() as u16 + 2;
    let storage = take(storage_needed, MIN_BOX_HEIGHT);
    let gpu_text = if gpus == 0 {
        0
    } else {
        gpus * GPU_TEXT_ROWS + 2
    };
    let mut gpu = take(gpu_text, gpu_text);
    let mut network = take(NET_MIN_HEIGHT, NET_COMPACT_HEIGHT);

    if gpu > 0 {
        // Half of the spare height becomes GPU history graph rows.
        let per_gpu = (remaining / 2 / gpus).min(GPU_GRAPH_MAX_ROWS);
        gpu += per_gpu * gpus;
        remaining -= per_gpu * gpus;
    }
    if network > 0 {
        network += remaining;
    }
    LeftHeights {
        memory,
        gpu,
        storage,
        network,
    }
}

/// A rounded box with a colored title, btop style.
pub(super) fn panel(title: &str, color: Color) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
}

/// A horizontal meter of `width` cells, colored along its length.
pub(super) fn meter(width: usize, ratio: f64) -> Vec<Span<'static>> {
    let filled = ((ratio.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    (0..width)
        .map(|i| {
            let color = if i < filled {
                graph::heat_color((i as f64 + 0.5) / width.max(1) as f64)
            } else {
                METER_EMPTY
            };
            Span::styled("■", Style::default().fg(color))
        })
        .collect()
}

/// Color for a percentage (0 green .. 100 red).
pub(super) fn percent_color(percent: f64) -> Color {
    graph::heat_color(percent / 100.0)
}

/// Color for a temperature: green below ~40 °C, red from ~85 °C.
pub(super) fn temperature_color(celsius: f32) -> Color {
    graph::heat_color((celsius as f64 - 40.0) / 45.0)
}

pub(super) fn dim(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(DIM))
}

pub(super) fn colored(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(color))
}

// ---------------------------------------------------------------- CPU

/// How the CPU box is drawn for a terminal size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuLayout {
    /// Box height including borders.
    height: u16,
    /// Whether the usage graph takes the left 40% of the box.
    graph: bool,
    /// Whether core cells have meters (otherwise the compact "C12  34%").
    meters: bool,
}

impl CpuLayout {
    /// Narrowest cell for this style.
    fn cell_min_width(self) -> u16 {
        if self.meters {
            CORE_CELL_MIN_WIDTH
        } else {
            CORE_CELL_COMPACT_WIDTH
        }
    }

    /// Width of the core grid inside a box of `width`.
    fn grid_width(self, width: u16) -> u16 {
        let inner = width.saturating_sub(2);
        if self.graph {
            inner - inner * 40 / 100
        } else {
            inner
        }
    }

    /// Box height when the grid may use up to `rows` rows.
    fn height_for(self, cores: usize, width: u16, rows: u16) -> u16 {
        let (_, used_rows, _) =
            core_grid_layout(cores, self.grid_width(width), rows, self.cell_min_width());
        used_rows as u16 + CPU_INFO_LINES + 2
    }
}

/// Chooses the richest CPU box style whose core grid fits in
/// [`CPU_BOX_MAX_PERCENT`] of the height (graph + meters, then meters only,
/// then compact cells), and grows it towards [`CPU_BOX_SHARE_PERCENT`] on
/// tall terminals so that meters get longer.
fn cpu_layout(cores: usize, area: Rect) -> CpuLayout {
    let cores = cores.max(1);
    let limit = (area.height * CPU_BOX_MAX_PERCENT / 100).max(7);
    let styles = [(true, true), (false, true), (false, false)].map(|(graph, meters)| CpuLayout {
        height: 0,
        graph,
        meters,
    });
    // The most compact grid of a style uses as many columns as fit (rows = 1 request).
    let mut layout = styles
        .into_iter()
        .find(|style| style.height_for(cores, area.width, 1) <= limit)
        .unwrap_or(styles[2]);
    let minimal = layout.height_for(cores, area.width, 1);
    let preferred = (area.height * CPU_BOX_SHARE_PERCENT / 100)
        .min(limit)
        .min(cores as u16 + CPU_INFO_LINES + 2);
    // Allowing more rows means fewer, wider columns; the box then shrinks to
    // the rows actually used, so no blank lines remain.
    let rows = preferred.max(minimal).saturating_sub(CPU_INFO_LINES + 2);
    layout.height = layout.height_for(cores, area.width, rows).max(7);
    layout
}

/// Grid for `cores` cells in `width` x `height`: as few columns as the
/// height allows (so meters get long), at most as many as fit.
/// Returns `(columns, rows, cell_width)`.
fn core_grid_layout(cores: usize, width: u16, height: u16, cell_min: u16) -> (usize, usize, usize) {
    let max_columns = (width / cell_min).max(1) as usize;
    let min_columns = cores.div_ceil(height.max(1) as usize).max(1);
    let columns = min_columns.min(max_columns);
    let rows = cores.div_ceil(columns);
    (columns, rows, width as usize / columns)
}

fn draw_cpu(frame: &mut Frame, app: &App, snapshot: &Snapshot, area: Rect, layout: CpuLayout) {
    let cpu = &snapshot.cpu;
    let host = &snapshot.host;
    // Local wall-clock time of the machine running the UI, like btop's clock.
    let clock = chrono::Local::now().format(" %H:%M:%S ").to_string();
    let mut block = panel("cpu", CPU_TITLE);
    if app.graph_span != GraphSpan::Fit {
        block = block.title_top(Line::from(dim(format!(
            " graph {} ",
            app.graph_span.label()
        ))));
    }
    let block = block
        .title_top(
            Line::from(Span::styled(
                clock,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))
            .centered(),
        )
        .title_top(
            Line::from(
                [
                    rigtop_core::view::version_note(snapshot)
                        .map(|note| colored(format!(" {note}"), WARN)),
                    Some(dim(" ")),
                    Some(colored(host.hostname.clone(), Color::White)),
                    Some(dim(format!(" up {} ", format::duration(host.uptime_secs)))),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
            )
            .right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let graph_width = if layout.graph {
        inner.width * 40 / 100
    } else {
        0
    };
    let [graph_area, cores_area] =
        Layout::horizontal([Constraint::Length(graph_width), Constraint::Fill(1)]).areas(inner);

    let total = Line::from(vec![
        colored(
            format!("{:.0}%", cpu.total_percent),
            percent_color(cpu.total_percent as f64),
        ),
        dim(format!(" {}", cpu.model)),
    ]);
    let [grid_area, info_area] = if layout.graph {
        let samples = app.history.cpu.graph(
            BrailleGraph::capacity(graph_area.width),
            app.graph_span,
            app.interval,
        );
        frame.render_widget(BrailleGraph::new(&samples, 100.0), graph_area);
        // One cell short of the graph, so the model does not run into the
        // core grid.
        frame.render_widget(
            Paragraph::new(total),
            Rect {
                height: 1,
                width: graph_area.width.saturating_sub(1),
                ..graph_area
            },
        );
        Layout::vertical([Constraint::Fill(1), Constraint::Length(CPU_INFO_LINES)])
            .areas(cores_area)
    } else {
        // No graph: the total goes on the first line instead.
        let [total_area, grid, info] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(CPU_INFO_LINES),
        ])
        .areas(cores_area);
        frame.render_widget(Paragraph::new(total), total_area);
        [grid, info]
    };
    draw_core_grid(frame, cpu, layout, grid_area);
    frame.render_widget(Paragraph::new(cpu_info_lines(app, snapshot)), info_area);
}

fn draw_core_grid(frame: &mut Frame, cpu: &CpuStats, layout: CpuLayout, area: Rect) {
    let cores = &cpu.per_core_percent;
    let cell_min = layout.cell_min_width();
    if area.width < cell_min || area.height == 0 || cores.is_empty() {
        return;
    }
    let (columns, rows, cell_width) =
        core_grid_layout(cores.len(), area.width, area.height, cell_min);
    let digits = cores.len().saturating_sub(1).to_string().len();
    // "C" + index + space + [meter] + " 100%" + gap, plus " 4.2G" when wide.
    let show_freq =
        layout.meters && cell_width >= CORE_FREQ_MIN_WIDTH && !cpu.per_core_mhz.is_empty();
    let fixed = 1 + digits + 1 + 5 + 1 + if show_freq { 5 } else { 0 };
    let meter_width = if layout.meters {
        cell_width.saturating_sub(fixed).max(3)
    } else {
        0
    };
    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut spans = Vec::new();
        // Column-major order (like btop): C0..C(rows-1) in the first column.
        for col in 0..columns {
            let index = col * rows + row;
            let Some(&usage) = cores.get(index) else {
                continue;
            };
            spans.push(dim(format!("C{index:<digits$} ")));
            spans.extend(meter(meter_width, usage as f64 / 100.0));
            spans.push(colored(
                format!("{usage:>4.0}%"),
                percent_color(usage as f64),
            ));
            if show_freq {
                let ghz = cpu.per_core_mhz.get(index).copied().unwrap_or(0) as f64 / 1000.0;
                spans.push(dim(format!(" {ghz:.1}G")));
            }
            // Pad to the cell width (the text excludes the trailing gap).
            let text_width = fixed - 1 + meter_width;
            spans.push(Span::raw(" ".repeat(cell_width.saturating_sub(text_width))));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Load average and temperatures, then power and fans.
fn cpu_info_lines(app: &App, snapshot: &Snapshot) -> Vec<Line<'static>> {
    let cpu = &snapshot.cpu;
    let config = &app.config;

    let mut first = Vec::new();
    if let Some(load) = cpu.load_average {
        first.push(dim("Load "));
        first.push(Span::raw(format!(
            "{:.2} {:.2} {:.2}",
            load[0], load[1], load[2]
        )));
    }
    let cpu_temps = cpu.temperatures.iter();
    // Other sensors appear only once the user has named them.
    let named_temps = snapshot
        .sensors
        .iter()
        .flat_map(|chip| chip.temperatures.iter())
        .filter(|t| {
            config.label_for(&t.key).is_some() && !cpu.temperatures.iter().any(|c| c.key == t.key)
        });
    for temp in cpu_temps.chain(named_temps) {
        if config.is_hidden(&temp.key) {
            continue;
        }
        let gap = if first.is_empty() { "" } else { "  " };
        first.push(dim(format!("{gap}{} ", config.temperature_label(temp))));
        first.push(colored(
            format!("{:.0}°C", temp.celsius),
            temperature_color(temp.celsius),
        ));
    }
    if snapshot.lhm_error.is_some() {
        let gap = if first.is_empty() { "" } else { "  " };
        first.push(colored(format!("{gap}{LHM_HINT}"), Color::Yellow));
    }

    let power = &snapshot.power;
    let watts = |w: Option<f32>| w.map_or("--".to_owned(), |w| format!("{w:.0}W"));
    // Without RAPL, an AMD APU's SoC power stands in for the CPU.
    let (cpu_label, cpu_w) = power.cpu_or_soc_w();
    let mut second = vec![
        dim(format!("Power {cpu_label} ")),
        Span::raw(watts(cpu_w)),
        dim(" GPU "),
        Span::raw(watts(power.gpus_w)),
        dim(" Σ "),
        colored(watts(power.total_w), Color::White),
    ];
    if let Some(battery) = &power.battery {
        second.push(dim("   Bat "));
        second.push(Span::raw(battery.summary()));
    }
    let fans: Vec<_> = snapshot
        .sensors
        .iter()
        .flat_map(|chip| chip.fans.iter())
        // Stopped, unnamed channels are usually unconnected headers.
        .filter(|f| !config.is_hidden(&f.key) && (f.rpm > 0 || config.label_for(&f.key).is_some()))
        .collect();
    if !fans.is_empty() {
        second.push(dim("   Fans"));
        for fan in fans {
            second.push(dim(format!(" {} ", config.fan_label(fan))));
            second.push(Span::raw(fan.rpm.to_string()));
        }
    }
    vec![Line::from(first), Line::from(second)]
}

// ---------------------------------------------------------------- memory

fn draw_memory(frame: &mut Frame, snapshot: &Snapshot, area: Rect) {
    let memory = &snapshot.memory;
    let block = panel("mem", MEM_TITLE).title_top(
        Line::from(dim(format!(
            " total {} ",
            format::bytes(memory.total_bytes)
        )))
        .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = memory_rows(memory);

    // "Cached ■■■■■■ 58.0G  65%"
    let meter_width = (inner.width as usize).saturating_sub(7 + 13);
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(label, bytes, ratio)| {
            let mut spans = vec![dim(format!("{label:<7}"))];
            spans.extend(meter(meter_width, ratio));
            spans.push(Span::raw(format!(" {:>6}", format::bytes(bytes))));
            spans.push(dim(format!(" {:>4.0}%", ratio * 100.0)));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Rows of the memory box: label, bytes and ratio of the total.
fn memory_rows(memory: &MemoryStats) -> Vec<(&'static str, u64, f64)> {
    let total = memory.total_bytes.max(1) as f64;
    let mut rows = vec![
        ("Used", memory.used_bytes, memory.used_bytes as f64 / total),
        (
            "Avail",
            memory.available_bytes,
            memory.available_bytes as f64 / total,
        ),
    ];
    if let Some(cached) = memory.cached_bytes {
        rows.push(("Cached", cached, cached as f64 / total));
    }
    rows.push(("Free", memory.free_bytes, memory.free_bytes as f64 / total));
    if memory.swap_total_bytes > 0 {
        let swap_total = memory.swap_total_bytes as f64;
        rows.push((
            "Swap",
            memory.swap_used_bytes,
            memory.swap_used_bytes as f64 / swap_total,
        ));
    }
    rows
}

// ---------------------------------------------------------------- GPU

fn draw_gpus(frame: &mut Frame, app: &App, snapshot: &Snapshot, area: Rect) {
    let block = panel("gpu", GPU_TITLE);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The layout decides how many history graph rows each GPU gets (possibly none).
    let gpus = snapshot.gpus.len().max(1) as u16;
    let rows_per_gpu = (inner.height / gpus).max(GPU_TEXT_ROWS);
    let graph_rows = rows_per_gpu - GPU_TEXT_ROWS;
    for (i, gpu) in snapshot.gpus.iter().enumerate() {
        let y = inner.y + i as u16 * rows_per_gpu;
        if y + rows_per_gpu > inner.y + inner.height {
            break;
        }
        let util = gpu.utilization_percent.unwrap_or(0.0) as f64;
        let name = gpu.name.trim_start_matches("NVIDIA ").to_owned();
        let mut line1 = vec![
            colored(format!("{} ", gpu.index), GPU_TITLE),
            Span::raw(format::truncate(&name, 22)),
            Span::raw(" "),
        ];
        if let Some(t) = gpu.temperature_c {
            line1.push(colored(format!("{t:.0}°C "), temperature_color(t)));
        }
        if let Some(w) = gpu.power_w {
            let limit = gpu
                .power_limit_w
                .map_or(String::new(), |l| format!("/{l:.0}"));
            line1.push(dim(format!("{w:.0}{limit}W ")));
        }
        if let Some(fan) = gpu.fan_percent {
            line1.push(dim(format!("fan {fan:.0}%")));
        }

        let mut line2 = vec![dim("util ")];
        line2.push(colored(
            format!("{} ", format::percent(gpu.utilization_percent)),
            percent_color(util),
        ));
        if let (Some(used), Some(total)) = (gpu.memory_used_bytes, gpu.memory_total_bytes) {
            let ratio = used as f64 / total.max(1) as f64;
            line2.push(dim(" vram "));
            line2.extend(meter(10, ratio));
            line2.push(Span::raw(format!(
                " {}/{}",
                format::bytes(used),
                format::bytes(total)
            )));
        }
        let rows = Rect {
            y,
            height: 1,
            ..inner
        };
        frame.render_widget(Paragraph::new(Line::from(line1)), rows);
        frame.render_widget(Paragraph::new(Line::from(line2)), Rect { y: y + 1, ..rows });
        if graph_rows == 0 {
            continue;
        }
        let samples = app
            .history
            .gpus
            .get(i)
            .map(|h| {
                h.graph(
                    BrailleGraph::capacity(inner.width),
                    app.graph_span,
                    app.interval,
                )
            })
            .unwrap_or_default();
        frame.render_widget(
            BrailleGraph::new(&samples, 100.0),
            Rect {
                y: y + GPU_TEXT_ROWS,
                height: graph_rows,
                ..rows
            },
        );
    }
}

// ---------------------------------------------------------------- storage

fn draw_storage(frame: &mut Frame, snapshot: &Snapshot, area: Rect) {
    let block = panel("storage", STORAGE_TITLE);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(storage_lines(snapshot, inner.width)), inner);
}

/// Filesystems, devices (I/O, temperature, SMART), btrfs and md RAID.
fn storage_lines(snapshot: &Snapshot, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Filesystem capacity: "/ +6  ■■■■■■  15%  925G free / 5.5T"
    let meter_width = (width as usize).saturating_sub(14 + 22).clamp(4, 20);
    for fs in &snapshot.filesystems {
        let mount = fs.mount_points.first().cloned().unwrap_or_default();
        let extra = fs.mount_points.len().saturating_sub(1);
        let name = if extra > 0 {
            format!("{mount} +{extra}")
        } else {
            mount
        };
        let used = fs.total_bytes.saturating_sub(fs.available_bytes);
        let ratio = used as f64 / fs.total_bytes.max(1) as f64;
        let mut spans = vec![Span::raw(format!("{} ", format::pad(&name, 13)))];
        spans.extend(meter(meter_width, ratio));
        spans.push(colored(
            format!(" {:>3.0}%", ratio * 100.0),
            percent_color(ratio * 100.0),
        ));
        spans.push(dim(format!(
            " {} free/{}",
            format::bytes(fs.available_bytes),
            format::bytes(fs.total_bytes)
        )));
        lines.push(Line::from(spans));
    }

    // Block devices with I/O, temperature and SMART.
    for disk in &snapshot.disks {
        let mut spans = vec![
            Span::raw(format::pad(&disk.name, 8)),
            dim(" R "),
            Span::raw(format!("{:>7}", format::rate(disk.read_bytes_per_sec))),
            dim(" W "),
            Span::raw(format!("{:>7}", format::rate(disk.write_bytes_per_sec))),
        ];
        if let Some(busy) = disk.busy_percent {
            spans.push(colored(
                format!(" {busy:>3.0}%"),
                percent_color(busy as f64),
            ));
        }
        if let Some(t) = disk.temperature_c {
            spans.push(colored(format!(" {t:.0}°C"), temperature_color(t)));
        }
        if let Some(smart) = snapshot
            .smart
            .iter()
            .find(|s| device_matches_disk(&s.device, &disk.name))
        {
            spans.extend(smart_spans(smart, now_secs));
        }
        lines.push(Line::from(spans));
    }
    // SMART results not tied to a listed block device (e.g. RAID controller pass-through).
    for smart in snapshot.smart.iter().filter(|s| {
        !snapshot
            .disks
            .iter()
            .any(|d| device_matches_disk(&s.device, &d.name))
    }) {
        let mut spans = vec![Span::raw(format::pad(&smart.device, 8))];
        // No disk row shows this drive's temperature, so show it here.
        if let Some(t) = smart.temperature_c {
            spans.push(colored(format!(" {t:.0}°C"), temperature_color(t)));
        }
        spans.extend(smart_spans(smart, now_secs));
        lines.push(Line::from(spans));
    }

    // btrfs: profiles, usage and error counters.
    for fs in &snapshot.raid.btrfs {
        let mount = fs
            .mount_points
            .first()
            .cloned()
            .unwrap_or_else(|| fs.fsid.chars().take(8).collect());
        let mut spans = vec![
            colored("btrfs ", STORAGE_TITLE),
            Span::raw(format!("{mount} ")),
        ];
        for alloc in &fs.allocations {
            if alloc.kind == "system" {
                continue;
            }
            let kind = if alloc.kind == "metadata" {
                "meta"
            } else {
                "data"
            };
            spans.push(dim(format!("{kind} ")));
            spans.push(Span::raw(format!("{} ", alloc.profile)));
            if alloc.kind == "data" {
                spans.push(dim(format!(
                    "{}/{} ",
                    format::bytes(alloc.used_bytes),
                    format::bytes(alloc.total_bytes)
                )));
            }
        }
        spans.push(dim(format!("{} devs", fs.devices.len())));
        lines.push(Line::from(spans));
        lines.push(btrfs_health_line(fs, now_secs));
    }

    // mdadm arrays.
    for md in &snapshot.raid.md {
        let mut spans = vec![
            colored(format!("{} ", md.name), STORAGE_TITLE),
            Span::raw(format!("{} ", md.level)),
        ];
        if let (Some(expected), Some(working)) = (md.expected_devices, md.working_devices) {
            spans.push(dim(format!("[{working}/{expected}] ")));
        }
        if !md.active {
            spans.push(colored("inactive", WARN));
        } else if md.is_degraded() {
            spans.push(colored("DEGRADED", BAD));
        } else {
            spans.push(colored("ok", OK));
        }
        if let Some(sync) = &md.sync {
            spans.push(colored(
                format!(" {} {:.1}%", sync.action, sync.percent),
                WARN,
            ));
            if let Some(minutes) = sync.finish_minutes {
                spans.push(dim(format!(" eta {minutes:.0}m")));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Second btrfs line: live error counters, then the last scrub, e.g.
/// "  errors 0 (live) · scrub 3d ago ok, 1h12m".
fn btrfs_health_line(fs: &BtrfsFilesystem, now_secs: u64) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    let missing = fs.device_stats.iter().filter(|d| d.missing).count();
    if missing > 0 {
        spans.push(colored(format!("{missing} device(s) MISSING "), BAD));
    }
    let errors: u64 = fs.device_stats.iter().map(|d| d.total_errors()).sum();
    if errors > 0 {
        let detail: Vec<String> = fs
            .device_stats
            .iter()
            .filter(|d| d.total_errors() > 0)
            .map(|d| format!("{}: {}", d.label(), d.total_errors()))
            .collect();
        spans.push(colored(
            format!("errors {errors} ({})", detail.join(", ")),
            BAD,
        ));
    } else {
        spans.push(colored("errors 0", OK));
    }
    // The counters are read every poll but only grow when data is read.
    spans.push(dim(" (live) · "));
    match &fs.scrub {
        None => spans.push(colored("scrub: no record", WARN)),
        Some(scrub) => {
            let age_secs = now_secs.saturating_sub(scrub.started_at_ms / 1000);
            let age_color = if age_secs > SCRUB_STALE_SECS {
                WARN
            } else {
                DIM
            };
            spans.push(colored(
                format!("scrub {} ", format::age(age_secs)),
                age_color,
            ));
            let (text, color) = match scrub.state.as_str() {
                "finished" if scrub.uncorrectable > 0 => {
                    (format!("{} uncorrectable", scrub.uncorrectable), BAD)
                }
                "finished" if scrub.errors > 0 => (format!("{} corrected", scrub.corrected), WARN),
                "finished" => ("ok".to_owned(), OK),
                "canceled" => ("canceled".to_owned(), WARN),
                _ => ("unfinished".to_owned(), WARN),
            };
            spans.push(colored(text, color));
            spans.push(dim(format!(", {}", format::duration(scrub.duration_secs))));
        }
    }
    Line::from(spans)
}

/// "SMART ok wear 3% (8m ago)" / "SMART FAIL" / "SMART error: ...", for one drive.
fn smart_spans(smart: &SmartDevice, now_secs: u64) -> Vec<Span<'static>> {
    let mut spans = vec![dim(" SMART ")];
    if let Some(error) = &smart.error {
        spans.push(colored(
            format!("error: {}", format::truncate(error, 30)),
            BAD,
        ));
        return spans;
    }
    let status = if smart.needs_attention() {
        colored(
            if smart.passed == Some(false) {
                "FAIL"
            } else {
                "warn"
            },
            BAD,
        )
    } else {
        colored("ok", OK)
    };
    spans.push(status);
    if let Some(used) = smart.percentage_used {
        spans.push(dim(format!(" wear {used}%")));
    }
    if let Some(n) = smart.reallocated_sectors.filter(|n| *n > 0) {
        spans.push(colored(format!(" realloc {n}"), BAD));
    }
    if let Some(n) = smart.pending_sectors.filter(|n| *n > 0) {
        spans.push(colored(format!(" pending {n}"), BAD));
    }
    if let Some(n) = smart.media_errors.filter(|n| *n > 0) {
        spans.push(colored(format!(" media err {n}"), BAD));
    }
    // Results come from a timer and may lag (sleeping disks keep the old one).
    if let Some(ms) = smart.collected_at_ms {
        let age_secs = now_secs.saturating_sub(ms / 1000);
        let color = if age_secs > SMART_STALE_SECS {
            WARN
        } else {
            DIM
        };
        spans.push(colored(format!(" ({})", format::age(age_secs)), color));
    }
    spans
}

// ---------------------------------------------------------------- network

fn draw_network(frame: &mut Frame, app: &App, snapshot: &Snapshot, area: Rect) {
    let selected = app
        .history
        .net_interface
        .as_deref()
        .and_then(|name| snapshot.networks.iter().find(|n| n.name == name));
    let name = app
        .history
        .net_interface
        .clone()
        .unwrap_or_else(|| "-".to_owned());
    let mut title = vec![colored(format!(" {name} "), Color::White)];
    if let Some(speed) = selected.and_then(link_label) {
        title.push(dim(format!("{speed} ")));
    }
    title.push(dim("n:next "));
    let block = panel("net", NET_TITLE).title_top(Line::from(title).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    // Physical interfaces that are up, listed under the graphs when there is room.
    let listed: Vec<&NetworkStats> = snapshot
        .networks
        .iter()
        .filter(|n| !is_virtual_interface(&n.name) && n.link_up != Some(false))
        .collect();
    let list_rows = (listed.len() as u16).min(inner.height.saturating_sub(NET_GRAPH_MIN_ROWS));
    let [graphs, list_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(list_rows)]).areas(inner);
    draw_network_graphs(frame, app, selected, graphs);

    let lines: Vec<Line> = listed
        .iter()
        .take(list_rows as usize)
        .map(|n| {
            let marker = if Some(&n.name) == app.history.net_interface.as_ref() {
                "▸"
            } else {
                " "
            };
            Line::from(vec![
                colored(marker, NET_TITLE),
                Span::raw(format::pad(&n.name, 12)),
                dim(format::pad(&link_label(n).unwrap_or_default(), 22)),
                Span::raw(format!(
                    "▼{:>8} ▲{:>8}",
                    format::rate(n.rx_bytes_per_sec),
                    format::rate(n.tx_bytes_per_sec)
                )),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);
}

/// Link speed text: "10G" for wired links, "Wi-Fi 576/865M -74dBm" for wireless.
pub(super) fn link_label(net: &NetworkStats) -> Option<String> {
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

/// Download graph on top, upload graph mirrored below, for the selected interface.
fn draw_network_graphs(frame: &mut Frame, app: &App, selected: Option<&NetworkStats>, area: Rect) {
    if area.height < 2 {
        return;
    }
    let capacity = BrailleGraph::capacity(area.width);
    let history = &app.history;
    let rx = history.net_rx.graph(capacity, app.graph_span, app.interval);
    let tx = history.net_tx.graph(capacity, app.graph_span, app.interval);
    // Shared scale so download and upload are comparable; at least 10 KiB/s
    // so that idle noise does not fill the graph.
    let max = rx
        .iter()
        .chain(&tx)
        .fold(10.0 * 1024.0, |a: f64, &b| a.max(b));

    let [rx_area, tx_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(BrailleGraph::new(&rx, max).color(graph::rx_color), rx_area);
    frame.render_widget(
        BrailleGraph::new(&tx, max)
            .color(graph::tx_color)
            .inverted(true),
        tx_area,
    );

    let (rx_rate, tx_rate, rx_total, tx_total) = selected.map_or((0.0, 0.0, 0, 0), |n| {
        (
            n.rx_bytes_per_sec,
            n.tx_bytes_per_sec,
            n.rx_total_bytes,
            n.tx_total_bytes,
        )
    });
    let rx_line = Line::from(vec![
        colored(format!("▼ {}", format::rate(rx_rate)), Color::White),
        dim(format!(
            "  total {}  scale {}",
            format::bytes(rx_total),
            format::rate(max)
        )),
    ]);
    let tx_line = Line::from(vec![
        colored(format!("▲ {}", format::rate(tx_rate)), Color::White),
        dim(format!("  total {}", format::bytes(tx_total))),
    ]);
    frame.render_widget(
        Paragraph::new(rx_line),
        Rect {
            height: 1,
            ..rx_area
        },
    );
    let tx_label_y = tx_area.y + tx_area.height.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(tx_line),
        Rect {
            y: tx_label_y,
            height: 1,
            ..tx_area
        },
    );
}

// ---------------------------------------------------------------- processes

fn draw_processes(
    frame: &mut Frame,
    app: &App,
    snapshot: &Snapshot,
    area: Rect,
    state: &mut TableState,
) -> ProcTableHit {
    let rows = app.process_rows();
    let arrow = if app.sort_descending { "▼" } else { "▲" };
    let mut title = vec![dim(format!(
        " {} of {} ",
        rows.len(),
        snapshot.process_count
    ))];
    if app.editing_filter || !app.filter.is_empty() {
        let cursor = if app.editing_filter { "_" } else { "" };
        title.push(colored(format!("filter: {}{cursor} ", app.filter), WARN));
    }
    let footer = match app.status() {
        Some(status) => Line::from(colored(format!(" {status} "), WARN)),
        None => Line::from(dim(
            " ↑↓ select  ←→ sort  r reverse  / filter  t kthreads  k term  K kill  +- interval  g graph span  q quit ",
        )),
    };
    let block = panel("proc", PROC_TITLE)
        .title_top(Line::from(title).right_aligned())
        .title_bottom(footer);

    let header_cell = |key: Option<SortKey>, text: &str| match key {
        Some(key) if app.sort_key == key => Cell::from(format!("{text}{arrow}"))
            .style(Style::default().fg(PROC_TITLE).add_modifier(Modifier::BOLD)),
        _ => Cell::from(text.to_owned()),
    };
    let has_gpus = !snapshot.gpus.is_empty();
    // Thread counts are not available everywhere (e.g. Windows).
    let has_threads = snapshot.processes.iter().any(|p| p.threads.is_some());
    let columns = process_columns(area.width.saturating_sub(2), has_gpus, has_threads);
    let header = Row::new(
        columns
            .iter()
            .map(|c| header_cell(c.sort_key(), c.header())),
    )
    .style(Style::default().add_modifier(Modifier::BOLD));

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|row| {
            let p = row.info;
            let cpu = p.cpu_percent as f64;
            Row::new(columns.iter().map(|column| match column {
                ProcColumn::Pid => Cell::from(p.pid.to_string()),
                ProcColumn::Program => Cell::from(p.name.clone()),
                ProcColumn::Command => {
                    Cell::from(Span::styled(p.command.clone(), Style::default().fg(DIM)))
                }
                ProcColumn::Threads => {
                    Cell::from(p.threads.map_or(String::new(), |t| t.to_string()))
                }
                ProcColumn::User => Cell::from(p.user.clone()),
                ProcColumn::Memory => Cell::from(format::bytes(p.memory_bytes)),
                ProcColumn::Cpu => Cell::from(Span::styled(
                    format!("{cpu:.1}"),
                    Style::default().fg(if cpu > 0.0 { percent_color(cpu) } else { DIM }),
                )),
                ProcColumn::Gpu => {
                    Cell::from(row.gpu_percent.map_or(String::new(), |g| format!("{g:.0}")))
                }
                ProcColumn::Vram => Cell::from(Span::styled(
                    row.vram_bytes.map_or(String::new(), format::bytes),
                    Style::default().fg(GPU_TITLE),
                )),
            }))
        })
        .collect();

    let widths: Vec<Constraint> = columns.iter().map(|c| c.constraint()).collect();
    let table = Table::new(table_rows, widths.clone())
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(45, 55, 80))
                .add_modifier(Modifier::BOLD),
        );
    // When the list shrinks (filter, processes exiting), ratatui keeps the
    // old scroll offset and leaves rows hidden above an empty area. Pull the
    // offset back so the table is filled from the top whenever possible.
    let visible_rows = area.height.saturating_sub(3) as usize; // borders + header
    let max_offset = rows.len().saturating_sub(visible_rows);
    if state.offset() > max_offset {
        *state.offset_mut() = max_offset;
    }
    frame.render_stateful_widget(table, area, state);

    // Same column split as ratatui's Table (Flex::Start, one space apart),
    // so clicks can be mapped back to columns and rows.
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let column_rects = Layout::horizontal(widths)
        .flex(Flex::Start)
        .spacing(1)
        .split(inner);
    ProcTableHit {
        header_y: inner.y,
        rows: Rect {
            y: inner.y + 1,
            height: inner.height.saturating_sub(1),
            ..inner
        },
        columns: columns
            .iter()
            .zip(column_rects.iter())
            .map(|(column, rect)| (rect.x, rect.width, column.sort_key()))
            .collect(),
        offset: state.offset(),
    }
}

/// Columns of the process table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcColumn {
    Pid,
    Program,
    Command,
    Threads,
    User,
    Memory,
    Cpu,
    Gpu,
    Vram,
}

impl ProcColumn {
    /// Display order.
    const ORDER: [ProcColumn; 9] = [
        ProcColumn::Pid,
        ProcColumn::Program,
        ProcColumn::Command,
        ProcColumn::Threads,
        ProcColumn::User,
        ProcColumn::Memory,
        ProcColumn::Cpu,
        ProcColumn::Gpu,
        ProcColumn::Vram,
    ];

    fn header(self) -> &'static str {
        match self {
            ProcColumn::Pid => "Pid",
            ProcColumn::Program => "Program",
            ProcColumn::Command => "Command",
            ProcColumn::Threads => "Thr",
            ProcColumn::User => "User",
            ProcColumn::Memory => "Mem",
            ProcColumn::Cpu => "Cpu%",
            ProcColumn::Gpu => "Gpu%",
            ProcColumn::Vram => "VRAM",
        }
    }

    fn sort_key(self) -> Option<SortKey> {
        match self {
            ProcColumn::Pid => Some(SortKey::Pid),
            ProcColumn::Program => Some(SortKey::Name),
            ProcColumn::Command => None,
            ProcColumn::Threads => Some(SortKey::Threads),
            ProcColumn::User => Some(SortKey::User),
            ProcColumn::Memory => Some(SortKey::Memory),
            ProcColumn::Cpu => Some(SortKey::Cpu),
            ProcColumn::Gpu => Some(SortKey::Gpu),
            ProcColumn::Vram => Some(SortKey::Vram),
        }
    }

    /// Width used to decide whether the column fits (Command: its minimum).
    fn width(self) -> u16 {
        match self {
            ProcColumn::Pid => 7,
            ProcColumn::Program => 15,
            ProcColumn::Command => 12,
            ProcColumn::Threads => 4,
            ProcColumn::User => 9,
            ProcColumn::Memory => 6,
            ProcColumn::Cpu => 6,
            ProcColumn::Gpu => 5,
            ProcColumn::Vram => 6,
        }
    }

    fn constraint(self) -> Constraint {
        match self {
            // The command line takes all remaining width.
            ProcColumn::Command => Constraint::Fill(1),
            other => Constraint::Length(other.width()),
        }
    }
}

/// Columns that fit in `width`: Pid, Program, Mem and Cpu% always, then
/// User, Thr, Gpu%, VRAM and Command as space allows (GPU columns only on
/// hosts with GPUs, Thr only where thread counts exist). Returned in
/// display order.
fn process_columns(width: u16, has_gpus: bool, has_threads: bool) -> Vec<ProcColumn> {
    let mut chosen = vec![
        ProcColumn::Pid,
        ProcColumn::Program,
        ProcColumn::Memory,
        ProcColumn::Cpu,
    ];
    let optional = [
        ProcColumn::User,
        ProcColumn::Threads,
        ProcColumn::Gpu,
        ProcColumn::Vram,
        ProcColumn::Command,
    ];
    // Every column is followed by one space of column spacing.
    let used = |cols: &[ProcColumn]| cols.iter().map(|c| c.width() + 1).sum::<u16>();
    for column in optional {
        if !has_gpus && matches!(column, ProcColumn::Gpu | ProcColumn::Vram) {
            continue;
        }
        if !has_threads && column == ProcColumn::Threads {
            continue;
        }
        let mut candidate = chosen.clone();
        candidate.push(column);
        if used(&candidate) <= width + 1 {
            chosen = candidate;
        }
    }
    ProcColumn::ORDER
        .into_iter()
        .filter(|c| chosen.contains(c))
        .collect()
}

fn draw_confirm(frame: &mut Frame, area: Rect, pid: u32, name: &str, signal: &str) {
    let width = 52.min(area.width);
    let height = 5.min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = panel("confirm", BAD).border_style(Style::default().fg(BAD));
    let text = vec![
        Line::from(vec![
            Span::raw(format!("Send {signal} to ")),
            colored(format!("{pid} ({name})"), Color::White),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(dim("y: send    any other key: cancel")),
    ];
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .alignment(Alignment::Center),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rigtop_core::Config;
    use rigtop_core::snapshot::{
        BtrfsDeviceStats, BtrfsFilesystem, CpuStats, DiskIo, GpuStats, HostInfo, MdArray,
        ProcessInfo,
    };
    use rigtop_core::worker::Event;
    use std::time::Duration;

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            host: HostInfo {
                hostname: "zefat".to_owned(),
                uptime_secs: 3600,
                ..Default::default()
            },
            cpu: CpuStats {
                model: "Test CPU".to_owned(),
                total_percent: 12.0,
                per_core_percent: vec![10.0; 32],
                ..Default::default()
            },
            gpus: vec![GpuStats {
                name: "NVIDIA GeForce RTX 4090".to_owned(),
                utilization_percent: Some(50.0),
                memory_used_bytes: Some(4 << 30),
                memory_total_bytes: Some(24 << 30),
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
            raid: rigtop_core::snapshot::RaidStats {
                btrfs: vec![BtrfsFilesystem {
                    fsid: "acb63bf0-ab89".to_owned(),
                    mount_points: vec!["/".to_owned()],
                    devices: vec!["nvme0n1".to_owned()],
                    device_stats: vec![BtrfsDeviceStats {
                        devid: 2,
                        corruption_errs: 3,
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
            processes: vec![ProcessInfo {
                pid: 4242,
                name: "python".to_owned(),
                cpu_percent: 99.0,
                ..Default::default()
            }],
            process_count: 1,
            ..Default::default()
        }
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_all_boxes_with_storage_health() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        app.apply(Event::Snapshot(Box::new(sample_snapshot())));
        let screen = render(&mut app, 160, 50);
        for expected in [
            "cpu", "zefat", "mem", "gpu", "RTX 4090", "storage", "net", "proc", "python",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}\n{screen}");
        }
        // Storage health is the point of rigtop: SMART, btrfs errors and md state.
        assert!(screen.contains("SMART ok wear 3%"), "{screen}");
        assert!(screen.contains("errors 3 (devid 2: 3)"), "{screen}");
        assert!(screen.contains("DEGRADED"), "{screen}");
        // All 32 cores fit in the grid.
        assert!(screen.contains("C31"), "{screen}");
    }

    #[test]
    fn laptop_shows_soc_power_and_battery() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        let mut snapshot = sample_snapshot();
        // RAPL unreadable (root only), an AMD APU and a battery.
        snapshot.power.cpu_package_w = None;
        snapshot.power.igpu_w = Some(6.0);
        snapshot.power.battery = Some(rigtop_core::snapshot::BatteryStats {
            percent: Some(72.0),
            status: "Discharging".to_owned(),
            power_w: Some(12.3),
        });
        app.apply(Event::Snapshot(Box::new(snapshot)));
        let screen = render(&mut app, 160, 50);
        assert!(screen.contains("Power SoC 6W"), "{screen}");
        assert!(screen.contains("Bat 72% -12W"), "{screen}");
    }

    #[test]
    fn hints_at_librehardwaremonitor_when_unreachable() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        let mut snapshot = sample_snapshot();
        snapshot.lhm_error = Some("127.0.0.1:8085: connection refused".to_owned());
        app.apply(Event::Snapshot(Box::new(snapshot)));
        let screen = render(&mut app, 160, 50);
        assert!(screen.contains(LHM_HINT), "{screen}");
    }

    #[test]
    fn shrinking_list_resets_stale_scroll_offset() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        let mut snapshot = sample_snapshot();
        snapshot.processes = (1..=200)
            .map(|pid| ProcessInfo {
                pid,
                name: format!("proc{pid}"),
                ..Default::default()
            })
            .collect();
        app.apply(Event::Snapshot(Box::new(snapshot)));
        app.sort_key = SortKey::Pid;
        app.sort_descending = false;
        // Scroll far down, then filter down to a handful of rows.
        app.selected_pid = Some(190);
        render(&mut app, 160, 50);
        assert!(app.table_state.offset() > 0);
        // 11 rows (proc19, proc190..199) with the last one selected: all of
        // them fit, so none may stay hidden above the visible area.
        app.filter = "proc19".to_owned();
        app.selected_pid = Some(199);
        let screen = render(&mut app, 160, 50);
        assert_eq!(app.table_state.offset(), 0);
        assert!(screen.contains("proc19 "), "{screen}");
    }

    #[test]
    fn storage_lines_win_space_on_short_terminals() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        let mut snapshot = sample_snapshot();
        snapshot.gpus.push(snapshot.gpus[0].clone());
        app.apply(Event::Snapshot(Box::new(snapshot)));
        // 120x40 with 32 cores and 2 GPUs: the CPU box takes 12 lines.
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("DEGRADED"), "{screen}");
        assert!(screen.contains("devid 2"), "{screen}");
    }

    #[test]
    fn cpu_layout_degrades_on_small_terminals() {
        // 32 cores on 210x70: graph + meters, grown so that no row is blank.
        let big = cpu_layout(32, Rect::new(0, 0, 210, 70));
        assert!(big.graph && big.meters);
        let (_, rows, _) = core_grid_layout(
            32,
            big.grid_width(210),
            big.height - 4,
            big.cell_min_width(),
        );
        assert_eq!(big.height, rows as u16 + 4);
        assert!(big.height <= 70 * CPU_BOX_MAX_PERCENT / 100);

        // 80x24: neither graph+meters (11 rows) nor meters only fit in 10 lines.
        let small = cpu_layout(32, Rect::new(0, 0, 80, 24));
        assert!(!small.graph && !small.meters);
        assert!(small.height <= 10, "{small:?}");
    }

    #[test]
    fn left_column_hides_boxes_that_do_not_fit() {
        let snapshot = sample_snapshot();
        // Tall: everything shown, the network box absorbs the rest.
        let tall = left_column_heights(&snapshot, Rect::new(0, 0, 90, 60));
        assert!(
            tall.memory > 0 && tall.gpu > 0 && tall.storage > 0 && tall.network >= NET_MIN_HEIGHT
        );
        assert_eq!(tall.memory + tall.gpu + tall.storage + tall.network, 60);
        // Short: memory and storage win, the rest is hidden rather than squashed.
        let short = left_column_heights(&snapshot, Rect::new(0, 0, 90, 12));
        assert!(short.memory > 0 && short.storage > 0);
        assert_eq!(short.gpu, 0);
        assert!(short.memory + short.gpu + short.storage + short.network <= 12);
    }

    #[test]
    fn process_columns_keep_essentials_when_narrow() {
        let narrow = process_columns(36, true, true);
        assert_eq!(
            narrow,
            vec![
                ProcColumn::Pid,
                ProcColumn::Program,
                ProcColumn::Memory,
                ProcColumn::Cpu
            ]
        );
        let wide = process_columns(120, true, true);
        assert_eq!(wide, ProcColumn::ORDER.to_vec());
        // No GPU columns on hosts without GPUs.
        assert!(!process_columns(120, false, true).contains(&ProcColumn::Vram));
        // No Thr column where thread counts are unknown (Windows).
        assert!(!process_columns(120, true, false).contains(&ProcColumn::Threads));
    }

    #[test]
    fn clock_and_clickable_headers_after_render() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut app = App::new(Config::default(), Duration::from_secs(1));
        app.apply(Event::Snapshot(Box::new(sample_snapshot())));
        let screen = render(&mut app, 160, 50);
        // "HH:MM:SS" somewhere on the first line.
        let first_line = screen.lines().next().unwrap();
        assert!(
            first_line.as_bytes().windows(8).any(|w| {
                w[2] == b':'
                    && w[5] == b':'
                    && w.iter()
                        .enumerate()
                        .all(|(i, c)| i == 2 || i == 5 || c.is_ascii_digit())
            }),
            "{first_line}"
        );

        let hit = app.proc_hit.clone().expect("process table geometry");
        let (x, _, _) = *hit
            .columns
            .iter()
            .find(|c| c.2 == Some(SortKey::Memory))
            .unwrap();
        // The header text at that position really is "Mem".
        let header: String = screen
            .lines()
            .nth(hit.header_y as usize)
            .unwrap()
            .chars()
            .skip(x as usize)
            .take(3)
            .collect();
        assert_eq!(header, "Mem");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: hit.header_y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.sort_key, SortKey::Memory);
    }

    #[test]
    fn small_terminal_does_not_panic() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        app.apply(Event::Snapshot(Box::new(sample_snapshot())));
        for (w, h) in [(20, 10), (1, 1), (80, 24), (44, 8)] {
            render(&mut app, w, h);
        }
    }

    #[test]
    fn waiting_screen_before_first_snapshot() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        assert!(render(&mut app, 40, 5).contains("collecting"));
    }
}
