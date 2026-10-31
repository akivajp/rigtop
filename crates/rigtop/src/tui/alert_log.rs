//! The alert window (`a`): every host's alerts now, to snooze them (`z`),
//! and the history of alerts as they appeared and cleared (kept between
//! runs), to see what happened while nobody watched.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use rigtop_core::snooze::{SnoozeFor, Snoozes};
use rigtop_core::view::{Alert, AlertChange, AlertLog, Severity, alerts_with, event_time};

use super::HostTui;
use super::ui::{BAD, WARN, colored, dim, panel};

/// An alert a host has now, and its snooze: `None` when not snoozed,
/// `Some(None)` until it clears, `Some(Some(ms))` until then.
struct CurrentAlert {
    host: String,
    alert: Alert,
    snoozed: Option<Option<u64>>,
}

/// The log, the snoozes, and whether the window is open.
#[derive(Default)]
pub struct AlertLogView {
    pub log: AlertLog,
    pub snoozes: Snoozes,
    pub open: bool,
    /// Events scrolled past from the newest.
    scroll: usize,
    /// Every host's alerts as of the last [`AlertLogView::refresh`].
    current: Vec<CurrentAlert>,
    /// The alert of `current` that `z` snoozes.
    cursor: usize,
    /// What the last `z` did.
    note: Option<String>,
}

