//! Terminal UI.

mod app;
mod format;
mod graph;
mod ui;
mod worker;

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event as TermEvent};
use rigtop_core::{Config, LocalSource};

use app::App;
use worker::Worker;

/// How often to check for input while idle. Drawing only happens when
/// something changed, so this mainly bounds input latency.
const INPUT_POLL: Duration = Duration::from_millis(50);

/// Runs the TUI on the local machine until the user quits.
pub fn run(config: Config, interval: Duration) -> Result<()> {
    let source_config = config.clone();
    let worker = Worker::spawn(move || LocalSource::with_config(&source_config), interval);
    let mut app = App::new(config, interval);

    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &worker);
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
    while !app.should_quit {
        while let Some(event) = worker.try_recv() {
            app.apply(event);
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
                TermEvent::Resize(..) => dirty = true,
                _ => {}
            }
        }
    }
    Ok(())
}
