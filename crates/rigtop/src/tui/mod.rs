//! Terminal UI.

mod app;
mod graph;
mod overview;
mod ui;
mod widget_panel;

use std::io::stdout;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use rigtop_core::demo::DemoSource;
use rigtop_core::hosts::{HostKind, HostSpec};
use rigtop_core::remote::RemoteSource;
use rigtop_core::widget::{Widget, start_widgets};
use rigtop_core::worker::{Command, Worker};
use rigtop_core::{Config, DataSource, LocalSource};

use app::App;
use overview::View;

/// How often to check for input while idle. Drawing only happens when
/// something changed, so this mainly bounds input latency.
const INPUT_POLL: Duration = Duration::from_millis(50);

/// One monitored host: its collector and the state its views show.
pub struct HostTui {
    pub spec: HostSpec,
    worker: Worker,
    pub app: App,
    /// Process limit last sent to the worker (`None` = all processes).
    sent_limit: Option<Option<usize>>,
}

/// Runs the TUI until the user quits. With several hosts (this machine plus
/// remote ones) it starts on the overview, with a tab per host.
pub fn run(config: Config, interval: Duration, specs: Vec<HostSpec>) -> Result<()> {
    let mut hosts: Vec<HostTui> = specs
        .into_iter()
        .map(|spec| {
            let source_config = config.clone();
            let kind = spec.kind.clone();
            let worker = Worker::spawn(
                move || -> Box<dyn DataSource> {
                    match kind {
                        HostKind::Remote(target) => Box::new(RemoteSource::new(target)),
                        HostKind::Local => Box::new(LocalSource::with_config(&source_config)),
                        HostKind::Demo(host) => Box::new(DemoSource::new(host)),
                    }
                },
                interval,
            );
            let mut app = App::new(config.clone(), interval);
            if !spec.is_local() {
                app.waiting_message = format!("connecting to {}…", spec.name);
            }
            HostTui {
                spec,
                worker,
                app,
                sent_limit: None,
            }
        })
        .collect();
    // The TUI redraws every second anyway, which shows new widget content.
    let widgets = start_widgets(&config.widgets, || {});
    let mut screen = Screen {
        view: if hosts.len() > 1 {
            View::Overview
        } else {
            View::Host(0)
        },
        include_local: config.overview.include_local,
        processes: config.overview.processes,
        tabs: Vec::new(),
        quit: false,
        show_widgets: true,
    };

    // ratatui::init installs a panic hook that restores the terminal; mouse
    // capture is ours to undo, also when panicking.
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        previous_hook(info);
    }));

    let result = event_loop(&mut terminal, &mut hosts, &mut screen, &widgets);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Screen-level state above the per-host apps.
struct Screen {
    view: View,
    /// Whether the overview has a box for this machine.
    include_local: bool,
    /// Processes per overview box (and sent by hosts not shown in detail).
    processes: usize,
    /// Clickable tab regions from the last frame.
    tabs: Vec<(Rect, View)>,
    quit: bool,
    /// Whether the widget column is shown (`w`).
    show_widgets: bool,
}

impl Screen {
    /// Views in tab order: the overview, then each host.
    fn step(&mut self, hosts: usize, forward: bool) {
        let count = hosts + 1;
        let position = match self.view {
            View::Overview => 0,
            View::Host(i) => i + 1,
        };
        let next = if forward {
            (position + 1) % count
        } else {
            (position + count - 1) % count
        };
        self.view = if next == 0 {
            View::Overview
        } else {
            View::Host(next - 1)
        };
    }

