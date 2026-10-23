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
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row, Table, TableState};
use rigtop_core::Snapshot;
use rigtop_core::snapshot::{SmartDevice, device_matches_disk};

use super::app::{App, SortKey, signal_name};
use super::format;
use super::graph::{self, BrailleGraph};

/// Box border color.
const BORDER: Color = Color::Rgb(85, 85, 100);
/// Secondary text (units, labels, commands).
const DIM: Color = Color::Rgb(125, 125, 140);
/// Unfilled part of meters.
const METER_EMPTY: Color = Color::Rgb(55, 55, 65);
/// Title colors per box.
const CPU_TITLE: Color = Color::Rgb(120, 210, 120);
const MEM_TITLE: Color = Color::Rgb(230, 200, 90);
const GPU_TITLE: Color = Color::Rgb(200, 130, 230);
const STORAGE_TITLE: Color = Color::Rgb(90, 200, 220);
const NET_TITLE: Color = Color::Rgb(110, 150, 240);
const PROC_TITLE: Color = Color::Rgb(240, 150, 90);
/// Status colors.
const OK: Color = Color::Rgb(110, 200, 110);
const WARN: Color = Color::Rgb(230, 200, 70);
const BAD: Color = Color::Rgb(235, 80, 80);

/// Width of one per-core cell: "C12 ■■■■■ 100% ".
const CORE_CELL_WIDTH: u16 = 15;
/// Lines below the core grid (load/temperatures, power/fans).
const CPU_INFO_LINES: u16 = 2;
/// Lines per GPU with and without the utilization history graph.
const GPU_ROWS_FULL: u16 = 3;
const GPU_ROWS_COMPACT: u16 = 2;
/// SMART results older than this are flagged as stale.
const SMART_STALE_SECS: u64 = 3600;

/// Draws one frame.
pub fn draw(frame: &mut Frame, app: &mut App) {
    app.sync_selection();
    // The table state is mutated while rendering rows that borrow `app`;
    // move it out for the duration of the frame.
    let mut table_state = std::mem::take(&mut app.table_state);
    draw_screen(frame, app, &mut table_state);
    app.table_state = table_state;
}

fn draw_screen(frame: &mut Frame, app: &App, table_state: &mut TableState) {
    let area = frame.area();
    let Some(snapshot) = &app.snapshot else {
        let waiting = Paragraph::new("collecting…")
            .alignment(Alignment::Center)
            .style(Style::default().fg(DIM));
        let [_, middle, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        frame.render_widget(waiting, middle);
        return;
    };

    let cpu_height = cpu_box_height(snapshot, area.width);
    let [top, bottom] =
        Layout::vertical([Constraint::Length(cpu_height), Constraint::Fill(1)]).areas(area);
    draw_cpu(frame, app, snapshot, top);

    let left_width = (area.width * 45 / 100).max(44).min(area.width);
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Fill(1)]).areas(bottom);

    let heights = left_column_heights(snapshot, left);
    let [mem, gpu, storage, net] = Layout::vertical([
        Constraint::Length(heights.memory),
        Constraint::Length(heights.gpu),
        Constraint::Fill(1),
        Constraint::Length(heights.network),
    ])
    .areas(left);
    draw_memory(frame, snapshot, mem);
    if heights.gpu > 0 {
        draw_gpus(frame, app, snapshot, gpu);
    }
    draw_storage(frame, snapshot, storage);
    draw_network(frame, app, snapshot, net);
    draw_processes(frame, app, snapshot, right, table_state);

    if let Some(confirm) = &app.confirm {
        draw_confirm(
            frame,
            area,
            confirm.pid,
            &confirm.name,
            signal_name(confirm.signal),
        );
    }
}

/// Box heights of the left column (storage takes the rest).
struct LeftHeights {
    memory: u16,
    gpu: u16,
    network: u16,
}

