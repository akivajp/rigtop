//! rigtop-gui: btop-inspired native system monitor (egui).
//!
//! Panels can be resized by dragging their borders and rearranged by
//! dragging their titles.

mod app;
mod panels;
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
}

fn main() -> Result<()> {
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rigtop")
            .with_app_id("rigtop")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rigtop",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::RigtopApp::new(
                &cc.egui_ctx,
                config,
                interval,
            )))
        }),
    )
    .map_err(|err| anyhow!("failed to start the GUI: {err}"))
}
