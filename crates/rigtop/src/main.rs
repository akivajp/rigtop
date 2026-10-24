//! rigtop: btop-inspired system monitor (terminal UI).
//!
//! Without options it starts the TUI for this machine; `--host` (and
//! `[[hosts]]` in the configuration) adds remote machines over SSH, where
//! `rigtop --agent` streams the data, with an overview of all hosts.
//! `--dump-json` prints one snapshot to check what the collectors see.

mod tui;

use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rigtop_core::agent::run_agent;
use rigtop_core::hosts::host_specs;
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

    /// Also monitor a remote host over SSH (repeatable; adds to the
    /// [[hosts]] of the configuration). Takes any destination `ssh` accepts
    /// (a ~/.ssh/config host, user@host, ...); key-based authentication is
    /// required, and the remote host needs rigtop installed.
    #[arg(long = "host", value_name = "DESTINATION", conflicts_with_all = ["agent", "dump_json"])]
    hosts: Vec<String>,

    /// Command that starts the agent on hosts given with --host
    /// [default: "rigtop --agent"].
    #[arg(long, value_name = "COMMAND")]
    remote_command: Option<String>,

    /// Run as a remote agent: write snapshots to stdout as JSON lines and
    /// read commands from stdin, until stdin is closed. Started by
    /// `rigtop --host` / `rigtop-gui --host` through SSH.
    #[arg(long, conflicts_with = "dump_json")]
    agent: bool,
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
    if cli.agent {
        let source = LocalSource::with_config(&config);
        let stdin = io::BufReader::new(io::stdin());
        return run_agent(source, interval, stdin, io::stdout().lock()).context("agent failed");
    }
    let specs = host_specs(&config, &cli.hosts, cli.remote_command.as_deref());
    tui::run(config, interval, specs)
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
