//! The alert history (`a`): the alerts of every host as they appeared and
//! cleared since rigtop started, to see what happened while nobody watched.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use rigtop_core::view::{AlertChange, AlertLog, Severity, alerts_with};

use super::HostTui;
use super::ui::{BAD, WARN, colored, dim, panel};

/// The log and whether its window is open.
#[derive(Default)]
pub struct AlertLogView {
    pub log: AlertLog,
    pub open: bool,
    /// Events scrolled past from the newest.
    scroll: usize,
}

impl AlertLogView {
    /// Records the alerts of `host`'s latest snapshot.
    pub fn record(&mut self, host: &HostTui) {
        let app = &host.app;
        let Some(snapshot) = &app.snapshot else {
            return;
        };
        let alerts = alerts_with(snapshot, &app.config.alerts);
        // "local" says little in a list of hosts; use the host name.
        let name = if host.spec.is_local() {
            &snapshot.host.hostname
        } else {
            &host.spec.name
        };
        // This machine's clock, which remote hosts' may not agree with.
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        self.log.update(name, &alerts, now_ms);
    }

    /// Opens the window.
    pub fn show(&mut self) {
        self.open = true;
        self.scroll = 0;
    }

    /// Keys while the window is open: `a`, `q` or `Esc` close it, the
    /// arrows and pages scroll. Every key is used up.
    pub fn handle_key(&mut self, key: KeyEvent) {
        let last = self.log.events().count().saturating_sub(1);
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('q') | KeyCode::Esc => self.open = false,
            KeyCode::Down => self.scroll = (self.scroll + 1).min(last),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = (self.scroll + 10).min(last),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = last,
            _ => {}
        }
    }

    /// Draws the window over `area`; seeing it marks the alerts as read.
    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.log.mark_read();
        let events: Vec<_> = self.log.events().rev().collect();
        let width = area.width.saturating_sub(4).min(100);
        // Borders and one line of help, then the events.
        let height = area
            .height
            .saturating_sub(2)
            .min(events.len().max(1) as u16 + 3);
        if width < 20 || height < 4 {
            return;
        }
        let rect = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + 1,
            width,
            height,
        };
        let block = panel("alerts", WARN)
            .title_top(Line::from(dim(format!(" {} events ", events.len()))).right_aligned())
            .title_bottom(Line::from(dim(" ↑↓ scroll  a/Esc close ")));
        let today = Local::now().date_naive();
        let mut lines = vec![Line::from(dim(if events.is_empty() {
            "No alerts since rigtop started."
        } else {
            "Newest first, since rigtop started."
        }))];
        for event in events.iter().skip(self.scroll) {
            let when = Local
                .timestamp_millis_opt(event.at_ms as i64)
                .single()
                .map(|t| {
                    if t.date_naive() == today {
                        t.format("%H:%M:%S").to_string()
                    } else {
                        t.format("%a %H:%M").to_string()
                    }
                })
                .unwrap_or_default();
            let text = match event.change {
                AlertChange::Raised => colored(
                    event.alert.text.clone(),
                    severity_color(event.alert.severity),
                ),
                AlertChange::Cleared => dim(format!("cleared: {}", event.alert.text)),
            };
            lines.push(Line::from(vec![
                dim(format!("{when:>9}  ")),
                Span::styled(
                    format!("{}  ", event.host),
                    Style::default().fg(Color::White),
                ),
                text,
            ]));
        }
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

/// "alerts 3" in the color of the worst unread alert, or `None` when
/// everything has been seen (`unread` from [`AlertLog::unread`]).
pub fn unread_badge(unread: (usize, Option<Severity>)) -> Option<Span<'static>> {
    let (count, worst) = unread;
    let worst = worst.filter(|_| count > 0)?;
    Some(colored(format!("alerts {count}"), severity_color(worst)))
}

fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Critical => BAD,
        Severity::Warning => WARN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;
    use rigtop_core::view::Alert;

    fn alert(text: &str, severity: Severity) -> Alert {
        Alert {
            severity,
            text: text.to_owned(),
        }
    }

    #[test]
    fn keys_scroll_and_close() {
        let mut view = AlertLogView::default();
        view.log
            .update("nas", &[alert("md0 degraded", Severity::Critical)], 1_000);
        view.log.update("nas", &[], 2_000);
        view.show();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.scroll, 1, "stops at the oldest event");
        view.handle_key(key(KeyCode::Home));
        assert_eq!(view.scroll, 0);
        view.handle_key(key(KeyCode::Esc));
        assert!(!view.open);
    }

    #[test]
    fn badge_shows_unread_alerts() {
        assert!(unread_badge((0, None)).is_none());
        let badge = unread_badge((2, Some(Severity::Critical))).unwrap();
        assert_eq!(badge.content, "alerts 2");
        assert_eq!(badge.style.fg, Some(BAD));
    }
}
