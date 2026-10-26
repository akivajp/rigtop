//! rigtop-gui: btop-inspired native system monitor (egui).
//!
//! Panels can be resized by dragging their borders and rearranged by
//! dragging their titles.

// Release builds on Windows are GUI applications, so starting them from
// Explorer does not open a console window. See `console` for how `--help`
// and errors still reach a terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod alert_log;
mod app;
mod console;
mod hosts_dialog;
mod lhm_setup;
mod notify;
mod overview;
mod panels;
mod sensors_dialog;
mod widget_panel;
mod widgets;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
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

    /// Also monitor a remote host over SSH (repeatable; adds to the
    /// [[hosts]] of the configuration). Takes any destination `ssh` accepts
    /// (a ~/.ssh/config host, user@host, ...); key-based authentication is
    /// required, and the remote host needs rigtop installed.
    #[arg(long = "host", value_name = "DESTINATION")]
    hosts: Vec<String>,

    /// Command that starts the agent on hosts given with --host
    /// [default: "rigtop --agent"].
    #[arg(long, value_name = "COMMAND")]
    remote_command: Option<String>,

    /// Show made-up data for three hosts instead of this machine and the
    /// configured hosts (to try the interface, or for screenshots).
    #[arg(long)]
    demo: bool,

    /// Show a sample desktop notification at startup, to check that alert
    /// notifications reach the desktop.
    #[arg(long)]
    test_notification: bool,
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
    // Hosts added in the GUI are saved to the same file.
    let config_path = cli.config.clone().or_else(Config::default_path);
    let config = match &config_path {
        Some(path) => Config::load(path),
        None => Ok(Config::default()),
    }
    .context("failed to load the configuration")?;

    if cli.test_notification {
        if !notify::SUPPORTED {
            bail!("desktop notifications are not available on Windows yet");
        }
        notify::send_test();
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("rigtop")
        .with_app_id("rigtop")
        .with_inner_size([1400.0, 900.0])
        .with_min_inner_size([640.0, 400.0]);
    // The window icon (title bar, task bar, Alt+Tab).
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/rigtop.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "rigtop",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::RigtopApp::new(
                cc,
                config,
                config_path,
                interval,
                &cli.hosts,
                cli.remote_command.as_deref(),
                cli.demo,
            )))
        }),
    )
    .map_err(|err| anyhow!("failed to start the GUI: {err}"))
}
