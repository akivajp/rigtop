//! rigtop: btop-inspired system monitor (terminal UI).
//!
//! The TUI itself is not implemented yet; for now the binary can print a
//! snapshot as JSON to check what the collectors see on a machine.

use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rigtop_core::{DataSource, LocalSource, MIN_POLL_INTERVAL};

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

    if cli.dump_json {
        return dump_json(interval, cli.pretty);
    }
    bail!("the terminal UI is not implemented yet; try --dump-json")
}

/// Takes a warm-up sample, waits one interval, and prints the second sample.
fn dump_json(interval: Duration, pretty: bool) -> Result<()> {
    let mut source = LocalSource::new();
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
