//! Terminal UI.

mod app;
mod graph;
mod ui;

use std::io::stdout;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent,
};
use ratatui::crossterm::execute;
use rigtop_core::remote::{RemoteSource, RemoteTarget};
use rigtop_core::{Config, DataSource, LocalSource};

use app::App;
use rigtop_core::worker::Worker;

/// How often to check for input while idle. Drawing only happens when
/// something changed, so this mainly bounds input latency.
const INPUT_POLL: Duration = Duration::from_millis(50);

/// Runs the TUI until the user quits, for this machine or, with `remote`,
/// for a host reached over SSH.
pub fn run(config: Config, interval: Duration, remote: Option<RemoteTarget>) -> Result<()> {
    let source_config = config.clone();
    let waiting = match &remote {
        Some(target) => format!("connecting to {}…", target.name),
        None => "collecting…".to_owned(),
    };
    let worker = Worker::spawn(
        move || -> Box<dyn DataSource> {
            match remote {
                Some(target) => Box::new(RemoteSource::new(target)),
                None => Box::new(LocalSource::with_config(&source_config)),
            }
        },
        interval,
    );
    let mut app = App::new(config, interval);
    app.waiting_message = waiting;

    // ratatui::init installs a panic hook that restores the terminal; mouse
    // capture is ours to undo, also when panicking.
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        previous_hook(info);
    }));

    let result = event_loop(&mut terminal, &mut app, &worker);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    worker: &Worker,
) -> Result<()> {
    let mut dirty = true;
    let mut status_visible = false;
    let mut drawn_second = 0;
    while !app.should_quit {
        while let Some(event) = worker.try_recv() {
            app.apply(event);
            dirty = true;
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
        if status_visible != app.status().is_some() {
            status_visible = app.status().is_some();
            dirty = true;
        }
        if dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            dirty = false;
        }
        if event::poll(INPUT_POLL)? {
            match event::read()? {
                TermEvent::Key(key) => {
                    if let Some(command) = app.handle_key(key) {
                        worker.send(command);
                    }
                    dirty = true;
                }
                TermEvent::Mouse(mouse) => {
                    app.handle_mouse(mouse);
                    dirty = true;
                }
                TermEvent::Resize(..) => dirty = true,
                _ => {}
            }
        }
    }
    Ok(())
}
