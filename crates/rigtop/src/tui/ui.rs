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
use rigtop_core::config::{IntegratedGpu, OverviewSection};
use rigtop_core::snapshot::{
    BtrfsFilesystem, CpuStats, DiskIo, GpuStats, MemoryStats, NetworkStats, SmartDevice, ZfsPool,
    device_matches_disk,
};

use super::app::{App, ProcTableHit};
use super::graph::{self, BrailleGraph};
use rigtop_core::format;
use rigtop_core::view::{
    GpuRow, GraphSpan, LHM_HINT, Severity, SortKey, disk_rate_text, is_virtual_interface,
    signal_name,
};

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
/// Columns a GPU's name keeps when its readings need the room.
const GPU_NAME_MIN_WIDTH: usize = 12;
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

    // Boxes hidden with `v`; the others take their room.
    let hidden = &app.config.tui.hidden_panels;
    let shown = |section: OverviewSection| !hidden.contains(&section);
    let left_shown = [
        OverviewSection::Memory,
        OverviewSection::Gpu,
        OverviewSection::Storage,
        OverviewSection::Network,
    ]
    .into_iter()
    .any(shown);
    let left_width = match (left_shown, shown(OverviewSection::Processes)) {
        (true, true) => (area.width * 45 / 100).max(44).min(area.width),
        (true, false) => area.width,
        (false, _) => 0,
    };
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Fill(1)]).areas(bottom);

    let heights = left_column_heights(snapshot, left, &shown, app.config.gpu.integrated);
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
    let hit = shown(OverviewSection::Processes).then(|| {
        // The per-container summary (`C`) above the table, at most a third.
        let containers = if app.show_containers {
            rigtop_core::view::container_usage(snapshot)
        } else {
            Vec::new()
        };
        let mut table_area = right;
        if !containers.is_empty() {
            let height = (containers.len() as u16 + 3).min(right.height / 3);
            let [top, rest] =
                Layout::vertical([Constraint::Length(height), Constraint::Fill(1)]).areas(right);
            draw_containers(frame, &containers, top);
            table_area = rest;
        }
        draw_processes(frame, app, snapshot, table_area, table_state)
    });

    if app.details
        && let Some(pid) = app.selected_pid
    {
        draw_details(frame, right, app, snapshot, pid);
    }
    if let Some(confirm) = &app.confirm {
        draw_confirm(
            frame,
            area,
            confirm.pid,
            &confirm.name,
            signal_name(confirm.signal),
        );
    }
    hit
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
fn left_column_heights(
    snapshot: &Snapshot,
    area: Rect,
    shown: &dyn Fn(OverviewSection) -> bool,
    integrated: IntegratedGpu,
) -> LeftHeights {
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

    // A hidden box wants no rows. GPUs drawn in full get text rows and a
    // share of the graph rows; compact ones (an iGPU next to a discrete
    // GPU) one line.
    let (gpus, compact_gpus) = if shown(OverviewSection::Gpu) {
        let rows = rigtop_core::view::panel_gpus(snapshot, integrated);
        let compact = rows
            .iter()
            .filter(|(_, _, r)| *r == GpuRow::Compact)
            .count();
        ((rows.len() - compact) as u16, compact as u16)
    } else {
        (0, 0)
    };
    let memory_wanted = if shown(OverviewSection::Memory) {
        memory_rows(&snapshot.memory).len() as u16 + 2
    } else {
        0
    };
    let memory = take(memory_wanted, MIN_BOX_HEIGHT);
    let storage_needed = if shown(OverviewSection::Storage) {
        storage_lines(snapshot, area.width.saturating_sub(2)).len() as u16 + 2
    } else {
        0
    };
    let storage = take(storage_needed, MIN_BOX_HEIGHT);
    let gpu_text = if gpus + compact_gpus == 0 {
        0
    } else {
        gpus * GPU_TEXT_ROWS + compact_gpus + 2
    };
    let mut gpu = take(gpu_text, gpu_text);
    let network_wanted = if shown(OverviewSection::Network) {
        NET_MIN_HEIGHT
    } else {
        0
    };
    let mut network = take(network_wanted, NET_COMPACT_HEIGHT);

    if gpu > 0 && gpus > 0 {
        // Half of the spare height becomes GPU history graph rows.
        let per_gpu = (remaining / 2 / gpus).min(GPU_GRAPH_MAX_ROWS);
        gpu += per_gpu * gpus;
        remaining -= per_gpu * gpus;
    }
    if network > 0 {
        network += remaining;
    } else if gpu > 0 {
        // Without the network box, the GPU graphs take the rest.
        gpu += remaining;
    }
    LeftHeights {
        memory,
        gpu,
        storage,
        network,
    }
}

