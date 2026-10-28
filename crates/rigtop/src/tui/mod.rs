//! Terminal UI.

mod alert_log;
mod app;
mod graph;
mod overview;
mod ui;
mod update;
mod widget_panel;

use std::io::stdout;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use rigtop_core::config::ConfigWatcher;
use rigtop_core::config::UpdatesConfig;
use rigtop_core::demo::DemoSource;
use rigtop_core::hosts::{HostKind, HostSpec};
use rigtop_core::remote::RemoteSource;
use rigtop_core::update::{ReleaseChecker, UpdateJob};
use rigtop_core::widget::{Widget, start_widgets};
use rigtop_core::worker::{Command, Worker};
use rigtop_core::{Config, DataSource, LocalSource};

use alert_log::AlertLogView;
use app::App;
use overview::View;
use update::{Target, UpdateWindow};

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

impl HostTui {
    /// Starts collecting from `spec`.
    fn start(spec: HostSpec, config: &Config, interval: Duration) -> Self {
        let worker = spawn_worker(&spec, config, interval);
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
    }
}

/// A collector thread for `spec`.
fn spawn_worker(spec: &HostSpec, config: &Config, interval: Duration) -> Worker {
    let source_config = config.clone();
    let kind = spec.kind.clone();
    Worker::spawn(
        move || -> Box<dyn DataSource> {
            match kind {
                HostKind::Remote(target) => Box::new(RemoteSource::new(target)),
                HostKind::Local => Box::new(LocalSource::with_config(&source_config)),
                HostKind::Demo(host) => Box::new(DemoSource::new(host)),
            }
        },
        interval,
    )
}

/// Stops workers without waiting: joining one that is still connecting
/// would freeze the screen.
fn stop_workers(workers: Vec<Worker>) {
    if !workers.is_empty() {
        thread::spawn(move || drop(workers));
    }
}

/// The hosts to monitor under a configuration (`[[hosts]]` plus the
/// command line, or the demo hosts).
pub type HostSpecs = Box<dyn Fn(&Config) -> Vec<HostSpec>>;

/// What applying an edited configuration file needs.
struct Reload {
    /// `None` without a configuration file (e.g. no config directory).
    watcher: Option<ConfigWatcher>,
    /// The configuration in use.
    config: Config,
    host_specs: HostSpecs,
    interval: Duration,
}

/// New releases and this machine's copy of rigtop (`U`).
struct Releases {
    /// `None` when update checks are off (`[updates] check = false`).
    checker: Option<ReleaseChecker>,
    /// The running program, taken at startup (see `UpdateJob::start_self`).
    exe: Option<PathBuf>,
    /// A release installed in place of the running program, which runs
    /// after a restart.
    installed: Option<String>,
}

impl Releases {
    fn new(config: &UpdatesConfig, exe: Option<PathBuf>) -> Self {
        Self {
            checker: config.check.then(|| ReleaseChecker::new(config.interval())),
            exe,
            installed: None,
        }
    }

    /// A newer release than this rigtop, once seen.
    fn available(&mut self) -> Option<String> {
        self.checker.as_mut()?.poll().map(str::to_owned)
    }

    /// What the title says about releases, if anything.
    fn note(&mut self) -> Option<String> {
        match &self.installed {
            Some(version) => Some(format!("restart for rigtop {version}")),
            None => self
                .available()
                .map(|version| format!("rigtop {version} available (U: update)")),
        }
    }
}

