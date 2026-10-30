//! The widget column on the right of the terminal: a large clock and boxes
//! fed by commands (see [`rigtop_core::widget`]), next to the overview and
//! the host views alike. `w` hides and shows it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use rigtop_core::format;
use rigtop_core::widget::{GaugeStyle, Level, Widget, WidgetRow, WidgetSource};

use super::ui::{BAD, DIM, METER_EMPTY, OK, WARN, colored, dim, meter, panel};

/// Width of the column.
pub const WIDTH: u16 = 42;
/// Narrower terminals keep all their width for the monitor.
pub const MIN_TERMINAL_WIDTH: u16 = 120;
const TITLE: Color = Color::Rgb(225, 140, 185);
const LABEL_WIDTH: usize = 8;
const GAUGE_WIDTH: usize = 10;

/// Stacks the widgets top-down, each as tall as its content; the ones that
/// do not fit are left out.
pub fn draw(frame: &mut Frame, area: Rect, widgets: &[Widget]) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let mut y = area.y;
    for widget in widgets {
        let lines = widget_lines(widget, inner_width);
        let height = (lines.len() as u16 + 2).min(area.bottom().saturating_sub(y));
        if height < 3 {
            break;
        }
        let rect = Rect { y, height, ..area };
        frame.render_widget(
            Paragraph::new(lines).block(panel(&widget.name, TITLE)),
            rect,
        );
        y += height;
    }
}

fn widget_lines(widget: &Widget, width: usize) -> Vec<Line<'static>> {
    match &widget.source {
        WidgetSource::Clock => clock_lines(width),
        WidgetSource::Invalid(problem) => vec![Line::from(colored(problem.clone(), WARN))],
        WidgetSource::Command { .. } | WidgetSource::Fixed(_) => {
            let Some(state) = widget.state() else {
                return Vec::new();
            };
            if state.output.is_none() && state.error.is_none() {
                return vec![Line::from(dim("running…"))];
            }
            let mut lines: Vec<Line<'static>> = state
                .output
                .iter()
                .flat_map(|o| o.rows.iter())
                .map(|row| row_line(row, width))
                .collect();
            if let Some(error) = &state.error {
                lines.push(Line::from(colored(format::truncate(error, width), BAD)));
            }
            if widget.is_stale(&state)
                && let Some(at) = state.updated
            {
                let age = format::age(at.elapsed().as_secs());
                lines.push(Line::from(colored(format!("updated {age} ago"), WARN)));
            }
            lines
        }
    }
}

fn row_line(row: &WidgetRow, width: usize) -> Line<'static> {
    if row.heading {
        let mut style = Style::default().add_modifier(Modifier::BOLD);
        if let Some(color) = level_color(row.level) {
            style = style.fg(color);
        }
        return Line::from(Span::styled(format::truncate(&row.value, width), style));
    }
    let mut spans = Vec::new();
    let mut used = 0;
    if !row.label.is_empty() {
        let label = format!("{} ", format::pad(&row.label, LABEL_WIDTH));
        spans.push(match row.label_level {
            None => dim(label),
            Some(level) => match level_color(level) {
                Some(color) => colored(label, color),
                None => Span::raw(label),
            },
        });
        used += LABEL_WIDTH + 1;
    }
    if let Some(gauge) = row.gauge {
        match row.gauge_style {
            GaugeStyle::Heat => spans.extend(meter(GAUGE_WIDTH, gauge as f64)),
            GaugeStyle::Level => spans.extend(solid_meter(
                GAUGE_WIDTH,
                gauge as f64,
                level_color(row.level).unwrap_or(Color::White),
            )),
        }
        spans.push(Span::raw(" "));
        used += GAUGE_WIDTH + 1;
    }
    let value = format::truncate(&row.value, width.saturating_sub(used));
    spans.push(match level_color(row.level) {
        Some(color) => colored(value, color),
        None => Span::raw(value),
    });
    Line::from(spans)
}

