//! rigtop: btop-inspired system monitor (terminal UI).
//!
//! Without options it starts the TUI. `--dump-json` prints one snapshot to
//! check what the collectors see on a machine.

mod tui;

use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rigtop_core::{Config, DataSource, LocalSource, MIN_POLL_INTERVAL};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "btop-inspired system monitor with storage health and multi-host support"
)]
struct Cli {
    /// Print one snapshot of this machine as JSON and exit.
    #[arg(long)]
    dump_json: bool,

    /// Sampling interval in milliseconds. Rates (CPU %, throughput, power)
    /// are averaged over this interval.
    #[arg(long, default_value_t = 1000, value_name = "MS")]
    interval_ms: u64,

    /// Pretty-print JSON output.
    #[arg(long)]
    pretty: bool,

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

    if cli.dump_json {
        return dump_json(&config, interval, cli.pretty);
    }
    tui::run(config, interval)
}

/// Takes a warm-up sample, waits one interval, and prints the second sample.
fn dump_json(config: &Config, interval: Duration, pretty: bool) -> Result<()> {
    let mut source = LocalSource::with_config(config);
    if let Some(reason) = source.gpu_unavailable_reason() {
        eprintln!("note: NVIDIA GPUs unavailable: {reason}");
    }
    thread::sleep(interval);
    let snapshot = source.poll().context("failed to collect a snapshot")?;

    let mut out = io::stdout().lock();
    if pretty {
        serde_json::to_writer_pretty(&mut out, &snapshot)?;
    } else {
        serde_json::to_writer(&mut out, &snapshot)?;
    }
    writeln!(out)?;
    Ok(())
}