/// Distributes the left column so that the storage box, rigtop's main
/// addition over btop, shows all its lines whenever possible: the GPU history
/// graphs are dropped first, then the network box shrinks.
fn left_column_heights(snapshot: &Snapshot, area: Rect) -> LeftHeights {
    let gpus = snapshot.gpus.len() as u16;
    let storage_needed = storage_lines(snapshot, area.width.saturating_sub(2)).len() as u16 + 2;
    let mut heights = LeftHeights {
        memory: 7,
        gpu: if gpus == 0 {
            0
        } else {
            gpus * GPU_ROWS_FULL + 2
        },
        network: 8,
    };
    let total = |h: &LeftHeights| h.memory + h.gpu + h.network + storage_needed;
    if total(&heights) > area.height && gpus > 0 {
        heights.gpu = gpus * GPU_ROWS_COMPACT + 2;
    }
    if total(&heights) > area.height {
        heights.network = 5;
    }
    heights
}

/// A rounded box with a colored title, btop style.
fn panel(title: &str, color: Color) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
}

/// A horizontal meter of `width` cells, colored along its length.
fn meter(width: usize, ratio: f64) -> Vec<Span<'static>> {
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
fn percent_color(percent: f64) -> Color {
    graph::heat_color(percent / 100.0)
}

/// Color for a temperature: green below ~40 °C, red from ~85 °C.
fn temperature_color(celsius: f32) -> Color {
    graph::heat_color((celsius as f64 - 40.0) / 45.0)
}

fn dim(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(DIM))
}

fn colored(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(color))
}

// ---------------------------------------------------------------- CPU

/// Height of the CPU box so that every core fits in the grid.
fn cpu_box_height(snapshot: &Snapshot, width: u16) -> u16 {
    let cores = snapshot.cpu.per_core_percent.len().max(1) as u16;
    let columns = (core_area_width(width) / CORE_CELL_WIDTH).max(1);
    let rows = cores.div_ceil(columns);
    (rows + CPU_INFO_LINES + 2).max(7)
}

/// Width available for the core grid inside a CPU box of `width`.
fn core_area_width(width: u16) -> u16 {
    let inner = width.saturating_sub(2);
    inner - inner * 40 / 100
}

fn draw_cpu(frame: &mut Frame, app: &App, snapshot: &Snapshot, area: Rect) {
    let cpu = &snapshot.cpu;
    let host = &snapshot.host;
    let block = panel("cpu", CPU_TITLE).title_top(
        Line::from(vec![
            dim(" "),
            colored(host.hostname.clone(), Color::White),
            dim(format!(" up {} ", format::duration(host.uptime_secs))),
        ])
        .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let graph_width = inner.width * 40 / 100;
    let [graph_area, cores_area] =
        Layout::horizontal([Constraint::Length(graph_width), Constraint::Fill(1)]).areas(inner);

    let samples = app
        .cpu_history
        .last(BrailleGraph::capacity(graph_area.width));
    frame.render_widget(BrailleGraph::new(&samples, 100.0), graph_area);
    let label = Line::from(vec![
        colored(
            format!("{:.0}%", cpu.total_percent),
            percent_color(cpu.total_percent as f64),
        ),
        dim(format!(" {}", cpu.model)),
    ]);
    frame.render_widget(
        Paragraph::new(label),
        Rect {
            height: 1,
            ..graph_area
        },
    );

    let [grid_area, info_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(CPU_INFO_LINES)])
            .areas(cores_area);
    draw_core_grid(frame, &cpu.per_core_percent, grid_area);
    frame.render_widget(Paragraph::new(cpu_info_lines(app, snapshot)), info_area);
}

