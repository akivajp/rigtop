//! rigtop: system monitor (terminal UI).
//!
//! Without options it starts the TUI for this machine; `--host` (and
//! `[[hosts]]` in the configuration) adds remote machines over SSH, where
//! `rigtop --agent` streams the data, with an overview of all hosts.
//! `--dump-json` prints one snapshot to check what the collectors see, and
//! `--watch` runs a command when alerts appear on any host, without a screen.

#[cfg(target_os = "linux")]
mod collector;
mod tui;
mod watch;

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
    about = "System monitor for workstations, laptops and NAS (terminal UI), many hosts over SSH"
)]
struct Cli {
    /// Print one snapshot of this machine as JSON and exit.
    #[arg(long)]
    dump_json: bool,

    /// Sampling interval in milliseconds [default: 1000; for --watch,
    /// `[watch] interval` of the configuration]. Rates (CPU %, throughput,
    /// power) are averaged over this interval.
    #[arg(long, value_name = "MS")]
    interval_ms: Option<u64>,

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
    #[arg(long = "host", value_name = "DESTINATION", conflicts_with_all = ["agent", "dump_json", "test_alert"])]
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

    /// Show made-up data for three hosts instead of this machine and the
    /// configured hosts (to try the interface, or for screenshots).
    #[arg(long, conflicts_with_all = ["agent", "dump_json", "hosts", "test_alert"])]
    demo: bool,

    /// Watch this machine and the configured hosts without a screen, and run
    /// the `[watch] command` of the configuration when an alert appears or
    /// clears (e.g. to send it to your phone). Meant to run as a service.
    #[arg(long, conflicts_with_all = ["agent", "dump_json", "test_alert"])]
    watch: bool,

    /// Run the `[watch] command` once with a sample alert, to check that
    /// alerts arrive, and exit.
    #[arg(long, conflicts_with_all = ["agent", "dump_json"])]
    test_alert: bool,

    /// Linux, as root: install and start the systemd timer that collects
    /// SMART data and btrfs scrub results for rigtop every 10 minutes
    /// (smartctl needs root). Use the full path with sudo, e.g.
    /// `sudo "$(command -v rigtop)" --install-collector`.
    #[arg(long, conflicts_with_all = ["agent", "dump_json", "hosts", "uninstall_collector"])]
    install_collector: bool,

    /// Linux, as root: stop and remove that timer and the data it wrote.
    #[arg(long, conflicts_with_all = ["agent", "dump_json", "hosts"])]
    uninstall_collector: bool,

    /// Open the configuration file in $VISUAL or $EDITOR (else nano or vi;
    /// Notepad on Windows), created with every setting commented out if
    /// there is none, then check it.
    #[arg(long, conflicts_with_all = ["agent", "dump_json", "hosts", "watch", "test_alert"])]
    edit_config: bool,
}

fn main() -> Result<()> {
    // Taken first: once an update replaces the program, its path reads
    // "... (deleted)" on Linux.
    let exe = std::env::current_exe().ok();
    let cli = Cli::parse();
    let interval_override = cli.interval_ms.map(Duration::from_millis);
    let interval = interval_override.unwrap_or(Duration::from_secs(1));
    if interval < MIN_POLL_INTERVAL {
        bail!(
            "--interval-ms must be at least {} ms",
            MIN_POLL_INTERVAL.as_millis()
        );
    }

    if cli.install_collector || cli.uninstall_collector {
        #[cfg(target_os = "linux")]
        return if cli.install_collector {
            collector::install()
        } else {
            collector::uninstall()
        };
        #[cfg(not(target_os = "linux"))]
        bail!("the collector is for Linux (SMART data comes from smartctl run by systemd)");
    }

    let config_path = cli.config.clone().or_else(Config::default_path);
    if cli.edit_config {
        // Before loading it: the point may be to fix it.
        let path = config_path.context("this system has no place for a configuration file")?;
        return edit_config(&path);
    }
    let config = match &config_path {
        Some(path) => Config::load(path),
        None => Ok(Config::default()),
    }
    .context("failed to load the configuration")?;

    if cli.dump_json {
        return dump_json(&config, interval, cli.pretty);
    }
    if cli.test_alert {
        return watch::test_alert(&config);
    }
    if cli.agent {
        let source = LocalSource::with_config(&config);
        let stdin = io::BufReader::new(io::stdin());
        return run_agent(source, interval, stdin, io::stdout().lock()).context("agent failed");
    }
    let specs: tui::HostSpecs = if cli.demo {
        Box::new(rigtop_core::demo::host_specs)
    } else {
        let (hosts, remote_command) = (cli.hosts, cli.remote_command);
        Box::new(move |config| host_specs(config, &hosts, remote_command.as_deref()))
    };
    if cli.watch {
        return watch::run(config, config_path, specs, interval_override);
    }
    tui::run(config, config_path, interval, specs, exe)
}

/// `--edit-config`: opens the configuration file in an editor and says
/// whether the result loads.
fn edit_config(path: &std::path::Path) -> Result<()> {
    if rigtop_core::config::create_if_missing(path)
        .with_context(|| format!("cannot create {}", path.display()))?
    {
        println!(
            "created {} with every setting commented out",
            path.display()
        );
    }
    let editor = editor_command().context("no editor found: set VISUAL or EDITOR")?;
    let mut words = editor.split_whitespace();
    let program = words.next().context("VISUAL or EDITOR is empty")?;
    let status = std::process::Command::new(program)
        .args(words)
        .arg(path)
        .status()
        .with_context(|| format!("cannot start {program}"))?;
    if !status.success() {
        bail!("{program} ended with {status}");
    }
    Config::load(path).context("the configuration does not load")?;
    println!(
        "{} is fine; running rigtop instances apply it",
        path.display()
    );
    Ok(())
}

/// The editor to use: `$VISUAL`, `$EDITOR`, else Notepad on Windows, else
/// nano or vi if installed.
fn editor_command() -> Option<String> {
    for variable in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(variable)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
    }
    if cfg!(windows) {
        return Some("notepad".to_owned());
    }
    let path = std::env::var_os("PATH")?;
    ["nano", "vi"]
        .into_iter()
        .find(|name| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
        .map(str::to_owned)
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
