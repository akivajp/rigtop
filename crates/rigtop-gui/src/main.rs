//! rigtop-gui: btop-inspired native system monitor (egui).
//!
//! Panels can be resized by dragging their borders and rearranged by
//! dragging their titles.

// Release builds on Windows are GUI applications, so starting them from
// Explorer does not open a console window. See `console` for how `--help`
// and errors still reach a terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod console;
mod panels;
mod widgets;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use rigtop_core::remote::{DEFAULT_REMOTE_COMMAND, RemoteTarget};
use rigtop_core::{Config, MIN_POLL_INTERVAL};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "btop-inspired native GUI system monitor with storage health"
)]
struct Cli {
    /// Sampling interval in milliseconds.
    #[arg(long, default_value_t = 1000, value_name = "MS")]
    interval_ms: u64,

    /// Configuration file [default: <config dir>/rigtop/config.toml].
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Monitor a remote host over SSH instead of this machine. Takes any
    /// destination `ssh` accepts (a ~/.ssh/config host, user@host, ...);
    /// key-based authentication is required. The remote host needs rigtop
    /// installed.
    #[arg(long, value_name = "DESTINATION")]
    host: Option<String>,

    /// Command that starts the agent on the remote host (with --host).
    #[arg(long, value_name = "COMMAND", default_value = DEFAULT_REMOTE_COMMAND, requires = "host")]
    remote_command: String,
}

fn main() -> std::process::ExitCode {
    // Must happen before clap may print `--help` or a usage error.
    console::attach_parent();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            console::report_error(&format!("{err:#}"));
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let interval = Duration::from_millis(cli.interval_ms);
    if interval < MIN_POLL_INTERVAL {
        bail!(
            "--interval-ms must be at least {} ms",
            MIN_POLL_INTERVAL.as_millis()
        );
    }
    let config = match &cli.config {
        Some(path) => Config::load(path),
        None => Config::load_default(),
    }
    .context("failed to load the configuration")?;

    let remote = cli
        .host
        .as_deref()
        .map(|host| RemoteTarget::ssh(host, &cli.remote_command));
    let title = match &remote {
        Some(target) => format!("rigtop - {}", target.name),
        None => "rigtop".to_owned(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id("rigtop")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rigtop",
        options,
        Box::new(move |cc| Ok(Box::new(app::RigtopApp::new(cc, config, interval, remote)))),
    )
    .map_err(|err| anyhow!("failed to start the GUI: {err}"))
}