/// The panel menu (`v`): which boxes of the host view are drawn, with the
/// key that toggles each.
///
/// `widgets` is whether the widget column is shown, `None` without widgets.
pub(super) fn draw_panels_menu(
    frame: &mut Frame,
    area: Rect,
    hidden: &[OverviewSection],
    widgets: Option<bool>,
) {
    let width = 26.min(area.width);
    let rows = PANEL_KEYS.len() as u16 + u16::from(widgets.is_some());
    let height = (rows + 4).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let mut lines: Vec<Line> = PANEL_KEYS
        .iter()
        .map(|(key, section)| {
            let mark = if hidden.contains(section) {
                "[ ]"
            } else {
                "[x]"
            };
            Line::from(vec![
                colored(format!(" {key}  "), Color::White),
                Span::raw(format!("{mark} {}", section.label())),
            ])
        })
        .collect();
    if let Some(shown) = widgets {
        let mark = if shown { "[x]" } else { "[ ]" };
        lines.push(Line::from(vec![
            colored(" w  ", Color::White),
            Span::raw(format!("{mark} Widgets")),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(dim(" v/Esc close")));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(panel("panels", WARN)), rect);
}

/// Where a key listed in the help works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyScope {
    /// The view of one host.
    Host,
    /// The overview of all hosts.
    Overview,
    /// Both.
    Anywhere,
}

/// A key of the help: the key, what it does, and where it works.
type HelpKey = (&'static str, &'static str, KeyScope);

/// What the help (`?`) lists: sections of keys, what they do and where.
/// Keys stay within 10 columns and descriptions within 26, so that two
/// columns of them fit an 80-column terminal.
const HELP: [(&str, &[HelpKey]); 3] = [
    (
        "processes",
        &[
            ("↑↓ PgUp/Dn", "select a process", KeyScope::Host),
            ("Home End", "first / last process", KeyScope::Host),
            ("← →  r", "sort column / reverse", KeyScope::Host),
            ("/ f", "filter (Enter keeps, Esc)", KeyScope::Host),
            ("Enter", "process details", KeyScope::Host),
            ("e", "tree view", KeyScope::Host),
            ("t", "kernel threads", KeyScope::Host),
            ("C", "per-container usage", KeyScope::Host),
            ("x", "notify when it exits", KeyScope::Host),
            ("k K", "terminate / kill (asks)", KeyScope::Host),
            ("p", "pause (asks) / resume", KeyScope::Host),
            ("] [", "priority down / up (nice)", KeyScope::Host),
        ],
    ),
    (
        "view",
        &[
            ("n", "next network interface", KeyScope::Host),
            ("i", "show / hide addresses", KeyScope::Host),
            ("g", "graph time span", KeyScope::Host),
            ("+ -", "slower / faster updates", KeyScope::Host),
            ("v", "show / hide boxes", KeyScope::Anywhere),
            ("w", "widget column", KeyScope::Anywhere),
            ("l", "this machine's card", KeyScope::Overview),
        ],
    ),
    (
        "hosts",
        &[
            ("Tab 0-9", "overview / hosts", KeyScope::Anywhere),
            ("a", "alerts, history (z snooze)", KeyScope::Anywhere),
            ("u", "update this host's agent", KeyScope::Host),
            ("u", "update all older agents", KeyScope::Overview),
            ("U", "update rigtop here", KeyScope::Anywhere),
            ("q", "quit", KeyScope::Anywhere),
            ("? F1", "this help", KeyScope::Anywhere),
        ],
    ),
];

/// What the screen under the help can do, to list only the keys that work.
#[derive(Debug, Clone, Copy)]
pub(super) struct HelpContext {
    /// The overview is shown (else one host's view).
    pub overview: bool,
    /// There are several hosts to switch between.
    pub multi: bool,
    /// There are widgets for a widget column.
    pub widgets: bool,
}

/// The help's lines, one list per section.
fn help_sections(context: HelpContext) -> Vec<Vec<Line<'static>>> {
    HELP.iter()
        .filter_map(|(title, keys)| {
            let lines: Vec<Line<'static>> = keys
                .iter()
                .filter(|(key, _, scope)| {
                    let here = match scope {
                        KeyScope::Host => !context.overview,
                        KeyScope::Overview => context.overview,
                        KeyScope::Anywhere => true,
                    };
                    here && (context.widgets || *key != "w") && (context.multi || *key != "Tab 0-9")
                })
                .map(|(key, what, _)| {
                    Line::from(vec![
                        colored(format!(" {} ", format::pad(key, 10)), Color::White),
                        Span::raw(*what),
                    ])
                })
                .collect();
            (!lines.is_empty()).then(|| {
                let mut section = vec![Line::from(colored(format!(" {title}"), WARN))];
                section.extend(lines);
                section
            })
        })
        .collect()
}

/// The help (`?`): the keys that work on the screen below, in one column,
/// or in two (processes, then the rest) when one would not fit.
pub(super) fn draw_help(frame: &mut Frame, area: Rect, context: HelpContext) {
    let sections = help_sections(context);
    let one: Vec<Line> = sections.join(&Line::from(""));
    // The borders; the closing hint sits on the bottom one.
    let chrome = 2;
    let columns: Vec<Vec<Line>> = if one.len() as u16 + chrome <= area.height || sections.len() < 2
    {
        vec![one]
    } else {
        let mut rest = sections.into_iter();
        let first = rest.next().unwrap_or_default();
        vec![first, rest.collect::<Vec<_>>().join(&Line::from(""))]
    };
    let column_width = columns.iter().flatten().map(Line::width).max().unwrap_or(0) as u16 + 1;
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0) as u16;
    let width = (column_width * columns.len() as u16 + 2).min(area.width);
    let height = (rows + chrome).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = panel("help", WARN).title_bottom(Line::from(dim(" any key closes ")));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let areas =
        Layout::horizontal(vec![Constraint::Length(column_width); columns.len()]).split(inner);
    for (lines, column) in columns.into_iter().zip(areas.iter()) {
        frame.render_widget(Paragraph::new(lines), *column);
    }
}

/// Keys of the panel menu and the boxes they toggle (the CPU box stays).
pub(super) const PANEL_KEYS: [(char, OverviewSection); 5] = [
    ('m', OverviewSection::Memory),
    ('g', OverviewSection::Gpu),
    ('s', OverviewSection::Storage),
    ('n', OverviewSection::Network),
    ('p', OverviewSection::Processes),
];

