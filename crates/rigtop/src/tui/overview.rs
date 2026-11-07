//! Multi-host views of the TUI: the tab bar and the overview of all hosts.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use rigtop_core::config::{IntegratedGpu, OverviewSection};
use rigtop_core::format;
use rigtop_core::snapshot::Snapshot;
use rigtop_core::view::{Severity, attention_rank};

use super::HostTui;
use super::app::App;
use super::graph::{self, BrailleGraph};
use super::ui::{
    BAD, BORDER, DIM, GPU_TITLE, OK, PROC_TITLE, WARN, colored, dim, link_label, meter,
    percent_color, temperature_color,
};

/// What a host box shows, top to bottom: lines of text, and history
/// graphs given the rows the text leaves over.
enum BoxRow {
    Text(Line<'static>),
    /// CPU usage history, this many rows high.
    CpuGraph(u16),
    /// Download over upload history of the network interface shown.
    NetGraph(u16),
}

/// Rows a graph needs to be worth drawing, and the most it gets.
const MIN_GRAPH_ROWS: usize = 2;
const MAX_CPU_GRAPH_ROWS: usize = 20;
const MAX_NET_GRAPH_ROWS: usize = 14;

/// Narrowest host box; more hosts than fit side by side wrap to new rows.
const BOX_MIN_WIDTH: u16 = 52;
/// Data older than this marks a host as stale.
const STALE_AFTER: Duration = Duration::from_secs(10);
/// Alerts listed per host box.
const MAX_ALERTS: usize = 3;

/// Disks listed with their I/O when a box has room.
const MAX_DISKS: usize = 4;
/// Columns taken by the label before a meter ("CPU  ").
const LABEL_WIDTH: usize = 5;
/// Columns kept for the value after a meter.
const VALUE_WIDTH: usize = 24;

/// What the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Host(usize),
}