/// Runs the TUI until the user quits. With several hosts (this machine plus
/// remote ones) it starts on the overview, with a tab per host. Edits to
/// `config_path` are applied while running. `exe` is the running program,
/// which `U` replaces with a new release.
pub fn run(
    config: Config,
    config_path: Option<PathBuf>,
    interval: Duration,
    host_specs: HostSpecs,
    exe: Option<PathBuf>,
) -> Result<()> {
    let mut hosts: Vec<HostTui> = host_specs(&config)
        .into_iter()
        .map(|spec| HostTui::start(spec, &config, interval))
        .collect();
    // The TUI redraws every second anyway, which shows new widget content.
    let mut widgets = start_widgets(&config.widgets, || {});
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
        alert_log: AlertLogView::default(),
        update: None,
        releases: Releases::new(&config.updates, exe),
    };
    let mut reload = Reload {
        watcher: config_path.map(ConfigWatcher::new),
        config,
        host_specs,
        interval,
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

    let result = event_loop(
        &mut terminal,
        &mut hosts,
        &mut screen,
        &mut widgets,
        &mut reload,
    );
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
    /// Every host's alerts as they came and went (`a`).
    alert_log: AlertLogView,
    /// Updating rigtop here (`U`) or on a remote host (`u`).
    update: Option<UpdateWindow>,
    releases: Releases,
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
    hosts: &mut Vec<HostTui>,
    screen: &mut Screen,
    widgets: &mut Vec<Widget>,
    reload: &mut Reload,
) -> Result<()> {
    let mut dirty = true;
    let mut status_visible = false;
    let mut drawn_second = 0;
    while !screen.quit {
        if let Some(result) = reload.watcher.as_mut().and_then(ConfigWatcher::poll) {
            match result {
                Ok(config) => apply_config(config, reload, hosts, screen, widgets),
                Err(err) => {
                    // The first line names the file and the problem.
                    let reason = err.to_string();
                    let reason = reason.lines().next().unwrap_or_default();
                    for host in hosts.iter_mut() {
                        host.app
                            .set_status(format!("configuration not applied: {reason}"));
                    }
                }
            }
            dirty = true;
        }
        if finish_update(screen, hosts) {
            dirty = true;
        }
        let release_note = screen.releases.note();
        let multi = hosts.len() > 1;
        for host in hosts.iter_mut() {
            let mut received = false;
            while let Some(event) = host.worker.try_recv() {
                host.app.apply(event);
                received = true;
            }
            if received {
                screen.alert_log.record(host);
                dirty = true;
            }
        }
        // With one host there is no tab bar; its CPU box shows new alerts.
        let unread = screen.alert_log.log.unread();
        if !multi {
            hosts[0].app.alerts_unread = Some(unread);
        }
        for host in hosts.iter_mut() {
            if host.app.release_note != release_note {
                host.app.release_note = release_note.clone();
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
                    screen.tabs = overview::draw_tabs(
                        frame,
                        bar,
                        hosts,
                        screen.view,
                        unread,
                        release_note.as_deref(),
                    );
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
                if screen.alert_log.open {
                    screen.alert_log.draw(frame, body);
                }
                if let Some(window) = &screen.update {
                    window.draw(frame, body);
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

/// Applies an edited configuration: host names, sensor names, alert
/// thresholds and the overview take effect at once; hosts added to or
/// removed from `[[hosts]]` start or stop (the others keep their connection
/// and history); widgets and this machine's collector restart when their
/// settings changed.
fn apply_config(
    config: Config,
    reload: &mut Reload,
    hosts: &mut Vec<HostTui>,
    screen: &mut Screen,
    widgets: &mut Vec<Widget>,
) {
    let specs = (reload.host_specs)(&config);
    if specs.is_empty() {
        return;
    }
    let old = std::mem::replace(&mut reload.config, config);
    let config = &reload.config;

    let shown = match screen.view {
        View::Host(index) => hosts.get(index).map(|h| h.spec.name.clone()),
        View::Overview => None,
    };
    let mut previous = std::mem::take(hosts);
    for spec in specs {
        match previous
            .iter()
            .position(|h| h.spec.name == spec.name && h.spec.kind == spec.kind)
        {
            Some(position) => {
                let mut host = previous.swap_remove(position);
                host.spec = spec;
                hosts.push(host);
            }
            None => hosts.push(HostTui::start(spec, config, reload.interval)),
        }
    }
    let mut stopped: Vec<Worker> = previous.into_iter().map(|h| h.worker).collect();
    // This machine's collector reads these settings when it starts.
    if old.smart != config.smart || old.btrfs != config.btrfs || old.lhm != config.lhm {
        for host in hosts.iter_mut().filter(|h| h.spec.is_local()) {
            let worker = spawn_worker(&host.spec, config, host.app.interval);
            stopped.push(std::mem::replace(&mut host.worker, worker));
            host.sent_limit = None;
        }
    }
    stop_workers(stopped);
    for host in hosts.iter_mut() {
        host.app.config = config.clone();
        host.app.set_status("configuration reloaded".to_owned());
    }
    if old.widgets != config.widgets {
        *widgets = start_widgets(&config.widgets, || {});
    }
    if old.updates != config.updates {
        let exe = screen.releases.exe.take();
        let installed = screen.releases.installed.take();
        screen.releases = Releases::new(&config.updates, exe);
        screen.releases.installed = installed;
    }
    if old.overview.include_local != config.overview.include_local {
        screen.include_local = config.overview.include_local;
    }
    screen.processes = config.overview.processes;

    // Stay on the same host if it is still there.
    let position = shown.and_then(|name| hosts.iter().position(|h| h.spec.name == name));
    screen.view = match position {
        Some(index) => View::Host(index),
        None if hosts.len() > 1 => View::Overview,
        None => View::Host(0),
    };
}

/// Opens the update window for the host at `index`, or says why it does
/// not apply.
fn ask_update(screen: &mut Screen, host: &mut HostTui, index: usize) {
    let (agent_version, os) = host
        .app
        .snapshot
        .as_ref()
        .map(|s| (s.host.rigtop_version.clone(), s.host.os.clone()))
        .unwrap_or_default();
    if !matches!(host.spec.kind, HostKind::Remote(_)) {
        host.app
            .set_status("u updates rigtop on remote hosts".to_owned());
    } else if !rigtop_core::update::agent_is_older(&agent_version) {
        host.app.set_status(format!(
            "{} already runs rigtop {} or newer",
            host.spec.name,
            rigtop_core::update::target_version()
        ));
    } else if let Some(reason) = rigtop_core::update::unsupported_reason(&os) {
        host.app.set_status(reason);
    } else {
        screen.update = Some(UpdateWindow::Confirm {
            target: Target::Host(index),
            text: rigtop_core::update::describe(&host.spec.name, &agent_version),
        });
    }
}

/// Opens the window for updating this machine's copy to a new release, or
/// says (on `host`'s status line) why it does not apply.
fn ask_self_update(screen: &mut Screen, host: &mut HostTui) {
    let available = screen.releases.available();
    if let Some(version) = &screen.releases.installed {
        host.app
            .set_status(format!("quit and start rigtop again to use {version}"));
    } else if screen.releases.checker.is_none() {
        host.app
            .set_status("update checks are off ([updates] check = false)".to_owned());
    } else if let Some(version) = available {
        match rigtop_core::update::self_update_unsupported() {
            Some(reason) => host.app.set_status(reason),
            None => {
                screen.update = Some(UpdateWindow::Confirm {
                    text: rigtop_core::update::describe_self(&version),
                    target: Target::Local { version },
                })
            }
        }
    } else {
        host.app.set_status(format!(
            "no newer release seen than rigtop {}",
            rigtop_core::update::target_version()
        ));
    }
}

/// Starts the confirmed update of `target`.
fn start_update(hosts: &[HostTui], releases: &Releases, target: Target) -> Option<UpdateWindow> {
    let (name, job) = match &target {
        Target::Host(index) => {
            let host = hosts.get(*index)?;
            let HostKind::Remote(remote) = &host.spec.kind else {
                return None;
            };
            (host.spec.name.clone(), UpdateJob::start(remote))
        }
        Target::Local { version } => {
            let Some(exe) = &releases.exe else {
                return Some(UpdateWindow::Done {
                    target,
                    result: Err("the running program was not found".to_owned()),
                });
            };
            (
                "this machine".to_owned(),
                UpdateJob::start_self(version, exe),
            )
        }
    };
    Some(UpdateWindow::Running { target, name, job })
}

/// Shows the outcome of a finished update. After a successful one, a remote
/// host reconnects so that the new agent runs, and this machine's title
/// asks for a restart. Returns whether anything changed.
fn finish_update(screen: &mut Screen, hosts: &mut [HostTui]) -> bool {
    let Some(UpdateWindow::Running { target, job, .. }) = &screen.update else {
        return false;
    };
    let Some(result) = job.poll() else {
        return false;
    };
    let target = target.clone();
    if result.is_ok() {
        match &target {
            Target::Host(index) => {
                if let Some(entry) = hosts.get_mut(*index) {
                    let worker = spawn_worker(&entry.spec, &entry.app.config, entry.app.interval);
                    stop_workers(vec![std::mem::replace(&mut entry.worker, worker)]);
                    entry.sent_limit = None;
                }
            }
            Target::Local { version } => screen.releases.installed = Some(version.clone()),
        }
    }
    screen.update = Some(UpdateWindow::Done { target, result });
    true
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
    if screen.alert_log.open {
        screen.alert_log.handle_key(key);
        return;
    }
    if let Some(window) = screen.update.take() {
        screen.update = match window {
            UpdateWindow::Confirm { target, .. } if key.code == KeyCode::Char('y') => {
                start_update(hosts, &screen.releases, target)
            }
            // Runs on; the outcome shows when it is done.
            running @ UpdateWindow::Running { .. } => Some(running),
            _ => None,
        };
        return;
    }
    let busy = match screen.view {
        View::Host(i) => hosts[i].app.editing_filter || hosts[i].app.confirm.is_some(),
        View::Overview => false,
    };
    if !busy && key.code == KeyCode::Char('a') {
        screen.alert_log.show();
        return;
    }
    if !busy
        && key.code == KeyCode::Char('u')
        && let View::Host(i) = screen.view
    {
        ask_update(screen, &mut hosts[i], i);
        return;
    }
    if !busy && key.code == KeyCode::Char('U') {
        // The overview has no status line; the first host's view shows it.
        let index = match screen.view {
            View::Host(i) => i,
            View::Overview => 0,
        };
        ask_self_update(screen, &mut hosts[index]);
        return;
    }
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
            alert_log: AlertLogView::default(),
            update: None,
            releases: Releases::new(
                &UpdatesConfig {
                    check: false,
                    ..Default::default()
                },
                None,
            ),
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