/// A UPS's summary (`100% 73W`, `on battery 72% 25m`) in the color of its
/// state.
pub(super) fn ups_span(ups: &rigtop_core::snapshot::UpsStats) -> Span<'static> {
    let text = ups.summary();
    match rigtop_core::view::ups_severity(ups) {
        Some(Severity::Critical) => colored(text, BAD),
        Some(Severity::Warning) => colored(text, WARN),
        None => Span::raw(text),
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
                    app.offline_note()
                        .map(|note| colored(format!(" {note} "), BAD)),
                    app.release_note
                        .as_ref()
                        .map(|note| colored(format!(" {note} "), OK)),
                    app.alerts_unread
                        .and_then(super::alert_log::unread_badge)
                        .map(|badge| Span::styled(format!(" {} ", badge.content), badge.style)),
                    rigtop_core::view::version_note(snapshot).map(|note| {
                        // Older agents can be updated from here.
                        let key =
                            if rigtop_core::update::agent_is_older(&snapshot.host.rigtop_version) {
                                " (u: update)"
                            } else {
                                ""
                            };
                        colored(format!(" {note}{key}"), WARN)
                    }),
                    host.reboot_required
                        .as_ref()
                        .map(|_| colored(format!(" {}", rigtop_core::view::REBOOT_NOTE), WARN)),
                    Some(dim(" ")),
                    Some(colored(host.hostname.clone(), Color::White)),
                    rigtop_core::view::link_rtt_text(snapshot).map(|(text, slow)| {
                        let text = format!(" {text}");
                        if slow { colored(text, WARN) } else { dim(text) }
                    }),
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
    // "C" + index + space + [meter] + " 100%" + gap, plus " 58°C" (the
    // core's temperature, when the CPU has per-core sensors) and " 4.2G"
    // when wide. The temperature comes first when space is short.
    let base = 1 + digits + 1 + 5 + 1;
    let show_temp = layout.meters
        && cell_width >= base + 5 + 3
        && cpu.per_core_celsius.iter().any(Option::is_some);
    let temp_width = if show_temp { 5 } else { 0 };
    let show_freq = layout.meters
        && cell_width >= CORE_FREQ_MIN_WIDTH + temp_width
        && !cpu.per_core_mhz.is_empty();
    let fixed = base + temp_width + if show_freq { 5 } else { 0 };
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
            if show_temp {
                spans.push(match cpu.per_core_celsius.get(index).copied().flatten() {
                    Some(celsius) => {
                        colored(format!("{celsius:>3.0}°C"), temperature_color(celsius))
                    }
                    None => Span::raw(" ".repeat(temp_width)),
                });
            }
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
    // Only resources tasks waited for, to keep the line short.
    let stalls = snapshot
        .pressure
        .as_ref()
        .map(|p| rigtop_core::view::pressure_items(p, &config.alerts))
        .unwrap_or_default();
    if !stalls.is_empty() {
        let gap = if first.is_empty() { "" } else { "  " };
        first.push(dim(format!("{gap}{}", rigtop_core::view::PRESSURE_LABEL)));
        for item in stalls {
            first.push(dim(format!(" {} ", item.name)));
            first.push(if item.warn {
                colored(item.value, WARN)
            } else {
                Span::raw(item.value)
            });
        }
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
    let mut second = Vec::new();
    // Average / fastest core; the core grid shows each one when it has room.
    if let Some(clock) = rigtop_core::view::cpu_clock_text(cpu) {
        second.push(dim("Clock "));
        second.push(Span::raw(clock));
        second.push(Span::raw("  "));
    }
    second.extend([
        dim(format!("Power {cpu_label} ")),
        Span::raw(watts(cpu_w)),
        dim(" GPU "),
        Span::raw(watts(power.gpus_w)),
        dim(" Σ "),
        colored(watts(power.total_w), Color::White),
    ]);
    if let Some(battery) = &power.battery {
        second.push(dim("   Bat "));
        second.push(Span::raw(battery.summary()));
        if let Some(note) = battery.health_note() {
            second.push(colored(format!(" {note}"), WARN));
        }
    }
    for ups in &snapshot.ups {
        second.push(dim(format!("   {} ", ups.label())));
        second.push(ups_span(ups));
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
    // ZFS's read cache, which the kernel counts as used.
    if let Some(arc) = memory.zfs_arc_bytes.filter(|b| *b > 0) {
        rows.push(("ARC", arc, arc as f64 / total));
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

    // The layout decides how many history graph rows each GPU drawn in
    // full gets (possibly none); compact ones take one line.
    let gpus = rigtop_core::view::panel_gpus(snapshot, app.config.gpu.integrated);
    let compact = gpus
        .iter()
        .filter(|(_, _, r)| *r == GpuRow::Compact)
        .count() as u16;
    let full = (gpus.len() as u16 - compact).max(1);
    let rows_per_gpu = (inner.height.saturating_sub(compact) / full).max(GPU_TEXT_ROWS);
    let graph_rows = rows_per_gpu - GPU_TEXT_ROWS;
    let bottom = inner.y + inner.height;
    let mut y = inner.y;
    for (i, gpu, row) in gpus {
        let rows = Rect {
            y,
            height: 1,
            ..inner
        };
        if row == GpuRow::Compact {
            if y >= bottom {
                break;
            }
            frame.render_widget(Paragraph::new(compact_gpu_line(gpu, inner.width)), rows);
            y += 1;
            continue;
        }
        if y + rows_per_gpu > bottom {
            break;
        }
        let util = gpu.utilization_percent.unwrap_or(0.0) as f64;
        let name = gpu.name.trim_start_matches("NVIDIA ").to_owned();
        let index = colored(format!("{} ", gpu.index), GPU_TITLE);
        let mut details = Vec::new();
        if let Some(t) = gpu.temperature_c {
            details.push(colored(format!("{t:.0}°C "), temperature_color(t)));
        }
        if let Some(w) = gpu.power_w {
            let limit = gpu
                .power_limit_w
                .map_or(String::new(), |l| format!("/{l:.0}"));
            details.push(dim(format!("{w:.0}{limit}W ")));
        }
        if let Some(fan) = gpu.fan_percent {
            details.push(dim(format!("fan {fan:.0}% ")));
        }
        for (note, severity) in rigtop_core::view::gpu_notes(gpu) {
            details.push(match severity {
                Some(Severity::Critical) => colored(format!("{note} "), BAD),
                Some(Severity::Warning) => colored(format!("{note} "), WARN),
                None => dim(format!("{note} ")),
            });
        }
        // The name takes the room the readings leave (at least a few
        // columns), so that they all show in a narrow box and a wide one
        // shows the whole name.
        let taken = index.width() + 1 + details.iter().map(Span::width).sum::<usize>();
        let name_width = (inner.width as usize)
            .saturating_sub(taken)
            .max(GPU_NAME_MIN_WIDTH);
        let mut line1 = vec![
            index,
            Span::raw(format::truncate(&name, name_width)),
            Span::raw(" "),
        ];
        line1.extend(details);

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
        frame.render_widget(Paragraph::new(Line::from(line1)), rows);
        frame.render_widget(Paragraph::new(Line::from(line2)), Rect { y: y + 1, ..rows });
        y += rows_per_gpu;
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
                y: rows.y + GPU_TEXT_ROWS,
                height: graph_rows,
                ..rows
            },
        );
    }
}

/// One line for an integrated GPU next to a discrete one:
/// "2 AMD Granite Ridge [Ra… 42°C util 0% vram 368M/2.0G", in `width`
/// columns: the name takes the room left, and VRAM is left out before the
/// name gets shorter than a few columns.
fn compact_gpu_line(gpu: &GpuStats, width: u16) -> Line<'static> {
    let util = gpu.utilization_percent.unwrap_or(0.0) as f64;
    let index = colored(format!("{} ", gpu.index), GPU_TITLE);
    let mut details = Vec::new();
    if let Some(t) = gpu.temperature_c {
        details.push(colored(format!("{t:.0}°C "), temperature_color(t)));
    }
    details.push(dim("util "));
    details.push(colored(
        format!("{} ", format::percent(gpu.utilization_percent)),
        percent_color(util),
    ));
    let vram = match (gpu.memory_used_bytes, gpu.memory_total_bytes) {
        (Some(used), Some(total)) => vec![
            dim("vram "),
            Span::raw(format!("{}/{}", format::bytes(used), format::bytes(total))),
        ],
        _ => Vec::new(),
    };
    let room = (width as usize)
        .saturating_sub(index.width() + 1 + details.iter().map(Span::width).sum::<usize>());
    let vram_width: usize = vram.iter().map(Span::width).sum();
    let name_min = GPU_NAME_MIN_WIDTH.min(Span::raw(gpu.name.as_str()).width());
    let show_vram = vram_width + name_min <= room;
    let name_width = room
        .saturating_sub(if show_vram { vram_width } else { 0 })
        .max(name_min);
    let mut spans = vec![
        index,
        dim(format!("{} ", format::truncate(&gpu.name, name_width))),
    ];
    spans.extend(details);
    if show_vram {
        spans.extend(vram);
    }
    Line::from(spans)
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

    // Device names line up at the longest one.
    let unlisted: Vec<&SmartDevice> = snapshot
        .smart
        .iter()
        .filter(|s| {
            !snapshot
                .disks
                .iter()
                .any(|d| device_matches_disk(&s.device, &d.name))
        })
        .collect();
    let name_width = snapshot
        .disks
        .iter()
        .map(|d| d.name.as_str())
        .chain(unlisted.iter().map(|s| s.device.as_str()))
        .map(|name| Span::raw(name).width())
        .max()
        .unwrap_or(0)
        .clamp(3, 12);

    // Block devices with I/O, temperature and SMART. A drive whose SMART
    // state does not fit on its row, even without the optional parts, gets
    // a line of its own below it when something is wrong; healthy ones
    // share one summary line, so that the box keeps its height.
    // The request time shows for every drive or for none, so that the
    // columns line up: none when it would push a temperature off its row.
    let latency = snapshot.disks.iter().all(|disk| {
        let groups = disk_groups(disk, name_width, true);
        groups.iter().map(Group::width).sum::<usize>() <= width as usize
    });
    let mut healthy = Vec::new();
    for disk in &snapshot.disks {
        let mut groups = disk_groups(disk, name_width, latency);
        let Some(smart) = snapshot
            .smart
            .iter()
            .find(|s| device_matches_disk(&s.device, &disk.name))
        else {
            lines.extend(fit_groups(groups, width));
            continue;
        };
        let state = smart_groups(smart, now_secs);
        if required_width(&groups) + required_width(&state) <= width as usize {
            groups.extend(state);
            lines.extend(fit_groups(groups, width));
        } else if smart.needs_attention() || smart.error.is_some() {
            lines.extend(fit_groups(groups, width));
            lines.extend(fit_groups(continued(state), width));
        } else {
            lines.extend(fit_groups(groups, width));
            healthy.push(smart);
        }
    }
    if !healthy.is_empty() {
        lines.extend(fit_groups(smart_summary(&healthy, now_secs), width));
    }
    // SMART results not tied to a listed block device (e.g. RAID controller pass-through).
    for smart in unlisted {
        let mut groups = vec![Group::new(vec![Span::raw(format::pad(
            &smart.device,
            name_width,
        ))])];
        // No disk row shows this drive's temperature, so show it here.
        if let Some(t) = smart.temperature_c {
            groups.push(Group::new(vec![colored(
                format!(" {t:.0}°C"),
                temperature_color(t),
            )]));
        }
        groups.extend(smart_groups(smart, now_secs));
        lines.extend(fit_groups(groups, width));
    }

    // btrfs: profiles, usage and error counters.
    for fs in &snapshot.raid.btrfs {
        let mount = fs
            .mount_points
            .first()
            .cloned()
            .unwrap_or_else(|| fs.fsid.chars().take(8).collect());
        let mut groups = vec![Group::new(vec![
            colored("btrfs ", STORAGE_TITLE),
            Span::raw(mount),
        ])];
        for alloc in &fs.allocations {
            if alloc.kind == "system" {
                continue;
            }
            let kind = if alloc.kind == "metadata" {
                "meta"
            } else {
                "data"
            };
            let mut spans = vec![dim(format!(" {kind} ")), Span::raw(alloc.profile.clone())];
            if alloc.kind == "data" {
                spans.push(dim(format!(
                    " {}/{}",
                    format::bytes(alloc.used_bytes),
                    format::bytes(alloc.total_bytes)
                )));
            }
            groups.push(Group::new(spans));
        }
        groups.push(Group::optional(vec![dim(format!(
            " {} devs",
            fs.devices.len()
        ))]));
        lines.extend(fit_groups(groups, width));
        lines.extend(fit_groups(btrfs_health_groups(fs, now_secs), width));
    }

    // mdadm arrays.
    for md in &snapshot.raid.md {
        let mut head = vec![
            colored(md.name.clone(), STORAGE_TITLE),
            Span::raw(format!(" {}", md.level)),
        ];
        if let (Some(expected), Some(working)) = (md.expected_devices, md.working_devices) {
            head.push(dim(format!(" [{working}/{expected}]")));
        }
        head.push(if !md.active {
            colored(" inactive", WARN)
        } else if md.is_degraded() {
            colored(" DEGRADED", BAD)
        } else {
            colored(" ok", OK)
        });
        let mut groups = vec![Group::new(head)];
        if let Some(sync) = &md.sync {
            groups.push(Group::new(vec![colored(
                format!(" {} {:.1}%", sync.action, sync.percent),
                WARN,
            )]));
            if let Some(minutes) = sync.finish_minutes {
                groups.push(Group::optional(vec![dim(format!(" eta {minutes:.0}m"))]));
            }
        }
        lines.extend(fit_groups(groups, width));
    }

    // ZFS pools.
    for pool in &snapshot.raid.zfs {
        let mut head = vec![
            colored(format!("{} ", pool.name), STORAGE_TITLE),
            Span::raw("zfs "),
        ];
        if let Some(layout) = pool.layout() {
            head.push(Span::raw(format!("{layout} ")));
        }
        head.push(zfs_state(pool));
        let mut groups = vec![Group::new(head)];
        if let (Some(size), Some(allocated)) = (pool.size_bytes, pool.allocated_bytes) {
            groups.push(Group::new(vec![dim(format!(
                " {}/{}",
                format::bytes(allocated),
                format::bytes(size)
            ))]));
        }
        lines.extend(fit_groups(groups, width));
        if let Some(health) = zfs_health_groups(pool) {
            lines.extend(fit_groups(health, width));
        }
    }
    lines
}