fn draw_core_grid(frame: &mut Frame, cores: &[f32], area: Rect) {
    if area.width < CORE_CELL_WIDTH || area.height == 0 {
        return;
    }
    let columns = (area.width / CORE_CELL_WIDTH) as usize;
    let rows = cores.len().div_ceil(columns);
    let digits = cores.len().saturating_sub(1).to_string().len();
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
            spans.extend(meter(5, usage as f64 / 100.0));
            spans.push(colored(
                format!("{usage:>4.0}%"),
                percent_color(usage as f64),
            ));
            let used = 2 + digits + 5 + 5;
            spans.push(Span::raw(
                " ".repeat((CORE_CELL_WIDTH as usize).saturating_sub(used)),
            ));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Load average and temperatures, then power and fans.
fn cpu_info_lines(app: &App, snapshot: &Snapshot) -> Vec<Line<'static>> {
    let cpu = &snapshot.cpu;
    let config = &app.config;

    let mut first = vec![
        dim("Load "),
        Span::raw(format!(
            "{:.2} {:.2} {:.2}",
            cpu.load_average[0], cpu.load_average[1], cpu.load_average[2]
        )),
    ];
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
        first.push(dim(format!("  {} ", config.temperature_label(temp))));
        first.push(colored(
            format!("{:.0}°C", temp.celsius),
            temperature_color(temp.celsius),
        ));
    }

    let power = &snapshot.power;
    let watts = |w: Option<f32>| w.map_or("--".to_owned(), |w| format!("{w:.0}W"));
    let mut second = vec![
        dim("Power CPU "),
        Span::raw(watts(power.cpu_package_w)),
        dim(" GPU "),
        Span::raw(watts(power.gpus_w)),
        dim(" Σ "),
        colored(watts(power.total_w), Color::White),
    ];
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

    let total = memory.total_bytes.max(1) as f64;
    let swap_total = memory.swap_total_bytes.max(1) as f64;
    let mut rows: Vec<(&str, u64, f64)> = vec![
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
        rows.push((
            "Swap",
            memory.swap_used_bytes,
            memory.swap_used_bytes as f64 / swap_total,
        ));
    }

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

// ---------------------------------------------------------------- GPU

fn draw_gpus(frame: &mut Frame, app: &App, snapshot: &Snapshot, area: Rect) {
    let block = panel("gpu", GPU_TITLE);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Drop the history graphs when the layout gave only compact rows.
    let rows_per_gpu = if inner.height >= snapshot.gpus.len() as u16 * GPU_ROWS_FULL {
        GPU_ROWS_FULL
    } else {
        GPU_ROWS_COMPACT
    };
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
        line2.push(colored(format!("{util:>3.0}% "), percent_color(util)));
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
        if rows_per_gpu < GPU_ROWS_FULL {
            continue;
        }
        let samples = app
            .gpu_history
            .get(i)
            .map(|h| h.last(BrailleGraph::capacity(inner.width)))
            .unwrap_or_default();
        frame.render_widget(
            BrailleGraph::new(&samples, 100.0),
            Rect { y: y + 2, ..rows },
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
        let mut spans = vec![Span::raw(format!("{:<13} ", format::truncate(&name, 13)))];
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
            Span::raw(format!("{:<8}", disk.name)),
            dim(" R "),
            Span::raw(format!("{:>7}", format::rate(disk.read_bytes_per_sec))),
            dim(" W "),
            Span::raw(format!("{:>7}", format::rate(disk.write_bytes_per_sec))),
            colored(
                format!(" {:>3.0}%", disk.busy_percent),
                percent_color(disk.busy_percent as f64),
            ),
        ];
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
        let mut spans = vec![Span::raw(format!("{:<8}", smart.device))];
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
        let missing = fs.device_stats.iter().filter(|d| d.missing).count();
        spans.push(dim(format!("{} devs ", fs.devices.len())));
        let errors: u64 = fs.device_stats.iter().map(|d| d.total_errors()).sum();
        if missing > 0 {
            spans.push(colored(format!("{missing} MISSING "), BAD));
        }
        if errors > 0 {
            let detail: Vec<String> = fs
                .device_stats
                .iter()
                .filter(|d| d.total_errors() > 0)
                .map(|d| format!("devid {}: {}", d.devid, d.total_errors()))
                .collect();
            spans.push(colored(
                format!("errors {errors} ({})", detail.join(", ")),
                BAD,
            ));
        } else if missing == 0 {
            spans.push(colored("no errors", OK));
        }
        lines.push(Line::from(spans));
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

/// "SMART ok wear 3%" / "SMART FAIL" / "SMART stale", for one drive.
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
    let stale = smart
        .collected_at_ms
        .is_some_and(|ms| now_secs.saturating_sub(ms / 1000) > SMART_STALE_SECS);
    if stale {
        spans.push(colored(" stale", WARN));
    }
    spans
}

