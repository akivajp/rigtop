//! The widget column on the right of the window: a large clock and panels
//! fed by commands (see [`rigtop_core::widget`]), shown next to the overview
//! and the host views alike.

use egui::{Color32, CornerRadius, FontId, Margin, RichText, Sense, Stroke, Ui, vec2};
use rigtop_core::format;
use rigtop_core::widget::{GaugeStyle, Level, Widget, WidgetRow, WidgetSource, WidgetState};

use crate::widgets::{self, BAD, BORDER, DIM, OK, TEXT, WARN};

/// Initial width of the column; it can be resized by dragging its edge.
pub const DEFAULT_WIDTH: f32 = 320.0;
const ROW_HEIGHT: f32 = 16.0;
const LABEL_WIDTH: f32 = 64.0;

/// Draws all widgets, scrolling when they do not fit.
pub fn show(ui: &mut Ui, widgets: &[Widget]) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, widget) in widgets.iter().enumerate() {
                ui.push_id(index, |ui| widget_frame(ui, widget));
                ui.add_space(4.0);
            }
        });
}

fn widget_frame(ui: &mut Ui, widget: &Widget) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                RichText::new(&widget.name)
                    .color(widgets::WIDGET_TITLE)
                    .strong(),
            );
            match &widget.source {
                WidgetSource::Clock => clock(ui),
                WidgetSource::Invalid(problem) => {
                    ui.label(RichText::new(problem).color(WARN));
                }
                WidgetSource::Command { .. } | WidgetSource::Fixed(_) => {
                    if let Some(state) = widget.state() {
                        content(ui, widget, &state);
                    }
                }
            }
        });
}

/// The time in large digits, the date under it.
fn clock(ui: &mut Ui) {
    let now = chrono::Local::now();
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(now.format("%H:%M:%S").to_string())
                .font(FontId::monospace(36.0))
                .color(TEXT),
        );
        ui.label(RichText::new(now.format("%Y-%m-%d %a").to_string()).color(DIM));
    });
}

fn content(ui: &mut Ui, widget: &Widget, state: &WidgetState) {
    if state.output.is_none() && state.error.is_none() {
        ui.label(RichText::new("running…").color(DIM));
        return;
    }
    if let Some(output) = &state.output {
        for row in &output.rows {
            row_ui(ui, row);
        }
    }
    // A failed run keeps the previous rows and says why underneath.
    if let Some(error) = &state.error {
        ui.label(RichText::new(error).color(BAD));
    }
    if widget.is_stale(state)
        && let Some(at) = state.updated
    {
        ui.label(
            RichText::new(format!(
                "updated {} ago",
                format::age(at.elapsed().as_secs())
            ))
            .color(WARN),
        );
    }
}

fn row_ui(ui: &mut Ui, row: &WidgetRow) {
    if row.separator {
        ui.separator();
        return;
    }
    if row.heading {
        ui.label(
            RichText::new(&row.value)
                .color(level_color(row.level))
                .strong(),
        );
        return;
    }
    ui.horizontal(|ui| {
        if !row.label.is_empty() {
            // A fixed, left-aligned column so values line up.
            ui.allocate_ui_with_layout(
                vec2(LABEL_WIDTH, ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(LABEL_WIDTH);
                    let color = row.label_level.map_or(DIM, level_color);
                    ui.add(egui::Label::new(RichText::new(&row.label).color(color)).truncate());
                },
            );
        }
        if let Some(gauge) = row.gauge {
            let width = (ui.available_width() * 0.4).clamp(40.0, 140.0);
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
            match row.gauge_style {
                GaugeStyle::Heat => widgets::paint_meter(ui, rect, gauge.clamp(0.0, 1.0)),
                GaugeStyle::Level => {
                    widgets::paint_solid_meter(ui, rect, gauge, level_color(row.level))
                }
            }
        }
        ui.add(
            egui::Label::new(RichText::new(&row.value).color(level_color(row.level))).truncate(),
        );
    });
}

fn level_color(level: Level) -> Color32 {
    match level {
        Level::Normal => TEXT,
        Level::Dim => DIM,
        Level::Ok => OK,
        Level::Warn => WARN,
        Level::Bad => BAD,
    }
}