/// A disk's name, I/O, busy share and (with `latency`) request time, and
/// temperature.
fn disk_groups(disk: &DiskIo, name_width: usize, latency: bool) -> Vec<Group> {
    let mut groups = vec![Group::new(vec![
        Span::raw(format::pad(&disk.name, name_width)),
        dim(" R "),
        Span::raw(format!("{:>7}", format::rate(disk.read_bytes_per_sec))),
        dim(" W "),
        Span::raw(format!("{:>7}", format::rate(disk.write_bytes_per_sec))),
    ])];
    if let Some(busy) = disk.busy_percent {
        groups.push(Group::new(vec![colored(
            format!(" {busy:>3.0}%"),
            percent_color(busy as f64),
        )]));
        // Request time where busy time is known (Linux); blank while idle,
        // so the columns stay aligned.
        if latency {
            let text = format!(
                " {:>5}",
                disk.latency_ms.map(format::latency).unwrap_or_default()
            );
            groups.push(Group::new(vec![match disk
                .latency_ms
                .and_then(rigtop_core::view::latency_severity)
            {
                Some(Severity::Critical) => colored(text, BAD),
                Some(Severity::Warning) => colored(text, WARN),
                None => dim(text),
            }]));
        }
    }
    if let Some(t) = disk.temperature_c {
        groups.push(Group::new(vec![colored(
            format!(" {t:.0}°C"),
            temperature_color(t),
        )]));
    }
    groups
}