/// A meter in one color (for amounts left, where full is good).
fn solid_meter(width: usize, ratio: f64, color: Color) -> Vec<Span<'static>> {
    let filled = ((ratio.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    vec![
        colored("■".repeat(filled), color),
        colored("■".repeat(width - filled), METER_EMPTY),
    ]
}

fn level_color(level: Level) -> Option<Color> {
    match level {
        Level::Normal => None,
        Level::Dim => Some(DIM),
        Level::Ok => Some(OK),
        Level::Warn => Some(WARN),
        Level::Bad => Some(BAD),
    }
}

/// "HH:MM:SS" in 3x5 block digits, the date under it; plain text when the
/// column is too narrow for the digits.
fn clock_lines(width: usize) -> Vec<Line<'static>> {
    let now = chrono::Local::now();
    let time = now.format("%H:%M:%S").to_string();
    let date = now.format("%Y-%m-%d %a").to_string();
    let center = |text: String| {
        let pad = width.saturating_sub(text.chars().count()) / 2;
        Line::from(format!("{}{text}", " ".repeat(pad)))
    };
    let rows = big_text(&time);
    if rows[0].chars().count() > width {
        return vec![center(time), Line::from(dim(date))];
    }
    let mut lines: Vec<Line<'static>> = rows.into_iter().map(center).collect();
    let pad = width.saturating_sub(date.chars().count()) / 2;
    lines.push(Line::from(dim(format!("{}{date}", " ".repeat(pad)))));
    lines
}

/// Five rows of block characters spelling `text` (digits and ':').
fn big_text(text: &str) -> [String; 5] {
    const DIGITS: [[&str; 5]; 10] = [
        ["███", "█ █", "█ █", "█ █", "███"],
        ["  █", "  █", "  █", "  █", "  █"],
        ["███", "  █", "███", "█  ", "███"],
        ["███", "  █", "███", "  █", "███"],
        ["█ █", "█ █", "███", "  █", "  █"],
        ["███", "█  ", "███", "  █", "███"],
        ["███", "█  ", "███", "█ █", "███"],
        ["███", "  █", "  █", "  █", "  █"],
        ["███", "█ █", "███", "█ █", "███"],
        ["███", "█ █", "███", "  █", "███"],
    ];
    const COLON: [&str; 5] = [" ", "█", " ", "█", " "];
    let mut rows: [String; 5] = Default::default();
    for (i, c) in text.chars().enumerate() {
        let glyph = match c {
            '0'..='9' => &DIGITS[c as usize - '0' as usize],
            ':' => &COLON,
            _ => continue,
        };
        for (row, part) in rows.iter_mut().zip(glyph) {
            if i > 0 {
                row.push(' ');
            }
            row.push_str(part);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_digits_line_up() {
        let rows = big_text("12:34:56");
        // Six digits of 3, two colons of 1, seven gaps.
        assert!(rows.iter().all(|r| r.chars().count() == 6 * 3 + 2 + 7));
        assert_eq!(rows[0].chars().take(7).collect::<String>(), "  █ ███");
    }

    #[test]
    fn rows_with_labels_gauges_and_levels() {
        let row = WidgetRow {
            label: "5h".to_owned(),
            value: "42% · resets 14:30".to_owned(),
            gauge: Some(0.42),
            level: Level::Ok,
            ..Default::default()
        };
        let text: String = row_line(&row, 40)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.starts_with("5h       ■■■■■■■■■■ 42%"), "{text}");
        // Long values are cut to the column.
        let long = WidgetRow {
            value: "x".repeat(100),
            ..Default::default()
        };
        let width: usize = row_line(&long, 40)
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        assert!(width <= 40);

        // A level-colored gauge fills from the left in one color.
        let left = WidgetRow {
            gauge: Some(0.8),
            gauge_style: GaugeStyle::Level,
            level: Level::Ok,
            ..Default::default()
        };
        let spans = row_line(&left, 40).spans;
        assert_eq!(spans[0].content, "■■■■■■■■");
        assert_eq!(spans[0].style.fg, Some(OK));

        // Labels are dim unless colored; headings take the row's level.
        let busy = WidgetRow {
            label: "busy".to_owned(),
            label_level: Some(Level::Ok),
            ..Default::default()
        };
        assert_eq!(row_line(&busy, 40).spans[0].style.fg, Some(OK));
        let plain = WidgetRow {
            label: "idle".to_owned(),
            ..Default::default()
        };
        assert_eq!(row_line(&plain, 40).spans[0].style.fg, Some(DIM));
        let heading = WidgetRow {
            value: "rigtop".to_owned(),
            heading: true,
            level: Level::Warn,
            ..Default::default()
        };
        assert_eq!(row_line(&heading, 40).spans[0].style.fg, Some(WARN));
    }
}