    /// Handles keys that switch views. Returns true when the key was used.
    fn handle_key(&mut self, key: KeyEvent, hosts: usize) -> bool {
        match key.code {
            KeyCode::Tab => self.step(hosts, true),
            KeyCode::BackTab => self.step(hosts, false),
            KeyCode::Char('0') => self.view = View::Overview,
            KeyCode::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if index < hosts {
                    self.view = View::Host(index);
                }
            }
            KeyCode::Char('l') if self.view == View::Overview => {
                self.include_local = !self.include_local
            }
            KeyCode::Char('q') if self.view == View::Overview => self.quit = true,
            _ => return false,
        }
        true
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    hosts: &mut [HostTui],
    screen: &mut Screen,
    widgets: &[Widget],
) -> Result<()> {
    let multi = hosts.len() > 1;
    let mut dirty = true;
    let mut status_visible = false;
    let mut drawn_second = 0;
    while !screen.quit {
        for host in hosts.iter_mut() {
            while let Some(event) = host.worker.try_recv() {
                host.app.apply(event);
                dirty = true;
            }
        }
        // Keep the clock ticking even with long polling intervals.
        let second = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if second != drawn_second {
            drawn_second = second;
            dirty = true;
        }
        // Redraw once more when a status message expires.
        let active_status = match screen.view {
            View::Host(i) => hosts[i].app.status().is_some(),
            View::Overview => false,
        };
        if status_visible != active_status {
            status_visible = active_status;
            dirty = true;
        }
        if dirty {
            terminal.draw(|frame| {
                let area = widget_column(frame, frame.area(), screen, widgets);
                let body = if multi {
                    let [bar, body] =
                        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
                    screen.tabs = overview::draw_tabs(frame, bar, hosts, screen.view);
                    body
                } else {
                    area
                };
                match screen.view {
                    View::Overview => overview::draw_overview(
                        frame,
                        body,
                        hosts,
                        screen.include_local,
                        screen.processes,
                    ),
                    View::Host(i) => ui::draw_in(frame, &mut hosts[i].app, body),
                }
            })?;
            dirty = false;
        }
        if event::poll(INPUT_POLL)? {
            match event::read()? {
                TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                    handle_key(key, hosts, screen, multi, !widgets.is_empty());
                    dirty = true;
                }
                TermEvent::Mouse(mouse) => {
                    let clicked_tab = (mouse.kind == MouseEventKind::Down(MouseButton::Left))
                        .then(|| {
                            let position = Position::new(mouse.column, mouse.row);
                            screen.tabs.iter().find(|(rect, _)| rect.contains(position))
                        })
                        .flatten();
                    if let Some((_, view)) = clicked_tab {
                        screen.view = *view;
                    } else if let View::Host(i) = screen.view {
                        hosts[i].app.handle_mouse(mouse);
                    }
                    dirty = true;
                }
                TermEvent::Resize(..) => dirty = true,
                _ => {}
            }
        }
        if let View::Host(i) = screen.view
            && hosts[i].app.should_quit
        {
            screen.quit = true;
        }
        // The host shown in detail sends every process; the others only
        // what an overview box lists.
        for (index, host) in hosts.iter_mut().enumerate() {
            let limit = if screen.view == View::Host(index) {
                None
            } else {
                Some(screen.processes)
            };
            if host.sent_limit != Some(limit) {
                host.worker.send(Command::SetProcessLimit(limit));
                host.sent_limit = Some(limit);
            }
        }
    }
    Ok(())
}

/// Routes a key: view switching first (unless the active host is typing a
/// filter or confirming a signal), then the active host's own keys.
/// Draws the widget column on the right when there are widgets, they are
/// not hidden and the terminal is wide enough; returns the rest of `area`.
fn widget_column(
    frame: &mut ratatui::Frame,
    area: Rect,
    screen: &Screen,
    widgets: &[Widget],
) -> Rect {
    if widgets.is_empty() || !screen.show_widgets || area.width < widget_panel::MIN_TERMINAL_WIDTH {
        return area;
    }
    let [main, side] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(widget_panel::WIDTH)])
            .areas(area);
    widget_panel::draw(frame, side, widgets);
    main
}

fn handle_key(
    key: KeyEvent,
    hosts: &mut [HostTui],
    screen: &mut Screen,
    multi: bool,
    has_widgets: bool,
) {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        screen.quit = true;
        return;
    }
    let busy = match screen.view {
        View::Host(i) => hosts[i].app.editing_filter || hosts[i].app.confirm.is_some(),
        View::Overview => false,
    };
    if has_widgets && !busy && key.code == KeyCode::Char('w') {
        screen.show_widgets = !screen.show_widgets;
        return;
    }
    if multi && !busy && screen.handle_key(key, hosts.len()) {
        return;
    }
    if let View::Host(i) = screen.view {
        if let Some(command) = hosts[i].app.handle_key(key) {
            hosts[i].worker.send(command);
        }
        // Every host's graphs cover the same time, like in the GUI.
        let span = hosts[i].app.graph_span;
        for host in hosts.iter_mut() {
            host.app.graph_span = span;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen {
            view: View::Overview,
            include_local: true,
            processes: 10,
            tabs: Vec::new(),
            quit: false,
            show_widgets: true,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn tab_and_digits_switch_views() {
        let mut screen = screen();
        assert!(screen.handle_key(key(KeyCode::Tab), 2));
        assert_eq!(screen.view, View::Host(0));
        screen.handle_key(key(KeyCode::Tab), 2);
        screen.handle_key(key(KeyCode::Tab), 2);
        assert_eq!(screen.view, View::Overview, "wraps around");
        screen.handle_key(key(KeyCode::BackTab), 2);
        assert_eq!(screen.view, View::Host(1));
        screen.handle_key(key(KeyCode::Char('1')), 2);
        assert_eq!(screen.view, View::Host(0));
        // Digits beyond the host count do nothing.
        screen.handle_key(key(KeyCode::Char('5')), 2);
        assert_eq!(screen.view, View::Host(0));
        screen.handle_key(key(KeyCode::Char('0')), 2);
        assert_eq!(screen.view, View::Overview);
    }

    #[test]
    fn overview_keys_toggle_local_and_quit() {
        let mut screen = screen();
        screen.handle_key(key(KeyCode::Char('l')), 2);
        assert!(!screen.include_local);
        // In a host view, "l" and "q" belong to the host's own keys.
        screen.view = View::Host(0);
        assert!(!screen.handle_key(key(KeyCode::Char('q')), 2));
        screen.view = View::Overview;
        screen.handle_key(key(KeyCode::Char('q')), 2);
        assert!(screen.quit);
    }
}