/// "  SMART ok · 3 drives · wear 3-4% · 9m ago": healthy drives in one
/// line, for a box too narrow to give each its SMART state.
fn smart_summary(healthy: &[&SmartDevice], now_secs: u64) -> Vec<Group> {
    let mut groups = vec![Group::new(vec![
        Span::raw(CONTINUATION),
        dim("SMART "),
        colored("ok", OK),
    ])];
    if healthy.len() > 1 {
        groups.push(Group::optional(vec![dim(format!(
            " · {} drives",
            healthy.len()
        ))]));
    }
    let wear: Vec<u32> = healthy.iter().filter_map(|s| s.percentage_used).collect();
    if let (Some(low), Some(high)) = (wear.iter().min(), wear.iter().max()) {
        let text = if low == high {
            format!(" · wear {high}%")
        } else {
            format!(" · wear {low}-{high}%")
        };
        groups.push(Group::optional(vec![dim(text)]));
    }
    // The oldest result; kept, in yellow, once stale.
    if let Some(age) = healthy
        .iter()
        .filter_map(|s| s.collected_at_ms)
        .map(|ms| now_secs.saturating_sub(ms / 1000))
        .max()
    {
        let stale = age > SMART_STALE_SECS;
        let spans = vec![colored(
            format!(" · {}", format::age(age)),
            if stale { WARN } else { DIM },
        )];
        groups.push(if stale {
            Group::new(spans)
        } else {
            Group::optional(spans)
        });
    }
    groups
}

/// Indent of the lines a storage line continues on.
const CONTINUATION: &str = "  ";

/// `groups` on a line that continues the one above: indented, without
/// their own leading spaces.
fn continued(mut groups: Vec<Group>) -> Vec<Group> {
    if let Some(first) = groups.first_mut().and_then(|g| g.spans.first_mut()) {
        first.content = first.content.trim_start().to_owned().into();
    }
    if let Some(group) = groups.first_mut() {
        group.spans.insert(0, Span::raw(CONTINUATION));
    }
    groups
}

/// Spans kept together on one line of the storage box.
struct Group {
    spans: Vec<Span<'static>>,
    /// May be left out when the line is short of room.
    optional: bool,
}

impl Group {
    fn new(spans: Vec<Span<'static>>) -> Self {
        Self {
            spans,
            optional: false,
        }
    }

    fn optional(spans: Vec<Span<'static>>) -> Self {
        Self {
            spans,
            optional: true,
        }
    }

    fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }
}

/// Columns the groups need without the optional ones.
fn required_width(groups: &[Group]) -> usize {
    groups
        .iter()
        .filter(|g| !g.optional)
        .map(Group::width)
        .sum()
}

/// Lays out groups on lines `width` columns wide. Optional groups are left
/// out first, the last one first, until the rest fits on one line; what
/// still does not fit continues on the next line, indented and without its
/// leading spaces. A group wider than a line is clipped.
fn fit_groups(mut groups: Vec<Group>, width: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    let mut total: usize = groups.iter().map(Group::width).sum();
    while total > width {
        let Some(index) = groups.iter().rposition(|g| g.optional) else {
            break;
        };
        total -= groups.remove(index).width();
    }
    let mut lines = Vec::new();
    let mut line: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for mut group in groups.into_iter().filter(|g| !g.spans.is_empty()) {
        let group_width = group.width();
        if used > 0 && used + group_width > width {
            lines.push(Line::from(std::mem::take(&mut line)));
            let first = &mut group.spans[0];
            first.content = first.content.trim_start().to_owned().into();
            line.push(Span::raw(CONTINUATION));
            used = CONTINUATION.len() + group.width();
        } else {
            used += group_width;
        }
        line.extend(group.spans);
    }
    if !line.is_empty() {
        lines.push(Line::from(line));
    }
    lines
}

/// A ZFS pool's devices with errors or in a bad state, its data errors and
/// its last scrub, as groups for [`fit_groups`]; `None` without
/// `zpool status` details.
fn zfs_health_groups(pool: &ZfsPool) -> Option<Vec<Group>> {
    if pool.devices.is_empty() && pool.scan.is_none() {
        return None;
    }
    let mut first = vec![Span::raw("  ")];
    let troubled: Vec<String> = pool
        .troubled_devices()
        .map(|d| {
            if d.is_unhealthy() {
                format!("{} {}", d.name, d.state)
            } else {
                format!("{}: {}", d.name, d.errors())
            }
        })
        .collect();
    let data_errors = pool.data_errors.unwrap_or(0);
    if data_errors > 0 {
        first.push(colored(format!("{data_errors} data errors "), BAD));
    }
    if troubled.is_empty() {
        if data_errors == 0 {
            first.push(colored("errors 0", OK));
        }
    } else {
        first.push(colored(troubled.join(", "), BAD));
    }
    let mut groups = vec![Group::new(first)];
    if let Some(scan) = &pool.scan {
        let (text, severity) = rigtop_core::view::zfs_scan_summary(scan);
        groups.push(Group::new(vec![
            dim(" · "),
            match severity {
                Some(Severity::Critical) => colored(text, BAD),
                Some(Severity::Warning) => colored(text, WARN),
                None => dim(text),
            },
        ]));
    }
    Some(groups)
}

/// A ZFS pool's health: "ok", "errors" when ONLINE with errors, or its
/// state in the color of its severity.
pub(super) fn zfs_state(pool: &ZfsPool) -> Span<'static> {
    if pool.is_online() && pool.has_errors() {
        colored("errors", BAD)
    } else if pool.is_online() {
        colored("ok", OK)
    } else if pool.is_failing() {
        colored(pool.state.clone(), BAD)
    } else {
        colored(pool.state.clone(), WARN)
    }
}

