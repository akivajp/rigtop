//! A window listing the alerts of every host as they appeared and cleared
//! (kept between runs), to see what happened while nobody watched.

use egui::{RichText, Ui};
use rigtop_core::view::{AlertChange, AlertLog, Severity, event_time};

use crate::app::ViewAction;
use crate::widgets::{BAD, DIM, TEXT, WARN};

/// "alerts…", or the number of unread alerts in the color of the worst
/// (`unread` from [`AlertLog::unread`]).
pub fn button(ui: &mut Ui, unread: (usize, Option<Severity>), actions: &mut Vec<ViewAction>) {
    let (unread, worst) = unread;
    let (text, color) = match worst {
        Some(severity) => (format!("alerts {unread}"), severity_color(severity)),
        None => ("alerts…".to_owned(), DIM),
    };
    let hint = if unread > 0 {
        "new alerts since you last looked: show the alert history"
    } else {
        "show the alerts of every host as they appeared and cleared"
    };
    if ui
        .add(egui::Button::new(RichText::new(text).color(color)).frame(false))
        .on_hover_text(hint)
        .clicked()
    {
        actions.push(ViewAction::ShowAlertLog);
    }
}

/// Draws the window while `open`; showing it marks the alerts as read.
pub fn show(ctx: &egui::Context, open: &mut bool, log: &mut AlertLog) {
    if !*open {
        return;
    }
    log.mark_read();
    let screen = ctx.content_rect();
    egui::Window::new("Alerts")
        .open(open)
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(egui::pos2(screen.center().x, screen.top() + 60.0))
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .show(ctx, |ui| contents(ui, log, screen.height()));
}

fn contents(ui: &mut Ui, log: &AlertLog, screen_height: f32) {
    if log.events().next().is_none() {
        ui.label(RichText::new("No alerts yet.").color(DIM));
        return;
    }
    ui.label(RichText::new("Newest first.").color(DIM));
    ui.add_space(4.0);
    let now = std::time::SystemTime::now();
    egui::ScrollArea::vertical()
        .max_height((screen_height * 0.6).max(200.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("alert_log")
                .striped(true)
                .num_columns(3)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    for event in log.events().rev() {
                        let when = event_time(event.at_ms, now);
                        ui.label(RichText::new(when).color(DIM).monospace());
                        ui.label(RichText::new(&event.host).color(TEXT));
                        match event.change {
                            AlertChange::Raised | AlertChange::Notice => ui.label(
                                RichText::new(&event.alert.text)
                                    .color(severity_color(event.alert.severity)),
                            ),
                            AlertChange::Cleared => ui.label(
                                RichText::new(format!("cleared: {}", event.alert.text)).color(DIM),
                            ),
                        };
                        ui.end_row();
                    }
                });
        });
}

fn severity_color(severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Critical => BAD,
        Severity::Warning => WARN,
    }
}
