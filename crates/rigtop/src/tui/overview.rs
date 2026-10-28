//! Multi-host views of the TUI: the tab bar and the overview of all hosts.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use rigtop_core::config::OverviewSection;
use rigtop_core::format;
use rigtop_core::snapshot::Snapshot;
use rigtop_core::view::{Severity, alerts_with};

use super::HostTui;
use super::ui::{
    BAD, BORDER, DIM, GPU_TITLE, OK, PROC_TITLE, WARN, colored, dim, link_label, meter,
    percent_color, temperature_color,
};

/// Narrowest host box; more hosts than fit side by side wrap to new rows.
const BOX_MIN_WIDTH: u16 = 52;
/// Data older than this marks a host as stale.
const STALE_AFTER: Duration = Duration::from_secs(10);
/// Alerts listed per host box.
const MAX_ALERTS: usize = 3;
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
                alerts_with(s, &app.config.alerts)
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
    spans.push(dim("Tab/0-9: switch  a: alerts"));
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
    let visible: Vec<&HostTui> = hosts
        .iter()
        .filter(|h| include_local || !h.spec.is_local())
        .collect();
    if visible.is_empty() {
        let text = "no hosts to show: add [[hosts]] to the configuration or use --host";
        frame.render_widget(Paragraph::new(dim(text)), area);
        return;
    }
    let columns = ((area.width / BOX_MIN_WIDTH).max(1) as usize).min(visible.len());
    let rows = visible.len().div_ceil(columns);
    let box_width = area.width / columns as u16;
    let box_height = area.height / rows as u16;
    for (i, host) in visible.iter().enumerate() {
        let (col, row) = ((i % columns) as u16, (i / columns) as u16);
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
        draw_host_box(frame, rect, host, processes);
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

fn draw_host_box(frame: &mut Frame, area: Rect, host: &HostTui, processes: usize) {
    let (color, _) = host_status(host);
    let mut title_right = Vec::new();
    if let Some(snapshot) = &host.app.snapshot {
        if snapshot.host.hostname != host.spec.name {
            title_right.push(dim(format!(" {}", snapshot.host.hostname)));
        }
        if let Some(note) = rigtop_core::view::version_note(snapshot) {
            title_right.push(colored(format!(" {note}"), WARN));
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
    let width = inner.width as usize;
    let budget = inner.height as usize;

    let Some(snapshot) = &host.app.snapshot else {
        let mut lines = vec![Line::from(dim(host.app.waiting_message.clone()))];
        if let Some(note) = host.app.offline_note() {
            lines.push(Line::from(colored(note, BAD)));
        }
        if let Some(error) = &host.app.connection_error {
            lines.push(Line::from(colored(error.clone(), WARN)));
        }
        // Connection errors can be long: wrap them inside the box.
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        return;
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(error) = &host.app.connection_error {
        let note = host
            .app
            .offline_note()
            .unwrap_or_else(|| "disconnected".to_owned());
        lines.push(Line::from(colored(format!("{note}: {error}"), WARN)));
    }
    let alerts = alerts_with(snapshot, &host.app.config.alerts);
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
    // Add whole sections while they fit; the process list may be shortened.
    for &section in &host.spec.sections {
        let left = budget.saturating_sub(lines.len());
        let mut section_lines = match section {
            OverviewSection::Cpu => cpu_lines(host, snapshot, width),
            OverviewSection::Gpu => gpu_lines(snapshot, width),
            OverviewSection::Memory => memory_lines(snapshot, width),
            OverviewSection::Processes => {
                process_lines(snapshot, processes.min(left.saturating_sub(1)))
            }
            OverviewSection::Network => network_lines(host, snapshot),
            OverviewSection::Storage => storage_lines(snapshot, width),
        };
        if section == OverviewSection::Processes && section_lines.len() <= 1 {
            continue;
        }
        if section_lines.len() > left {
            break;
        }
        lines.append(&mut section_lines);
    }
    frame.render_widget(Paragraph::new(lines), inner);
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

fn cpu_lines(host: &HostTui, snapshot: &Snapshot, width: usize) -> Vec<Line<'static>> {
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
    let config = &host.app.config;
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
    if let Some(load) = cpu.load_average {
        info.push(dim(format!("load {:.2}", load[0])));
    }
    vec![
        meter_line("CPU", cpu.total_percent as f64 / 100.0, width, value),
        Line::from(info),
    ]
}

fn gpu_lines(snapshot: &Snapshot, width: usize) -> Vec<Line<'static>> {
    snapshot
        .gpus
        .iter()
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

fn network_lines(host: &HostTui, snapshot: &Snapshot) -> Vec<Line<'static>> {
    let Some(net) = host
        .app
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
        health.push(super::ui::zfs_state(pool));
        health.push(Span::raw("  "));
    }
    if !health.is_empty() {
        lines.push(Line::from(health));
    }
    lines
}