/// Second btrfs line: live error counters, then the last scrub, e.g.
/// "  errors 0 (live) · scrub 3d ago ok, 1h12m", as groups for
/// [`fit_groups`].
fn btrfs_health_groups(fs: &BtrfsFilesystem, now_secs: u64) -> Vec<Group> {
    let mut first = vec![Span::raw("  ")];
    let missing = fs.device_stats.iter().filter(|d| d.missing).count();
    if missing > 0 {
        first.push(colored(format!("{missing} device(s) MISSING "), BAD));
    }
    let errors: u64 = fs.device_stats.iter().map(|d| d.total_errors()).sum();
    if errors > 0 {
        let detail: Vec<String> = fs
            .device_stats
            .iter()
            .filter(|d| d.total_errors() > 0)
            .map(|d| format!("{}: {}", d.label(), d.total_errors()))
            .collect();
        first.push(colored(
            format!("errors {errors} ({})", detail.join(", ")),
            BAD,
        ));
    } else {
        first.push(colored("errors 0", OK));
    }
    // The counters are read every poll but only grow when data is read.
    first.push(dim(" (live)"));
    let mut scrub_group = vec![dim(" · ")];
    match &fs.scrub {
        None => scrub_group.push(colored("scrub: no record", WARN)),
        Some(scrub) => {
            let age_secs = now_secs.saturating_sub(scrub.started_at_ms / 1000);
            let age_color = if age_secs > SCRUB_STALE_SECS {
                WARN
            } else {
                DIM
            };
            scrub_group.push(colored(
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
            scrub_group.push(colored(text, color));
            scrub_group.push(dim(format!(", {}", format::duration(scrub.duration_secs))));
        }
    }
    vec![Group::new(first), Group::new(scrub_group)]
}

/// "SMART ok wear 3% (8m ago)" / "SMART FAIL" / "SMART error: ...", for
/// one drive, as groups for [`fit_groups`] (wear and a recent result's age
/// may be left out).
fn smart_groups(smart: &SmartDevice, now_secs: u64) -> Vec<Group> {
    if let Some(error) = &smart.error {
        return vec![Group::new(vec![
            dim(" SMART "),
            colored(format!("error: {}", format::truncate(error, 30)), BAD),
        ])];
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
    let mut groups = vec![Group::new(vec![dim(" SMART "), status])];
    if let Some(used) = smart.percentage_used {
        groups.push(Group::optional(vec![dim(format!(" wear {used}%"))]));
    }
    for (name, value) in [
        ("realloc", smart.reallocated_sectors),
        ("pending", smart.pending_sectors),
        ("media err", smart.media_errors),
    ] {
        if let Some(n) = value.filter(|n| *n > 0) {
            groups.push(Group::new(vec![colored(format!(" {name} {n}"), BAD)]));
        }
    }
    // Results come from a timer and may lag (sleeping disks keep the old one).
    if let Some(ms) = smart.collected_at_ms {
        let age_secs = now_secs.saturating_sub(ms / 1000);
        let stale = age_secs > SMART_STALE_SECS;
        let spans = vec![colored(
            format!(" ({})", format::age(age_secs)),
            if stale { WARN } else { DIM },
        )];
        groups.push(if stale {
            Group::new(spans)
        } else {
            Group::optional(spans)
        });
    }
    groups
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
    // The shown interface's addresses (and the public one) above the list.
    let addresses = app
        .show_addresses
        .then(|| rigtop_core::view::address_text(selected, snapshot.public_ip.as_deref()))
        .flatten();
    let address_rows = u16::from(addresses.is_some());
    let list_rows =
        (listed.len() as u16 + address_rows).min(inner.height.saturating_sub(NET_GRAPH_MIN_ROWS));
    let [graphs, list_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(list_rows)]).areas(inner);
    draw_network_graphs(frame, app, selected, graphs);

    let mut lines: Vec<Line> = addresses
        .map(|text| {
            Line::from(dim(format!(
                " {}",
                format::truncate(&text, (inner.width as usize).saturating_sub(1))
            )))
        })
        .into_iter()
        .collect();
    // Columns as wide as their longest entry; in a narrow box the link
    // ("Wi-Fi 865/216M -69dBm") is shortened so that the rates still show.
    let links: Vec<String> = listed
        .iter()
        .map(|n| link_label(n).unwrap_or_default())
        .collect();
    let name_width = listed
        .iter()
        .map(|n| Span::raw(n.name.as_str()).width())
        .max()
        .unwrap_or(0)
        .clamp(4, 12)
        + 1;
    let rates_width = 19;
    let link_width = links
        .iter()
        .map(|l| Span::raw(l.as_str()).width())
        .max()
        .filter(|&w| w > 0)
        .map_or(0, |w| w + 1)
        .min((inner.width as usize).saturating_sub(1 + name_width + rates_width));
    lines.extend(
        listed
            .iter()
            .zip(&links)
            .take(list_rows as usize)
            .map(|(n, link)| {
                let marker = if Some(&n.name) == app.history.net_interface.as_ref() {
                    "▸"
                } else {
                    " "
                };
                Line::from(vec![
                    colored(marker, NET_TITLE),
                    Span::raw(format::pad(&n.name, name_width)),
                    dim(format::pad(
                        &format::truncate(link, link_width.saturating_sub(1)),
                        link_width,
                    )),
                    Span::raw(format!(
                        "▼{:>8} ▲{:>8}",
                        format::rate(n.rx_bytes_per_sec),
                        format::rate(n.tx_bytes_per_sec)
                    )),
                ])
            }),
    );
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

/// What each container uses (`C`), the busiest first.
fn draw_containers(
    frame: &mut Frame,
    containers: &[rigtop_core::view::ContainerUsage],
    area: Rect,
) {
    let gpus = containers
        .iter()
        .any(|c| c.gpu_percent.is_some() || c.vram_bytes.is_some());
    let block = panel("containers", NET_TITLE)
        .title_top(Line::from(dim(" / and a name lists its processes ")).right_aligned());
    let mut header = vec!["Container", "Procs", "Cpu%", "Mem"];
    let mut widths = vec![
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Length(6),
        Constraint::Length(7),
    ];
    if gpus {
        header.extend(["Gpu%", "VRAM"]);
        widths.extend([Constraint::Length(5), Constraint::Length(7)]);
    }
    let rows = containers.iter().map(|c| {
        let mut cells = vec![
            Cell::from(Span::styled(c.name.clone(), Style::default().fg(NET_TITLE))),
            Cell::from(c.processes.to_string()),
            Cell::from(colored(
                format!("{:.1}", c.cpu_percent),
                percent_color(c.cpu_percent as f64),
            )),
            Cell::from(format::bytes(c.memory_bytes)),
        ];
        if gpus {
            cells.push(Cell::from(format::percent(c.gpu_percent)));
            cells.push(Cell::from(
                c.vram_bytes.map(format::bytes).unwrap_or_default(),
            ));
        }
        Row::new(cells)
    });
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::default().add_modifier(Modifier::BOLD)))
        .block(block);
    frame.render_widget(table, area);
}

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
            " ? help  ↑↓ select  Enter details  x notify on exit  p pause  [] priority  C containers  ←→ sort  r reverse  / filter  e tree  t kthreads  v panels  U update  k term  K kill  +- interval  g graph span  a alerts  q quit ",
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
    let has_containers = snapshot.processes.iter().any(|p| p.container.is_some());
    let columns = process_columns(
        area.width.saturating_sub(2),
        Optional {
            gpus: has_gpus,
            threads: has_threads,
            containers: has_containers,
        },
        app.tree,
    );
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
                ProcColumn::Program => Cell::from(Line::from(vec![
                    dim(row.tree_prefix.clone()),
                    // Watched for its exit (`x`).
                    if app.exit_watch.contains(p.pid) {
                        colored(p.name.clone(), WARN)
                    } else {
                        Span::raw(p.name.clone())
                    },
                    // Paused (`p`).
                    if rigtop_core::view::is_stopped(p) {
                        colored(" stopped", WARN)
                    } else {
                        Span::raw("")
                    },
                ])),
                ProcColumn::Command => {
                    Cell::from(Span::styled(p.command.clone(), Style::default().fg(DIM)))
                }
                ProcColumn::Threads => {
                    Cell::from(p.threads.map_or(String::new(), |t| t.to_string()))
                }
                ProcColumn::User => Cell::from(p.user.clone()),
                ProcColumn::Container => Cell::from(Span::styled(
                    p.container.clone().unwrap_or_default(),
                    Style::default().fg(NET_TITLE),
                )),
                ProcColumn::Memory => Cell::from(format::bytes(p.memory_bytes)),
                ProcColumn::Cpu => Cell::from(Span::styled(
                    format!("{cpu:.1}"),
                    Style::default().fg(if cpu > 0.0 { percent_color(cpu) } else { DIM }),
                )),
                ProcColumn::Disk => Cell::from(disk_rate_text(p)),
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

    let widths: Vec<Constraint> = columns.iter().map(|c| c.constraint(app.tree)).collect();
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
    Container,
    Memory,
    Cpu,
    Disk,
    Gpu,
    Vram,
}