/// Status color and description of a host.
pub fn host_status(host: &HostTui) -> (Color, String) {
    let app = &host.app;
    if let Some(error) = &app.connection_error {
        return (BAD, error.clone());
    }
    match app.last_update {
        None => (DIM, app.waiting_message.clone()),
        Some(at) if at.elapsed() > STALE_AFTER => {
            (WARN, format!("no data for {}s", at.elapsed().as_secs()))
        }
        Some(_) => {
            let critical = app.snapshot.as_ref().is_some_and(|s| {
                app.alerts(s)
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
fn attention(host: &HostTui) -> u8 {
    let app = &host.app;
    let alerts = app
        .snapshot
        .as_ref()
        .map(|s| app.alerts(s))
        .unwrap_or_default();
    let stale = app.last_update.is_some_and(|at| at.elapsed() > STALE_AFTER);
    attention_rank(&alerts, app.connection_error.is_some(), stale)
}

/// Draws "0 Overview  1 ● local  2 ● zefat" and returns the clickable
/// region of each tab.
pub fn draw_tabs(
    frame: &mut Frame,
    area: Rect,
    hosts: &[HostTui],
    view: View,
    unread: (usize, Option<Severity>),
    release_note: Option<&str>,
) -> Vec<(Rect, View)> {
    let mut spans = Vec::new();
    let mut regions = Vec::new();
    let mut x = area.x;
    let selected = Style::default()
        .bg(Color::Rgb(45, 55, 80))
        .add_modifier(Modifier::BOLD);
    let mut push_tab =
        |spans: &mut Vec<Span<'static>>, parts: Vec<Span<'static>>, target: View, x: &mut u16| {
            let width: u16 = parts.iter().map(|s| s.width() as u16).sum();
            let style = if target == view {
                selected
            } else {
                Style::default()
            };
            for part in parts {
                let part_style = part.style;
                spans.push(Span::styled(part.content, part_style.patch(style)));
            }
            regions.push((Rect::new(*x, area.y, width, 1), target));
            spans.push(Span::raw("  "));
            *x += width + 2;
        };
    push_tab(
        &mut spans,
        vec![dim(" 0 "), Span::raw("Overview ")],
        View::Overview,
        &mut x,
    );
    for (index, host) in hosts.iter().enumerate() {
        let (color, _) = host_status(host);
        let key = if index < 9 {
            format!(" {} ", index + 1)
        } else {
            "   ".to_owned()
        };
        push_tab(
            &mut spans,
            vec![
                dim(key),
                colored("● ", color),
                Span::raw(format!("{} ", host.spec.name)),
            ],
            View::Host(index),
            &mut x,
        );
    }
    if let Some(note) = release_note {
        spans.push(colored(note.to_owned(), OK));
        spans.push(Span::raw("  "));
    }
    if let Some(badge) = super::alert_log::unread_badge(unread) {
        spans.push(badge);
        spans.push(Span::raw("  "));
    }
    // In the overview, `u` updates every remote host with an older rigtop.
    let outdated = hosts
        .iter()
        .filter(|h| matches!(h.spec.kind, rigtop_core::hosts::HostKind::Remote(_)))
        .filter_map(|h| h.app.snapshot.as_ref())
        .filter(|s| rigtop_core::update::updatable(&s.host.rigtop_version, &s.host.os))
        .count();
    if view == View::Overview && outdated > 0 {
        spans.push(colored(format!("u: update agents ({outdated})"), WARN));
        spans.push(Span::raw("  "));
    }
    spans.push(dim("Tab/0-9: switch  a: alerts  ?: help"));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    regions
}

/// Draws one box per host with alerts and the configured sections.
pub fn draw_overview(
    frame: &mut Frame,
    area: Rect,
    hosts: &[HostTui],
    include_local: bool,
    processes: usize,
) {
    let mut visible: Vec<&HostTui> = hosts
        .iter()
        .filter(|h| include_local || !h.spec.is_local())
        .collect();
    if visible
        .first()
        .is_some_and(|h| h.app.config.overview.alerts_first)
    {
        // Stable: hosts of the same rank keep the configured order.
        visible.sort_by_key(|h| attention(h));
    }
    if visible.is_empty() {
        let text = "no hosts to show: add [[hosts]] to the configuration or use --host";
        frame.render_widget(Paragraph::new(dim(text)), area);
        return;
    }
    let columns = ((area.width / BOX_MIN_WIDTH).max(1) as usize).min(visible.len());
    let rows = visible.len().div_ceil(columns);
    let box_width = area.width / columns as u16;
    let box_height = area.height / rows as u16;
    for (row, row_hosts) in visible.chunks(columns).enumerate() {
        let row = row as u16;
        // Boxes of a row are laid out together so their sections line up.
        let mut cards = Vec::new();
        for (col, host) in row_hosts.iter().enumerate() {
            let col = col as u16;
            let width = if col as usize == columns - 1 {
                area.width - box_width * col
            } else {
                box_width
            };
            let rect = Rect::new(
                area.x + box_width * col,
                area.y + box_height * row,
                width,
                box_height,
            );
            let inner = draw_box_frame(frame, rect, host);
            match &host.app.snapshot {
                Some(snapshot) => cards.push((
                    inner,
                    Card {
                        app: &host.app,
                        snapshot,
                        sections: &host.spec.sections,
                        width: inner.width as usize,
                    },
                )),
                None => draw_waiting(frame, inner, &host.app),
            }
        }
        let (rects, cards): (Vec<Rect>, Vec<Card>) = cards.into_iter().unzip();
        let budget = rects.first().map_or(0, |r| r.height as usize);
        for ((inner, card), rows) in rects
            .iter()
            .zip(&cards)
            .zip(layout_row(&cards, budget, processes))
        {
            draw_rows(frame, *inner, card.app, rows);
        }
    }
    let hint = dim(" l: toggle this machine ");
    let hint_area = Rect::new(
        area.right().saturating_sub(24),
        area.bottom().saturating_sub(1),
        24,
        1,
    );
    if area.height > 4 {
        frame.render_widget(Paragraph::new(hint), hint_area);
    }
}

/// Draws a host's box with its title; the area inside it.
fn draw_box_frame(frame: &mut Frame, area: Rect, host: &HostTui) -> Rect {
    let (color, _) = host_status(host);
    let mut title_right = Vec::new();
    if let Some(snapshot) = &host.app.snapshot {
        if snapshot.host.hostname != host.spec.name {
            title_right.push(dim(format!(" {}", snapshot.host.hostname)));
        }
        if let Some(note) = rigtop_core::view::version_note(snapshot) {
            title_right.push(colored(format!(" {note}"), WARN));
        }
        if snapshot.host.reboot_required.is_some() {
            title_right.push(colored(
                format!(" {}", rigtop_core::view::REBOOT_NOTE),
                WARN,
            ));
        }
        if let Some((text, slow)) = rigtop_core::view::link_rtt_text(snapshot) {
            let text = format!(" {text}");
            title_right.push(if slow { colored(text, WARN) } else { dim(text) });
        }
        title_right.push(dim(format!(
            " up {} ",
            format::duration(snapshot.host.uptime_secs)
        )));
    }
    // " ● name " with the status dot in front of the host name.
    let title = Line::from(vec![
        Span::raw(" "),
        colored("● ", color),
        Span::styled(
            host.spec.name.clone(),
            Style::default().fg(PROC_TITLE).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(title)
        .title_top(Line::from(title_right).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// The box of a host still waiting for its first snapshot.
fn draw_waiting(frame: &mut Frame, inner: Rect, app: &App) {
    let mut lines = vec![Line::from(dim(app.waiting_message.clone()))];
    if let Some(note) = app.offline_note() {
        lines.push(Line::from(colored(note, BAD)));
    }
    if let Some(error) = &app.connection_error {
        lines.push(Line::from(colored(error.clone(), WARN)));
    }
    // Connection errors can be long: wrap them inside the box.
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// One host box of a row of boxes laid out together.
struct Card<'a> {
    app: &'a App,
    snapshot: &'a Snapshot,
    sections: &'a [OverviewSection],
    /// Columns inside the box.
    width: usize,
}

/// The rows of one section across the boxes of a row: each box's lines,
/// all given the height of the tallest.
struct Slot {
    section: OverviewSection,
    cells: Vec<Vec<Line<'static>>>,
    height: usize,
}

/// Lays out the boxes of one row of the overview, `budget` rows high
/// inside, so that each section starts on the same row in every box: a
/// section takes as many rows as it needs in the box that needs the most,
/// and the others are padded with blank lines.
fn layout_row(cards: &[Card], budget: usize, processes: usize) -> Vec<Vec<BoxRow>> {
    let Some(order) = section_order(cards) else {
        // Sections in different orders cannot line up: each box alone.
        return cards
            .iter()
            .flat_map(|card| layout_row(std::slice::from_ref(card), budget, processes))
            .collect();
    };
    let heads: Vec<Vec<Line<'static>>> = cards.iter().map(head_lines).collect();
    let head_height = heads.iter().map(Vec::len).max().unwrap_or(0);

    // Add whole sections while they fit; the process list may be shortened.
    let mut slots: Vec<Slot> = Vec::new();
    let mut used = head_height;
    for section in order {
        let left = budget.saturating_sub(used);
        let cells: Vec<Vec<Line<'static>>> = cards
            .iter()
            .map(|card| {
                if !card.sections.contains(&section) {
                    return Vec::new();
                }
                let lines = section_lines(card, section, left, processes);
                if section == OverviewSection::Processes && lines.len() <= 1 {
                    Vec::new()
                } else {
                    lines
                }
            })
            .collect();
        let height = cells.iter().map(Vec::len).max().unwrap_or(0);
        if height == 0 {
            continue;
        }
        if height > left {
            break;
        }
        used += height;
        slots.push(Slot {
            section,
            cells,
            height,
        });
    }

    // Rows to spare go first to details (CPU and GPU names, swap, disk
    // I/O), section by section, then to the CPU and network graphs.
    let mut spare = budget.saturating_sub(used);
    for slot in &mut slots {
        let detailed: Vec<Option<Vec<Line<'static>>>> = cards
            .iter()
            .zip(&slot.cells)
            .map(|(card, lines)| {
                if lines.is_empty() {
                    None
                } else {
                    section_details(card, slot.section, lines)
                }
            })
            .collect();
        let full = detailed
            .iter()
            .zip(&slot.cells)
            .map(|(d, lines)| d.as_ref().map_or(lines.len(), Vec::len))
            .max()
            .unwrap_or(0);
        let grow = full.saturating_sub(slot.height);
        if grow <= spare {
            spare -= grow;
            slot.height = full;
        }
        // Details that fit the section's height, even if it cannot grow:
        // a box with fewer lines than the tallest uses its padding.
        for (cell, detailed) in slot.cells.iter_mut().zip(detailed) {
            if let Some(detailed) = detailed
                && detailed.len() <= slot.height
            {
                *cell = detailed;
            }
        }
    }
    let has = |wanted| slots.iter().any(|slot| slot.section == wanted);
    let (cpu_rows, net_rows) = graph_rows(
        spare,
        has(OverviewSection::Cpu),
        has(OverviewSection::Network)
            && cards.iter().any(|c| c.app.history.net_interface.is_some()),
    );

    let pad = |rows: &mut Vec<BoxRow>, lines: Vec<Line<'static>>, height: usize| {
        let blank = height.saturating_sub(lines.len());
        rows.extend(lines.into_iter().map(BoxRow::Text));
        rows.extend((0..blank).map(|_| BoxRow::Text(Line::default())));
    };
    heads
        .into_iter()
        .enumerate()
        .map(|(i, head)| {
            let card = &cards[i];
            let mut rows = Vec::new();
            pad(&mut rows, head, head_height);
            for slot in &mut slots {
                let lines = std::mem::take(&mut slot.cells[i]);
                let shown = !lines.is_empty();
                pad(&mut rows, lines, slot.height);
                // A graph under its section, or blank rows where another
                // box has one.
                let graph = match slot.section {
                    OverviewSection::Cpu => {
                        Some((cpu_rows, shown, BoxRow::CpuGraph as fn(u16) -> BoxRow))
                    }
                    OverviewSection::Network => Some((
                        net_rows,
                        shown && card.app.history.net_interface.is_some(),
                        BoxRow::NetGraph as fn(u16) -> BoxRow,
                    )),
                    _ => None,
                };
                if let Some((height, drawn, make)) = graph
                    && height > 0
                {
                    if drawn {
                        rows.push(make(height));
                    } else {
                        pad(&mut rows, Vec::new(), height as usize);
                    }
                }
            }
            rows
        })
        .collect()
}

/// The order of the sections across `cards`, if every box lists them in
/// an order consistent with it (each may leave some out).
fn section_order(cards: &[Card]) -> Option<Vec<OverviewSection>> {
    let mut order: Vec<OverviewSection> = Vec::new();
    for card in cards {
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

/// The lines above the sections: a lost connection and the alerts.
fn head_lines(card: &Card) -> Vec<Line<'static>> {
    let app = card.app;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(error) = &app.connection_error {
        let note = app
            .offline_note()
            .unwrap_or_else(|| "disconnected".to_owned());
        lines.push(Line::from(colored(format!("{note}: {error}"), WARN)));
    }
    let alerts = app.alerts(card.snapshot);
    for alert in alerts.iter().take(MAX_ALERTS) {
        let color = if alert.severity == Severity::Critical {
            BAD
        } else {
            WARN
        };
        lines.push(Line::from(colored(format!("▲ {}", alert.text), color)));
    }
    if alerts.len() > MAX_ALERTS {
        lines.push(Line::from(colored(
            format!("  +{} more", alerts.len() - MAX_ALERTS),
            WARN,
        )));
    }
    lines
}

/// The lines of one section of a box, with `left` rows left for it.
fn section_lines(
    card: &Card,
    section: OverviewSection,
    left: usize,
    processes: usize,
) -> Vec<Line<'static>> {
    let (snapshot, width) = (card.snapshot, card.width);
    match section {
        OverviewSection::Cpu => cpu_lines(card.app, snapshot, width),
        OverviewSection::Gpu => gpu_lines(snapshot, width, card.app.config.gpu.integrated),
        OverviewSection::Memory => memory_lines(snapshot, width),
        OverviewSection::Processes => {
            process_lines(snapshot, processes.min(left.saturating_sub(1)))
        }
        OverviewSection::Network => network_lines(card.app, snapshot),
        OverviewSection::Storage => storage_lines(snapshot, width),
    }
}

/// A section's `lines` with details added, if it has any.
fn section_details(
    card: &Card,
    section: OverviewSection,
    lines: &[Line<'static>],
) -> Option<Vec<Line<'static>>> {
    let snapshot = card.snapshot;
    match section {
        OverviewSection::Cpu => cpu_details(snapshot, card.width, lines),
        OverviewSection::Gpu => gpu_details(snapshot, lines, card.app.config.gpu.integrated),
        OverviewSection::Memory => memory_details(snapshot, card.width, lines),
        OverviewSection::Storage => storage_details(snapshot, lines),
        _ => None,
    }
}

/// Splits `spare` rows between the CPU and network graphs (3:2, like the
/// GUI), each at least [`MIN_GRAPH_ROWS`] or nothing, within their caps.
fn graph_rows(spare: usize, cpu: bool, net: bool) -> (u16, u16) {
    let (cpu_share, net_share) = match (cpu, net) {
        (true, true) => (spare * 3 / 5, spare - spare * 3 / 5),
        (true, false) => (spare, 0),
        (false, true) => (0, spare),
        (false, false) => (0, 0),
    };
    let size = |rows: usize, cap: usize| {
        let rows = rows.min(cap);
        if rows >= MIN_GRAPH_ROWS {
            rows as u16
        } else {
            0
        }
    };
    (
        size(cpu_share, MAX_CPU_GRAPH_ROWS),
        size(net_share, MAX_NET_GRAPH_ROWS),
    )
}

/// Draws the rows of a box from the top of `area`.
fn draw_rows(frame: &mut Frame, area: Rect, app: &App, rows: Vec<BoxRow>) {
    let mut y = area.y;
    let bottom = area.y + area.height;
    let mut text: Vec<Line<'static>> = Vec::new();
    let flush = |frame: &mut Frame, text: &mut Vec<Line<'static>>, y: &mut u16| {
        let height = (text.len() as u16).min(bottom.saturating_sub(*y));
        if height > 0 {
            let rect = Rect {
                y: *y,
                height,
                ..area
            };
            frame.render_widget(Paragraph::new(std::mem::take(text)), rect);
        }
        text.clear();
        *y += height;
    };
    for row in rows {
        match row {
            BoxRow::Text(line) => text.push(line),
            BoxRow::CpuGraph(height) | BoxRow::NetGraph(height) => {
                flush(frame, &mut text, &mut y);
                let height = height.min(bottom.saturating_sub(y));
                if height == 0 {
                    continue;
                }
                let rect = Rect { y, height, ..area };
                let capacity = BrailleGraph::capacity(rect.width);
                if matches!(row, BoxRow::CpuGraph(_)) {
                    let samples = app
                        .history
                        .cpu
                        .graph(capacity, app.graph_span, app.interval);
                    frame.render_widget(BrailleGraph::new(&samples, 100.0), rect);
                } else {
                    let rx = app
                        .history
                        .net_rx
                        .graph(capacity, app.graph_span, app.interval);
                    let tx = app
                        .history
                        .net_tx
                        .graph(capacity, app.graph_span, app.interval);
                    // A shared scale of at least 10 KiB/s, as in the host view.
                    let max = rx
                        .iter()
                        .chain(&tx)
                        .fold(10.0 * 1024.0, |a: f64, &b| a.max(b));
                    let top = Rect {
                        height: height.div_ceil(2),
                        ..rect
                    };
                    let under = Rect {
                        y: rect.y + top.height,
                        height: height - top.height,
                        ..rect
                    };
                    frame.render_widget(BrailleGraph::new(&rx, max).color(graph::rx_color), top);
                    if under.height > 0 {
                        frame.render_widget(
                            BrailleGraph::new(&tx, max)
                                .color(graph::tx_color)
                                .inverted(true),
                            under,
                        );
                    }
                }
                y += height;
            }
        }
    }
    flush(frame, &mut text, &mut y);
}

/// The CPU lines with the model name under the meter.
fn cpu_details(
    snapshot: &Snapshot,
    width: usize,
    lines: &[Line<'static>],
) -> Option<Vec<Line<'static>>> {
    let model = snapshot.cpu.model.trim();
    if model.is_empty() {
        return None;
    }
    let mut detailed = lines.to_vec();
    detailed.insert(
        1.min(detailed.len()),
        Line::from(dim(format::truncate(model, width))),
    );
    Some(detailed)
}

/// The GPU lines, each after the GPU's name and power.
fn gpu_details(
    snapshot: &Snapshot,
    lines: &[Line<'static>],
    integrated: IntegratedGpu,
) -> Option<Vec<Line<'static>>> {
    let gpus: Vec<_> = rigtop_core::view::card_gpus(snapshot, integrated).collect();
    if gpus.is_empty() || gpus.len() != lines.len() {
        return None;
    }
    let mut detailed = Vec::new();
    for (gpu, line) in gpus.iter().zip(lines) {
        detailed.push(Line::from(dim(format!(
            "{:<LABEL_WIDTH$}{}",
            "",
            rigtop_core::view::gpu_detail(gpu)
        ))));
        detailed.push(line.clone());
    }
    Some(detailed)
}

/// The memory line with swap under it, when there is swap.
fn memory_details(
    snapshot: &Snapshot,
    width: usize,
    lines: &[Line<'static>],
) -> Option<Vec<Line<'static>>> {
    let memory = &snapshot.memory;
    if memory.swap_total_bytes == 0 {
        return None;
    }
    let ratio = memory.swap_used_bytes as f64 / memory.swap_total_bytes as f64;
    let value = Span::raw(format!(
        "{}/{} {:.0}%",
        format::bytes(memory.swap_used_bytes),
        format::bytes(memory.swap_total_bytes),
        ratio * 100.0
    ));
    let mut detailed = lines.to_vec();
    detailed.push(meter_line("Swap", ratio, width, value));
    Some(detailed)
}

/// The storage lines with each disk's I/O after the filesystems (before
/// the health line).
fn storage_details(snapshot: &Snapshot, lines: &[Line<'static>]) -> Option<Vec<Line<'static>>> {
    if snapshot.disks.is_empty() {
        return None;
    }
    let disk_lines = snapshot.disks.iter().take(MAX_DISKS).map(|disk| {
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
        if let Some(ms) = disk.latency_ms {
            let color = match rigtop_core::view::latency_severity(ms) {
                Some(Severity::Critical) => BAD,
                Some(Severity::Warning) => WARN,
                None => DIM,
            };
            spans.push(colored(format!(" {:>5}", format::latency(ms)), color));
        }
        if let Some(t) = disk.temperature_c {
            spans.push(colored(format!(" {t:.0}°C"), temperature_color(t)));
        }
        Line::from(spans)
    });
    // The health line, if any, stays last.
    let filesystems = snapshot.filesystems.len().min(3);
    let mut detailed: Vec<Line<'static>> = lines[..filesystems.min(lines.len())].to_vec();
    detailed.extend(disk_lines);
    detailed.extend(lines[filesystems.min(lines.len())..].iter().cloned());
    Some(detailed)
}

/// "Label ■■■■■■ value" filling `width`.
fn meter_line(label: &str, ratio: f64, width: usize, value: Span<'static>) -> Line<'static> {
    let meter_width = width.saturating_sub(LABEL_WIDTH + VALUE_WIDTH).max(4);
    let mut spans = vec![dim(format!("{label:<LABEL_WIDTH$}"))];
    spans.extend(meter(meter_width, ratio));
    spans.push(Span::raw(" "));
    spans.push(value);
    Line::from(spans)
}

fn cpu_lines(app: &App, snapshot: &Snapshot, width: usize) -> Vec<Line<'static>> {
    let cpu = &snapshot.cpu;
    let value = colored(
        format!(
            "{:>3.0}% {} cores",
            cpu.total_percent,
            cpu.per_core_percent.len()
        ),
        percent_color(cpu.total_percent as f64),
    );
    let mut info = Vec::new();
    let config = &app.config;
    for temp in cpu
        .temperatures
        .iter()
        .filter(|t| !config.is_hidden(&t.key))
    {
        info.push(dim(format!("{} ", config.temperature_label(temp))));
        info.push(colored(
            format!("{:.0}°C ", temp.celsius),
            temperature_color(temp.celsius),
        ));
    }
    if let Some(total) = snapshot.power.total_w {
        info.push(dim("Σ "));
        info.push(Span::raw(format!("{total:.0}W ")));
    }
    if let Some(battery) = &snapshot.power.battery {
        info.push(dim("bat "));
        info.push(Span::raw(format!("{} ", battery.summary())));
    }
    for ups in &snapshot.ups {
        info.push(dim(format!("{} ", ups.label())));
        info.push(super::ui::ups_span(ups));
        info.push(Span::raw(" "));
    }
    if let Some(load) = cpu.load_average {
        info.push(dim(format!("load {:.2}", load[0])));
    }
    vec![
        meter_line("CPU", cpu.total_percent as f64 / 100.0, width, value),
        Line::from(info),
    ]
}

fn gpu_lines(snapshot: &Snapshot, width: usize, integrated: IntegratedGpu) -> Vec<Line<'static>> {
    rigtop_core::view::card_gpus(snapshot, integrated)
        .map(|gpu| {
            let util = gpu.utilization_percent.unwrap_or(0.0) as f64;
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
            meter_line(
                &format!("GPU{}", gpu.index),
                util / 100.0,
                width,
                colored(value, percent_color(util)),
            )
        })
        .collect()
}

fn memory_lines(snapshot: &Snapshot, width: usize) -> Vec<Line<'static>> {
    let memory = &snapshot.memory;
    let ratio = memory.used_bytes as f64 / memory.total_bytes.max(1) as f64;
    let value = Span::raw(format!(
        "{}/{} {:.0}%",
        format::bytes(memory.used_bytes),
        format::bytes(memory.total_bytes),
        ratio * 100.0
    ));
    vec![meter_line("Mem", ratio, width, value)]
}

fn process_lines(snapshot: &Snapshot, limit: usize) -> Vec<Line<'static>> {
    let mut processes: Vec<_> = snapshot
        .processes
        .iter()
        .filter(|p| !p.kernel_thread)
        .collect();
    processes.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
    let mut lines = vec![Line::from(dim(format!(
        "{:<18} {:>6} {:>6} {:>6}",
        "Program", "Cpu%", "Mem", "VRAM"
    )))];
    for process in processes.iter().take(limit) {
        let cpu = process.cpu_percent as f64;
        let vram: u64 = snapshot
            .gpus
            .iter()
            .flat_map(|g| g.processes.iter())
            .filter(|g| g.pid == process.pid)
            .filter_map(|g| g.memory_bytes)
            .sum();
        lines.push(Line::from(vec![
            Span::raw(format::pad(&process.name, 18)),
            colored(
                format!(" {cpu:>6.1}"),
                if cpu > 0.0 { percent_color(cpu) } else { DIM },
            ),
            Span::raw(format!(" {:>6}", format::bytes(process.memory_bytes))),
            colored(
                format!(
                    " {:>6}",
                    if vram > 0 {
                        format::bytes(vram)
                    } else {
                        String::new()
                    }
                ),
                GPU_TITLE,
            ),
        ]));
    }
    // Blank lines up to the limit, so the sections below stay in place.
    lines.resize(limit + 1, Line::default());
    lines
}

fn network_lines(app: &App, snapshot: &Snapshot) -> Vec<Line<'static>> {
    let Some(net) = app
        .history
        .net_interface
        .as_deref()
        .and_then(|name| snapshot.networks.iter().find(|n| n.name == name))
    else {
        return Vec::new();
    };
    vec![Line::from(vec![
        dim(format!("{:<LABEL_WIDTH$}", "Net")),
        Span::raw(format!("{} ", format::truncate(&net.name, 14))),
        dim(format!("{} ", link_label(net).unwrap_or_default())),
        Span::raw(format!(
            "▼ {}  ▲ {}",
            format::rate(net.rx_bytes_per_sec),
            format::rate(net.tx_bytes_per_sec)
        )),
    ])]
}

fn storage_lines(snapshot: &Snapshot, width: usize) -> Vec<Line<'static>> {
    let used_ratio = |f: &rigtop_core::snapshot::FilesystemUsage| {
        f.total_bytes.saturating_sub(f.available_bytes) as f64 / f.total_bytes.max(1) as f64
    };
    let mut filesystems: Vec<_> = snapshot.filesystems.iter().collect();
    filesystems.sort_by(|a, b| used_ratio(b).total_cmp(&used_ratio(a)));
    let mut lines: Vec<Line<'static>> = filesystems
        .iter()
        .take(3)
        .map(|fs| {
            let ratio = used_ratio(fs);
            let name = fs.mount_points.first().cloned().unwrap_or_default();
            let value = colored(
                format!(
                    "{:.0}% {} free",
                    ratio * 100.0,
                    format::bytes(fs.available_bytes)
                ),
                percent_color(ratio * 100.0),
            );
            meter_line(
                &format::truncate(&name, LABEL_WIDTH - 1),
                ratio,
                width,
                value,
            )
        })
        .collect();
    // One-line health summary: SMART, btrfs, md.
    let mut health = Vec::new();
    if !snapshot.smart.is_empty() {
        let ok = snapshot
            .smart
            .iter()
            .filter(|s| s.error.is_none() && !s.needs_attention())
            .count();
        let color = if ok == snapshot.smart.len() { OK } else { BAD };
        health.push(dim("SMART "));
        health.push(colored(
            format!("{ok}/{} ok  ", snapshot.smart.len()),
            color,
        ));
    }
    for fs in &snapshot.raid.btrfs {
        let profile = fs
            .allocations
            .iter()
            .find(|a| a.kind == "data")
            .map_or("", |a| a.profile.as_str());
        health.push(dim(format!("btrfs {profile} ")));
        health.push(if fs.has_problems() {
            colored("errors  ", BAD)
        } else {
            colored("ok  ", OK)
        });
    }
    for md in &snapshot.raid.md {
        health.push(dim(format!("{} ", md.name)));
        health.push(if md.is_degraded() {
            colored("degraded  ", BAD)
        } else {
            colored("ok  ", OK)
        });
    }
    for pool in &snapshot.raid.zfs {
        health.push(dim(format!("zfs {} ", pool.name)));
        let errors: u64 = pool.troubled_devices().map(|d| d.errors()).sum();
        let data_errors = pool.data_errors.unwrap_or(0);
        if data_errors > 0 {
            health.push(colored(format!("{data_errors} data errors"), BAD));
        } else if errors > 0 && pool.is_online() {
            health.push(colored(format!("{errors} errors"), BAD));
        } else {
            health.push(super::ui::zfs_state(pool));
        }
        health.push(Span::raw("  "));
    }
    if !health.is_empty() {
        lines.push(Line::from(health));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(gpus: u32) -> Snapshot {
        use rigtop_core::snapshot::GpuStats;
        let mut snapshot = Snapshot::default();
        snapshot.memory.total_bytes = 16 << 30;
        snapshot.gpus = (0..gpus)
            .map(|index| GpuStats {
                index,
                name: format!("GPU model {index}"),
                utilization_percent: Some(10.0),
                ..Default::default()
            })
            .collect();
        snapshot
    }

    /// The text of each row, graphs as "<graph>".
    fn texts(rows: &[BoxRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                BoxRow::Text(line) => line.to_string(),
                BoxRow::CpuGraph(_) | BoxRow::NetGraph(_) => "<graph>".to_owned(),
            })
            .collect()
    }

    fn row_of(rows: &[String], prefix: &str) -> usize {
        rows.iter()
            .position(|r| r.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix:?} in {rows:#?}"))
    }

    #[test]
    fn sections_line_up_across_a_row() {
        use std::time::Duration;
        let sections = [
            OverviewSection::Cpu,
            OverviewSection::Gpu,
            OverviewSection::Memory,
        ];
        let mut lost = App::new(Default::default(), Duration::from_secs(1));
        lost.connection_error = Some("timed out".to_owned());
        let fine = App::new(Default::default(), Duration::from_secs(1));
        let (one, three) = (snapshot(1), snapshot(3));
        let cards = [
            Card {
                app: &lost,
                snapshot: &one,
                sections: &sections,
                width: 60,
            },
            Card {
                app: &fine,
                snapshot: &three,
                sections: &sections,
                width: 60,
            },
        ];
        let check = |budget: usize| {
            let rows: Vec<Vec<String>> = layout_row(&cards, budget, 5)
                .iter()
                .map(|r| texts(r))
                .collect();
            for prefix in ["CPU", "Mem"] {
                assert_eq!(
                    row_of(&rows[0], prefix),
                    row_of(&rows[1], prefix),
                    "{prefix} at budget {budget}: {rows:#?}"
                );
            }
            rows
        };
        // Tight: the bare sections, padded to the tallest.
        let rows = check(7);
        assert_eq!(row_of(&rows[0], "CPU"), 1, "under the lost connection");
        // Roomy: details and a CPU graph, still in line.
        let rows = check(30);
        assert!(rows[0].iter().any(|r| r == "<graph>"));
        assert!(rows[1].iter().any(|r| r.contains("GPU model 2")));
        assert!(rows[0].iter().any(|r| r.contains("GPU model 0")));
    }

    #[test]
    fn sections_in_different_orders_are_laid_out_alone() {
        use std::time::Duration;
        let app = App::new(Default::default(), Duration::from_secs(1));
        let shot = snapshot(1);
        let forward = [OverviewSection::Cpu, OverviewSection::Memory];
        let backward = [OverviewSection::Memory, OverviewSection::Cpu];
        let card = |sections| Card {
            app: &app,
            snapshot: &shot,
            sections,
            width: 60,
        };
        let cards = [card(&forward[..]), card(&backward[..])];
        assert!(section_order(&cards).is_none());
        let rows: Vec<Vec<String>> = layout_row(&cards, 3, 5).iter().map(|r| texts(r)).collect();
        assert_eq!(row_of(&rows[0], "CPU"), 0);
        assert_eq!(row_of(&rows[1], "Mem"), 0);
    }

    #[test]
    fn spare_rows_go_to_graphs_three_to_two() {
        assert_eq!(graph_rows(10, true, true), (6, 4));
        assert_eq!(graph_rows(10, true, false), (10, 0));
        // Too little for a graph: none rather than a stub.
        assert_eq!(graph_rows(3, true, true), (0, 2));
        assert_eq!(graph_rows(1, true, false), (0, 0));
        // Capped, the rest stays empty.
        assert_eq!(
            graph_rows(100, true, true),
            (MAX_CPU_GRAPH_ROWS as u16, MAX_NET_GRAPH_ROWS as u16)
        );
    }
}