/// This machine's clock in milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl AlertLogView {
    /// Records the alerts of `host`'s latest snapshot, and the watched
    /// processes that exited (also shown in its status line). Returns how
    /// many exited.
    pub fn record(&mut self, host: &mut HostTui) -> usize {
        let Some(name) = host.alert_name() else {
            return 0;
        };
        let app = &mut host.app;
        let Some(snapshot) = &app.snapshot else {
            return 0;
        };
        let alerts = alerts_with(snapshot, &app.config.alerts);
        // This machine's clock, which remote hosts' may not agree with.
        let now_ms = now_millis();
        self.snoozes.prune(&name, &alerts, now_ms);
        let (alerts, snoozed) = self.snoozes.split(&name, alerts, now_ms);
        app.snoozed_keys = self.snoozes.keys(&name, now_ms);
        self.log.update_snoozed(&name, &alerts, &snoozed, now_ms);
        let exited = app.exit_watch.check(snapshot, now_ms);
        let count = exited.len();
        for alert in exited {
            app.set_status(alert.text.clone());
            self.log.record(&name, alert, now_ms);
        }
        count
    }

    /// Picks up snoozes made elsewhere or ended, and lists every host's
    /// alerts for the window.
    pub fn refresh(&mut self, hosts: &mut [HostTui]) {
        self.snoozes.reload();
        let now_ms = now_millis();
        self.current.clear();
        for host in hosts {
            let Some(name) = host.alert_name() else {
                continue;
            };
            host.app.snoozed_keys = self.snoozes.keys(&name, now_ms);
            let Some(snapshot) = &host.app.snapshot else {
                continue;
            };
            for alert in alerts_with(snapshot, &host.app.config.alerts) {
                let snoozed = self.snoozes.get(&name, &alert, now_ms).map(|s| s.until_ms);
                self.current.push(CurrentAlert {
                    host: name.clone(),
                    alert,
                    snoozed,
                });
            }
        }
        self.cursor = self.cursor.min(self.current.len().saturating_sub(1));
    }

    /// Opens the window.
    pub fn show(&mut self) {
        self.open = true;
        self.scroll = 0;
        self.note = None;
    }

    /// Keys while the window is open: `a`, `q` or `Esc` close it, the
    /// arrows pick an alert of now and `z` snoozes it for longer each time
    /// (1 hour, 1 day, until it clears, then not), pages scroll the
    /// history. Every key is used up.
    pub fn handle_key(&mut self, key: KeyEvent) {
        let last = self.log.events().count().saturating_sub(1);
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('q') | KeyCode::Esc => self.open = false,
            KeyCode::Down => {
                self.cursor = (self.cursor + 1).min(self.current.len().saturating_sub(1));
            }
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('z') => self.cycle_snooze(),
            KeyCode::PageDown => self.scroll = (self.scroll + 10).min(last),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = last,
            _ => {}
        }
    }

    /// Snoozes the alert under the cursor one step longer.
    fn cycle_snooze(&mut self) {
        let Some(item) = self.current.get_mut(self.cursor) else {
            return;
        };
        let now_ms = now_millis();
        let next = match item.snoozed {
            None => Some(SnoozeFor::Hour),
            // Snoozed for longer than an hour: it was a day.
            Some(Some(until)) if until.saturating_sub(now_ms) > 3_600_000 => {
                Some(SnoozeFor::UntilCleared)
            }
            Some(Some(_)) => Some(SnoozeFor::Day),
            Some(None) => None,
        };
        match next {
            Some(duration) => {
                self.snoozes
                    .snooze(&item.host, &item.alert, duration, now_ms);
                item.snoozed = self
                    .snoozes
                    .get(&item.host, &item.alert, now_ms)
                    .map(|s| s.until_ms);
                self.note = Some(format!("snoozed {}", duration.label()));
            }
            None => {
                self.snoozes.unsnooze(&item.host, &item.alert);
                item.snoozed = None;
                self.note = Some("no longer snoozed".to_owned());
            }
        }
    }

    /// Draws the window over `area`; seeing it marks the alerts as read.
    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.log.mark_read();
        let events: Vec<_> = self.log.events().rev().collect();
        let width = area.width.saturating_sub(4).min(100);
        // Borders, the alerts of now under a heading, one line before the
        // history, then the events.
        let now_lines = self.current.len().max(1) + 1;
        let height = area
            .height
            .saturating_sub(2)
            .min((now_lines + events.len().max(1)) as u16 + 3);
        if width < 20 || height < 4 {
            return;
        }
        let rect = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + 1,
            width,
            height,
        };
        let help = match &self.note {
            Some(note) => format!(" {note} "),
            None => " ↑↓ pick  z snooze  PgUp/PgDn scroll  a/Esc close ".to_owned(),
        };
        let block = panel("alerts", WARN)
            .title_top(Line::from(dim(format!(" {} events ", events.len()))).right_aligned())
            .title_bottom(Line::from(dim(help)));
        let now = SystemTime::now();
        let mut lines = vec![Line::from(Span::styled(
            "Now",
            Style::default().fg(Color::White),
        ))];
        if self.current.is_empty() {
            lines.push(Line::from(dim("  No alerts.")));
        }
        for (i, item) in self.current.iter().enumerate() {
            let marker = if i == self.cursor { "› " } else { "  " };
            let text = match item.snoozed {
                None => colored(item.alert.text.clone(), severity_color(item.alert.severity)),
                Some(until) => {
                    let until = match until {
                        Some(ms) => format!("until {}", event_time(ms, now)),
                        None => "until it clears".to_owned(),
                    };
                    dim(format!("{} (snoozed {until})", item.alert.text))
                }
            };
            lines.push(Line::from(vec![
                Span::raw(marker),
                Span::styled(
                    format!("{}  ", item.host),
                    Style::default().fg(Color::White),
                ),
                text,
            ]));
        }
        lines.push(Line::from(dim(if events.is_empty() {
            "History: no alerts yet."
        } else {
            "History, newest first:"
        })));
        for event in events.iter().skip(self.scroll) {
            let when = event_time(event.at_ms, now);
            let text = match event.change {
                AlertChange::Raised | AlertChange::Notice => colored(
                    event.alert.text.clone(),
                    severity_color(event.alert.severity),
                ),
                AlertChange::Cleared => dim(format!("cleared: {}", event.alert.text)),
            };
            lines.push(Line::from(vec![
                dim(format!("{when:>12}  ")),
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
        view.handle_key(key(KeyCode::PageDown));
        assert_eq!(view.scroll, 1, "stops at the oldest event");
        view.handle_key(key(KeyCode::Home));
        assert_eq!(view.scroll, 0);
        view.handle_key(key(KeyCode::Esc));
        assert!(!view.open);
    }

    #[test]
    fn z_snoozes_longer_each_time_then_not() {
        let mut view = AlertLogView::default();
        let disk = alert("sdc: SMART warning", Severity::Warning);
        view.current = vec![
            CurrentAlert {
                host: "nas".to_owned(),
                alert: alert("md0 degraded", Severity::Critical),
                snoozed: None,
            },
            CurrentAlert {
                host: "nas".to_owned(),
                alert: disk.clone(),
                snoozed: None,
            },
        ];
        view.show();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        view.handle_key(key(KeyCode::Down));
        let until = |view: &AlertLogView| {
            view.snoozes
                .get("nas", &disk, now_millis())
                .map(|s| s.until_ms)
        };
        view.handle_key(key(KeyCode::Char('z')));
        let hour = until(&view).unwrap().unwrap() - now_millis();
        assert!((3_590_000..=3_600_000).contains(&hour), "{hour}");
        view.handle_key(key(KeyCode::Char('z')));
        assert!(until(&view).unwrap().unwrap() - now_millis() > 80_000_000);
        view.handle_key(key(KeyCode::Char('z')));
        assert_eq!(until(&view), Some(None));
        assert_eq!(view.note.as_deref(), Some("snoozed until it clears"));
        view.handle_key(key(KeyCode::Char('z')));
        assert_eq!(until(&view), None);
        // The first alert was never touched.
        assert_eq!(view.snoozes.entries().len(), 0);
    }

    #[test]
    fn badge_shows_unread_alerts() {
        assert!(unread_badge((0, None)).is_none());
        let badge = unread_badge((2, Some(Severity::Critical))).unwrap();
        assert_eq!(badge.content, "alerts 2");
        assert_eq!(badge.style.fg, Some(BAD));
    }
}