impl ProcColumn {
    /// Display order.
    const ORDER: [ProcColumn; 11] = [
        ProcColumn::Pid,
        ProcColumn::Program,
        ProcColumn::Command,
        ProcColumn::Threads,
        ProcColumn::User,
        ProcColumn::Container,
        ProcColumn::Memory,
        ProcColumn::Cpu,
        ProcColumn::Disk,
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
            ProcColumn::Container => "Container",
            ProcColumn::Memory => "Mem",
            ProcColumn::Cpu => "Cpu%",
            ProcColumn::Disk => "Disk",
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
            ProcColumn::Container => Some(SortKey::Container),
            ProcColumn::Memory => Some(SortKey::Memory),
            ProcColumn::Cpu => Some(SortKey::Cpu),
            ProcColumn::Disk => Some(SortKey::Disk),
            ProcColumn::Gpu => Some(SortKey::Gpu),
            ProcColumn::Vram => Some(SortKey::Vram),
        }
    }

    /// Width used to decide whether the column fits (Command: its minimum).
    /// The tree view widens Program for the lines before the names.
    fn width(self, tree: bool) -> u16 {
        match self {
            ProcColumn::Pid => 7,
            ProcColumn::Program if tree => 25,
            ProcColumn::Program => 15,
            ProcColumn::Command => 12,
            ProcColumn::Threads => 4,
            ProcColumn::User => 9,
            ProcColumn::Container => 14,
            ProcColumn::Memory => 6,
            ProcColumn::Cpu => 6,
            // "1023M/s"
            ProcColumn::Disk => 7,
            ProcColumn::Gpu => 5,
            ProcColumn::Vram => 6,
        }
    }

    fn constraint(self, tree: bool) -> Constraint {
        match self {
            // The command line takes all remaining width.
            ProcColumn::Command => Constraint::Fill(1),
            other => Constraint::Length(other.width(tree)),
        }
    }
}

/// Which optional columns have data on a host.
#[derive(Debug, Clone, Copy)]
struct Optional {
    gpus: bool,
    threads: bool,
    containers: bool,
}