// ---------------------------------------------------------------- network

fn draw_network(frame: &mut Frame, app: &App, snapshot: &Snapshot, area: Rect) {
    let name = app.net_interface.clone().unwrap_or_else(|| "-".to_owned());
    let block = panel("net", NET_TITLE).title_top(
        Line::from(vec![
            colored(format!(" {name} "), Color::White),
            dim("n:next "),
        ])
        .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    let capacity = BrailleGraph::capacity(inner.width);
    let rx = app.net_rx_history.last(capacity);
    let tx = app.net_tx_history.last(capacity);
    // Shared scale so download and upload are comparable; at least 10 KiB/s
    // so that idle noise does not fill the graph.
    let max = app
        .net_rx_history
        .max_of_last(capacity)
        .max(app.net_tx_history.max_of_last(capacity))
        .max(10.0 * 1024.0);

    let [rx_area, tx_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(inner);
    frame.render_widget(BrailleGraph::new(&rx, max).color(graph::rx_color), rx_area);
    frame.render_widget(
        BrailleGraph::new(&tx, max)
            .color(graph::tx_color)
            .inverted(true),
        tx_area,
    );

    let iface = snapshot
        .networks
        .iter()
        .find(|n| Some(&n.name) == app.net_interface.as_ref());
    let (rx_rate, tx_rate, rx_total, tx_total) = iface.map_or((0.0, 0.0, 0, 0), |n| {
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
) {
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
            " ↑↓ select  ←→ sort  r reverse  / filter  t kthreads  k term  K kill  +- interval  q quit ",
        )),
    };
    let block = panel("proc", PROC_TITLE)
        .title_top(Line::from(title).right_aligned())
        .title_bottom(footer);

    let header_cell = |key: SortKey, text: &str| {
        if app.sort_key == key {
            Cell::from(format!("{text}{arrow}"))
                .style(Style::default().fg(PROC_TITLE).add_modifier(Modifier::BOLD))
        } else {
            Cell::from(text.to_owned())
        }
    };
    let header = Row::new(vec![
        header_cell(SortKey::Pid, "Pid"),
        header_cell(SortKey::Name, "Program"),
        Cell::from("Command"),
        header_cell(SortKey::Threads, "Thr"),
        header_cell(SortKey::User, "User"),
        header_cell(SortKey::Memory, "Mem"),
        header_cell(SortKey::Cpu, "Cpu%"),
        header_cell(SortKey::Gpu, "Gpu%"),
        header_cell(SortKey::Vram, "VRAM"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|row| {
            let p = row.info;
            let cpu = p.cpu_percent as f64;
            Row::new(vec![
                Cell::from(p.pid.to_string()),
                Cell::from(p.name.clone()),
                Cell::from(Span::styled(p.command.clone(), Style::default().fg(DIM))),
                Cell::from(p.threads.map_or(String::new(), |t| t.to_string())),
                Cell::from(p.user.clone()),
                Cell::from(format::bytes(p.memory_bytes)),
                Cell::from(Span::styled(
                    format!("{cpu:.1}"),
                    Style::default().fg(if cpu > 0.0 { percent_color(cpu) } else { DIM }),
                )),
                Cell::from(row.gpu_percent.map_or(String::new(), |g| format!("{g:.0}"))),
                Cell::from(Span::styled(
                    row.vram_bytes.map_or(String::new(), format::bytes),
                    Style::default().fg(GPU_TITLE),
                )),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Length(15),
        Constraint::Fill(1),
        Constraint::Length(4),
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Length(6),
    ];
    let table = Table::new(table_rows, widths)
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
    use crate::tui::worker::Event;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rigtop_core::Config;
    use rigtop_core::snapshot::{
        BtrfsDeviceStats, BtrfsFilesystem, CpuStats, DiskIo, GpuStats, HostInfo, MdArray,
        ProcessInfo,
    };
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