/// Columns that fit in `width`: Pid, Program, Mem and Cpu% always, then
/// User, Container, Gpu%, VRAM, Disk, Thr and Command as space allows (GPU
/// columns only on hosts with GPUs, Thr only where thread counts exist,
/// Container only where processes run in containers). Returned in display
/// order.
fn process_columns(width: u16, has: Optional, tree: bool) -> Vec<ProcColumn> {
    let mut chosen = vec![
        ProcColumn::Pid,
        ProcColumn::Program,
        ProcColumn::Memory,
        ProcColumn::Cpu,
    ];
    let optional = [
        ProcColumn::User,
        ProcColumn::Container,
        ProcColumn::Gpu,
        ProcColumn::Vram,
        ProcColumn::Disk,
        ProcColumn::Threads,
        ProcColumn::Command,
    ];
    // Every column is followed by one space of column spacing.
    let used = |cols: &[ProcColumn]| cols.iter().map(|c| c.width(tree) + 1).sum::<u16>();
    for column in optional {
        let available = match column {
            ProcColumn::Gpu | ProcColumn::Vram => has.gpus,
            ProcColumn::Threads => has.threads,
            ProcColumn::Container => has.containers,
            _ => true,
        };
        if !available {
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

/// The selected process's details over the lower part of `area` (the
/// process box), so the selected row usually stays in sight above.
/// What the details say about a watched process.
fn exit_watch_text(state: rigtop_core::view::WatchState) -> String {
    match state {
        rigtop_core::view::WatchState::Unknown => {
            // Older agents do not report exits; a new watch is confirmed
            // by the next snapshot.
            "notify, once the host confirms (needs rigtop 0.19 or later there)".to_owned()
        }
        _ => "notify (in the alert history, a)".to_owned(),
    }
}

fn draw_details(frame: &mut Frame, area: Rect, app: &App, snapshot: &Snapshot, pid: u32) {
    let rows = rigtop_core::view::process_details(snapshot, pid, std::time::SystemTime::now()).map(
        |mut rows| {
            if app.exit_watch.contains(pid) {
                let state = app.exit_watch.state(snapshot, pid);
                rows.push(("On exit", exit_watch_text(state)));
            }
            rows
        },
    );
    let mut lines: Vec<Line> = match rows {
        Some(rows) => rows
            .into_iter()
            .map(|(label, value)| {
                Line::from(vec![
                    colored(format!("{label:<10}"), PROC_TITLE),
                    Span::raw(value),
                ])
            })
            .collect(),
        None => vec![Line::from(dim(format!("process {pid} has exited")))],
    };
    lines.push(Line::from(dim(
        "Enter/Esc: close   ↑↓: other processes   x: notify on exit",
    )));
    // Rows after wrapping (long command lines), plus the borders.
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let wrapped: usize = lines
        .iter()
        .map(|line| line.width().div_ceil(inner_width).max(1))
        .sum();
    let height = (wrapped as u16 + 2).min(area.height);
    let rect = Rect {
        y: area.bottom() - height,
        height,
        ..area
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("details", PROC_TITLE))
            .wrap(ratatui::widgets::Wrap { trim: false }),
        rect,
    );
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
                zfs: vec![rigtop_core::snapshot::ZfsPool {
                    name: "tank".to_owned(),
                    state: "DEGRADED".to_owned(),
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
            time_left_secs: Some(9000),
            health_percent: Some(77.0),
        });
        app.apply(Event::Snapshot(Box::new(snapshot)));
        let screen = render(&mut app, 160, 50);
        assert!(screen.contains("Power SoC 6W"), "{screen}");
        assert!(screen.contains("Bat 72% -12W 2h30m health 77%"), "{screen}");
    }

    #[test]
    fn shows_the_ups_next_to_the_power() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        let mut snapshot = sample_snapshot();
        snapshot.ups = vec![rigtop_core::snapshot::UpsStats {
            name: "ups".to_owned(),
            status: vec!["OB".to_owned(), "DISCHRG".to_owned()],
            charge_percent: Some(72.0),
            runtime_secs: Some(1500),
            power_w: Some(73.0),
            ..Default::default()
        }];
        app.apply(Event::Snapshot(Box::new(snapshot)));
        let screen = render(&mut app, 160, 50);
        assert!(screen.contains("UPS on battery 72% 25m"), "{screen}");
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
    fn help_lists_the_keys_of_the_screen_below() {
        let text = |context| -> String {
            help_sections(context)
                .concat()
                .iter()
                .map(|line| format!("{line}\n"))
                .collect()
        };
        let host = text(HelpContext {
            overview: false,
            multi: false,
            widgets: false,
        });
        assert!(host.contains("process details"), "{host}");
        assert!(host.contains("update this host's agent"), "{host}");
        // Nothing to switch to, and no widget column.
        assert!(!host.contains("Tab 0-9") && !host.contains("widget column"));
        let overview = text(HelpContext {
            overview: true,
            multi: true,
            widgets: true,
        });
        assert!(!overview.contains("process details"), "{overview}");
        for expected in ["update all older agents", "Tab 0-9", "widget column"] {
            assert!(overview.contains(expected), "{expected}: {overview}");
        }

        // An 80x24 terminal shows the whole help of a host view.
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let context = HelpContext {
            overview: false,
            multi: true,
            widgets: true,
        };
        terminal.draw(|f| draw_help(f, f.area(), context)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let screen: String = (0..24)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        for expected in [
            "priority down / up (nice)",
            "alerts, history (z snooze)",
            "this help",
        ] {
            assert!(screen.contains(expected), "{expected}:\n{screen}");
        }
    }

    /// Three NVMe drives like zefat's, one of them with reallocated sectors.
    fn three_drives(realloc: Option<u64>) -> Snapshot {
        let mut snapshot = sample_snapshot();
        snapshot.raid = Default::default();
        snapshot.disks = (0..3)
            .map(|i| DiskIo {
                name: format!("nvme{i}n1"),
                read_bytes_per_sec: 16e3,
                write_bytes_per_sec: 128e6,
                busy_percent: Some(72.0),
                latency_ms: Some(0.3),
                temperature_c: Some(57.0 + i as f32),
            })
            .collect();
        snapshot.smart = (0..3)
            .map(|i| SmartDevice {
                device: format!("nvme{i}"),
                passed: Some(true),
                percentage_used: Some(3 + i),
                reallocated_sectors: realloc.filter(|_| i == 2),
                ..Default::default()
            })
            .collect();
        snapshot
    }

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn storage_keeps_temperatures_and_smart_in_a_narrow_box() {
        // The left column of a 120-column terminal next to the widgets.
        let lines = storage_lines(&three_drives(None), 42);
        let text = plain(&lines);
        assert!(lines.iter().all(|l| l.width() <= 42), "{text:#?}");
        for t in ["57°C", "58°C", "59°C"] {
            assert!(text.iter().any(|l| l.contains(t)), "{t}: {text:#?}");
        }
        // Healthy drives share one line instead of one each.
        assert_eq!(text.iter().filter(|l| l.contains("SMART")).count(), 1);
        assert!(
            text.iter()
                .any(|l| l.starts_with("  SMART ok · 3 drives · wear 3-5%")),
            "{text:#?}"
        );

        // A drive with a problem gets its own line under its row.
        let lines = storage_lines(&three_drives(Some(8)), 42);
        let text = plain(&lines);
        assert!(lines.iter().all(|l| l.width() <= 42), "{text:#?}");
        let row = text.iter().position(|l| l.starts_with("nvme2n1")).unwrap();
        assert!(
            text[row + 1].starts_with("  SMART warn") && text[row + 1].contains("realloc 8"),
            "{text:#?}"
        );
        assert!(text.iter().any(|l| l.contains("SMART ok · 2 drives")));

        // Room for the request times but not for a problem's details: every
        // row keeps its request time (the columns line up, and a failing
        // drive is often a slow one), the details go below.
        let lines = storage_lines(&three_drives(Some(8)), 52);
        let text = plain(&lines);
        let rows: Vec<&String> = text.iter().filter(|l| l.starts_with("nvme")).collect();
        assert!(rows.iter().all(|l| l.contains("0.3ms")), "{text:#?}");
        let row = text.iter().position(|l| l.starts_with("nvme2n1")).unwrap();
        assert!(text[row + 1].starts_with("  SMART warn"), "{text:#?}");

        // Wide: everything on the drive's row, as before.
        let text = plain(&storage_lines(&three_drives(None), 80));
        assert!(
            text.iter()
                .any(|l| l.starts_with("nvme0n1") && l.contains("0.3ms 57°C SMART ok wear 3%")),
            "{text:#?}"
        );
    }

    #[test]
    fn gpu_readings_fit_a_narrow_box() {
        let mut app = App::new(Config::default(), Duration::from_secs(1));
        let mut snapshot = sample_snapshot();
        snapshot.gpus[0].name = "NVIDIA RTX 4000 Ada Generation".to_owned();
        snapshot.gpus[0].temperature_c = Some(38.0);
        snapshot.gpus[0].power_w = Some(7.0);
        snapshot.gpus[0].power_limit_w = Some(130.0);
        snapshot.gpus[0].fan_percent = Some(30.0);
        app.apply(Event::Snapshot(Box::new(snapshot)));
        // A 44-column left box: the name gives way to the readings.
        let screen = render(&mut app, 80, 40);
        assert!(screen.contains("0 RTX 4000 Ada"), "{screen}");
        assert!(screen.contains("… 38°C 7/130W fan 30% "), "{screen}");
        // A wide one shows the whole name.
        let screen = render(&mut app, 200, 60);
        assert!(screen.contains("RTX 4000 Ada Generation 38°C"), "{screen}");
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
        let tall = left_column_heights(
            &snapshot,
            Rect::new(0, 0, 90, 60),
            &|_| true,
            IntegratedGpu::Compact,
        );
        assert!(
            tall.memory > 0 && tall.gpu > 0 && tall.storage > 0 && tall.network >= NET_MIN_HEIGHT
        );
        assert_eq!(tall.memory + tall.gpu + tall.storage + tall.network, 60);
        // Short: memory and storage win, the rest is hidden rather than squashed.
        let short = left_column_heights(
            &snapshot,
            Rect::new(0, 0, 90, 12),
            &|_| true,
            IntegratedGpu::Compact,
        );
        assert!(short.memory > 0 && short.storage > 0);
        assert_eq!(short.gpu, 0);
        assert!(short.memory + short.gpu + short.storage + short.network <= 12);
    }

    #[test]
    fn process_columns_keep_essentials_when_narrow() {
        let all = Optional {
            gpus: true,
            threads: true,
            containers: true,
        };
        let narrow = process_columns(36, all, false);
        assert_eq!(
            narrow,
            vec![
                ProcColumn::Pid,
                ProcColumn::Program,
                ProcColumn::Memory,
                ProcColumn::Cpu
            ]
        );
        let wide = process_columns(120, all, false);
        assert_eq!(wide, ProcColumn::ORDER.to_vec());
        // No GPU columns on hosts without GPUs.
        let no_gpus = Optional { gpus: false, ..all };
        assert!(!process_columns(120, no_gpus, false).contains(&ProcColumn::Vram));
        // No Thr column where thread counts are unknown (Windows).
        let no_threads = Optional {
            threads: false,
            ..all
        };
        assert!(!process_columns(120, no_threads, false).contains(&ProcColumn::Threads));
        // No Container column without containers.
        let no_containers = Optional {
            containers: false,
            ..all
        };
        assert!(!process_columns(120, no_containers, false).contains(&ProcColumn::Container));
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
