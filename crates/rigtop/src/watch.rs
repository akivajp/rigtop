//! `rigtop --watch`: watches every host without a screen and runs the
//! `[watch] command` when an alert appears or clears, so that a degraded
//! RAID or a host gone offline reaches you (ntfy, mail, chat...) while no
//! rigtop window is open.
//!
//! The hosts are the TUI's: this machine, `[[hosts]]` and `--host`. Changes
//! are printed with the time (for the journal when run as a systemd service)
//! and kept in their own alert history, so a restart does not send alerts
//! that were already sent.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use chrono::Local;
use rigtop_core::config::{ConfigWatcher, NotifyLevel, WatchConfig};
use rigtop_core::hosts::HostSpec;
use rigtop_core::view::{
    Alert, AlertChange, AlertEvent, AlertLog, Severity, alert_log_path, alerts_with, event_time,
};
use rigtop_core::widget::run_command_with_env;
use rigtop_core::worker::{Command, Event, Worker};
use rigtop_core::{Config, MIN_POLL_INTERVAL};

use crate::tui::{HostSpecs, spawn_worker, stop_workers};

/// Time between looks at the workers' events.
const TICK: Duration = Duration::from_millis(250);

/// Longest failure reason kept in an "unreachable" alert.
const MAX_REASON: usize = 120;

/// What the watch knows about one host.
#[derive(Debug)]
struct HostState {
    name: String,
    /// When the host last sent data (or when watching it started).
    last_seen: SystemTime,
    /// Why the latest poll failed, if it did.
    error: Option<String>,
    /// Alerts of the latest snapshot.
    alerts: Vec<Alert>,
    /// The alert raised while the host is unreachable, worded once so that
    /// it stays the same alert until the host is back.
    offline: Option<Alert>,
}

impl HostState {
    fn new(name: &str, now: SystemTime) -> Self {
        Self {
            name: name.to_owned(),
            last_seen: now,
            error: None,
            alerts: Vec::new(),
            offline: None,
        }
    }

    /// Takes the alerts of a new snapshot; the host is reachable again.
    fn on_snapshot(&mut self, alerts: Vec<Alert>, now: SystemTime) {
        self.alerts = alerts;
        self.last_seen = now;
        self.error = None;
        self.offline = None;
    }

    /// Raises the "unreachable" alert once the host has sent nothing for
    /// `offline_after`. Returns true when it did.
    fn check_offline(&mut self, now: SystemTime, offline_after: Duration) -> bool {
        if self.offline.is_some() || offline_after.is_zero() {
            return false;
        }
        let silent = now.duration_since(self.last_seen).unwrap_or_default();
        if silent < offline_after {
            return false;
        }
        let since_ms = millis(self.last_seen);
        let reason = match &self.error {
            Some(error) => short_reason(&self.name, error),
            None => "no data".to_owned(),
        };
        self.offline = Some(Alert {
            severity: Severity::Critical,
            text: format!("unreachable since {} ({reason})", event_time(since_ms, now)),
        });
        true
    }

    /// The host's alerts as they stand: those of its latest snapshot (kept
    /// while it is unreachable, since nothing says they cleared) and the
    /// "unreachable" alert.
    fn current_alerts(&self) -> Vec<Alert> {
        let mut alerts = self.alerts.clone();
        alerts.extend(self.offline.clone());
        alerts
    }
}

/// The part of a polling error worth keeping in an alert: "ssh: Could not
/// resolve hostname nas" out of "source disconnected: nas: ssh: Could not
/// resolve hostname nas (retrying in 5s)".
fn short_reason(host: &str, error: &str) -> String {
    let mut reason = error.strip_prefix("source disconnected: ").unwrap_or(error);
    reason = reason
        .strip_prefix(host)
        .and_then(|r| r.strip_prefix(": "))
        .unwrap_or(reason);
    if let Some(i) = reason.rfind(" (retrying in ") {
        reason = &reason[..i];
    }
    reason.chars().take(MAX_REASON).collect()
}

/// A watched host and its collector thread.
struct Watched {
    spec: HostSpec,
    worker: Worker,
    state: HostState,
}

impl Watched {
    fn start(spec: HostSpec, config: &Config, interval: Duration, now: SystemTime) -> Self {
        let worker = spawn_worker(&spec, config, interval);
        // Alerts do not need the process list: do not send it over SSH.
        worker.send(Command::SetProcessLimit(Some(0)));
        let state = HostState::new(&spec.name, now);
        Self {
            spec,
            worker,
            state,
        }
    }
}

/// Watches the hosts until the process is stopped.
pub fn run(
    config: Config,
    config_path: Option<PathBuf>,
    specs: HostSpecs,
    interval_override: Option<Duration>,
) -> Result<()> {
    let mut config = config;
    let mut watcher = config_path.map(ConfigWatcher::new);
    let interval_of = |watch: &WatchConfig| {
        interval_override
            .unwrap_or(watch.interval)
            .max(MIN_POLL_INTERVAL)
    };
    let mut interval = interval_of(&config.watch);
    let mut log = match alert_log_path("watch") {
        Some(path) => AlertLog::persistent(path),
        None => AlertLog::default(),
    };

    let now = SystemTime::now();
    let mut hosts: Vec<Watched> = specs(&config)
        .into_iter()
        .map(|spec| Watched::start(spec, &config, interval, now))
        .collect();
    announce(&config.watch, &hosts, interval);
    let sender = start_sender();

    loop {
        thread::sleep(TICK);
        let now = SystemTime::now();

        // An edited configuration applies without a restart.
        if let Some(result) = watcher.as_mut().and_then(ConfigWatcher::poll) {
            match result {
                Ok(new_config) => {
                    config = new_config;
                    interval = interval_of(&config.watch);
                    hosts = restart_hosts(hosts, specs(&config), &config, interval, now);
                    print_line("configuration reloaded");
                    announce(&config.watch, &hosts, interval);
                }
                Err(err) => print_line(&format!("configuration not reloaded: {err}")),
            }
        }

        for host in &mut hosts {
            let mut changed = false;
            while let Some(event) = host.worker.try_recv() {
                match event {
                    Event::Snapshot(snapshot) => {
                        host.state
                            .on_snapshot(alerts_with(&snapshot, &config.alerts), now);
                        changed = true;
                    }
                    Event::Error(err) => host.state.error = Some(err),
                    Event::SignalResult { .. } => {}
                }
            }
            changed |= host.state.check_offline(now, config.watch.offline_after);
            if !changed {
                continue;
            }
            let events = log.update(&host.state.name, &host.state.current_alerts(), millis(now));
            for event in events {
                let send = wanted(&event, &config.watch) && !config.watch.command.is_empty();
                print_line(&format!(
                    "{}{}",
                    message(&event),
                    if send { "" } else { "  (not sent)" }
                ));
                if send {
                    let job = (config.watch.command.clone(), config.watch.timeout, event);
                    let _ = sender.send(job);
                }
            }
        }
    }
}

/// Keeps the hosts whose specification did not change and starts the new
/// ones; the others stop in the background.
fn restart_hosts(
    mut old: Vec<Watched>,
    specs: Vec<HostSpec>,
    config: &Config,
    interval: Duration,
    now: SystemTime,
) -> Vec<Watched> {
    let mut hosts = Vec::with_capacity(specs.len());
    for spec in specs {
        match old.iter().position(|h| h.spec == spec) {
            Some(i) => {
                let host = old.swap_remove(i);
                host.worker.send(Command::SetInterval(interval));
                hosts.push(host);
            }
            None => hosts.push(Watched::start(spec, config, interval, now)),
        }
    }
    stop_workers(old.into_iter().map(|h| h.worker).collect());
    hosts
}

/// Prints what is watched and where the alerts go.
fn announce(watch: &WatchConfig, hosts: &[Watched], interval: Duration) {
    let names: Vec<&str> = hosts.iter().map(|h| h.spec.name.as_str()).collect();
    print_line(&format!(
        "watching {} host{} every {}s: {}",
        names.len(),
        if names.len() == 1 { "" } else { "s" },
        interval.as_secs_f32(),
        names.join(", ")
    ));
    if watch.command.is_empty() {
        print_line("no [watch] command in the configuration: changes are only printed");
    } else if watch.level == NotifyLevel::Off {
        print_line("[watch] level is \"off\": the command never runs");
    }
}

/// Whether `event` is one the command runs for.
fn wanted(event: &AlertEvent, watch: &WatchConfig) -> bool {
    let min = match watch.level {
        NotifyLevel::Off => return false,
        NotifyLevel::Warning => Severity::Warning,
        NotifyLevel::Critical => Severity::Critical,
    };
    event.alert.severity >= min && (event.change != AlertChange::Cleared || watch.cleared)
}

/// One line for the event: "[critical] nas: md0 (raid1) degraded",
/// "[cleared] nas: md0 (raid1) degraded".
fn message(event: &AlertEvent) -> String {
    let tag = match event.change {
        AlertChange::Raised | AlertChange::Notice => severity_name(event.alert.severity),
        AlertChange::Cleared => "cleared",
    };
    format!("[{tag}] {}: {}", event.host, event.alert.text)
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "critical",
        Severity::Warning => "warning",
    }
}

/// The environment the command gets for `event`.
fn command_env(event: &AlertEvent) -> Vec<(&'static str, String)> {
    let change = match event.change {
        AlertChange::Raised => "raised",
        AlertChange::Cleared => "cleared",
        AlertChange::Notice => "notice",
    };
    vec![
        ("RIGTOP_HOST", event.host.clone()),
        (
            "RIGTOP_SEVERITY",
            severity_name(event.alert.severity).to_owned(),
        ),
        ("RIGTOP_CHANGE", change.to_owned()),
        ("RIGTOP_ALERT", event.alert.text.clone()),
        ("RIGTOP_MESSAGE", message(event)),
    ]
}

/// One run of the command: the command as configured then, and the event.
type Job = (String, Duration, AlertEvent);

/// Starts the thread that runs the command, one event after another in the
/// order they happened (a slow command must not hold up watching), and
/// prints why a run failed.
fn start_sender() -> Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    thread::spawn(move || {
        for (command, timeout, event) in rx {
            if let Err(err) = run_for(&command, &event, timeout) {
                print_line(&format!("command failed for {}: {err}", message(&event)));
            }
        }
    });
    tx
}

fn run_for(command: &str, event: &AlertEvent, timeout: Duration) -> Result<String, String> {
    let env = command_env(event);
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    run_command_with_env(command, &env, timeout)
}

/// `rigtop --test-alert`: runs the command once with a sample alert.
pub fn test_alert(config: &Config) -> Result<()> {
    let watch = &config.watch;
    if watch.command.is_empty() {
        bail!("no command to test: set `command` under [watch] in the configuration");
    }
    let event = AlertEvent {
        at_ms: millis(SystemTime::now()),
        host: "test".to_owned(),
        change: AlertChange::Raised,
        alert: Alert {
            severity: Severity::Warning,
            text: "Test alert: rigtop --watch sends alerts like this.".to_owned(),
        },
    };
    match run_for(&watch.command, &event, watch.timeout) {
        Ok(output) => {
            print!("{output}");
            println!("ran: {}", watch.command);
            Ok(())
        }
        Err(err) => bail!("the command failed: {err}"),
    }
}

/// Prints `text` after the local date and time.
fn print_line(text: &str) {
    println!("{} {text}", Local::now().format("%Y-%m-%d %H:%M:%S"));
}

/// Milliseconds since the Unix epoch.
fn millis(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(severity: Severity, text: &str) -> Alert {
        Alert {
            severity,
            text: text.to_owned(),
        }
    }

    fn event(change: AlertChange, severity: Severity) -> AlertEvent {
        AlertEvent {
            at_ms: 0,
            host: "nas".to_owned(),
            change,
            alert: alert(severity, "md0 (raid1) degraded"),
        }
    }

    #[test]
    fn a_silent_host_becomes_one_unreachable_alert() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let after = Duration::from_secs(180);
        let mut host = HostState::new("nas", start);
        host.on_snapshot(vec![alert(Severity::Warning, "/srv 92% full")], start);
        host.error = Some(
            "source disconnected: nas: ssh: connect to host nas port 22: No route to host \
             (retrying in 3s)"
                .to_owned(),
        );

        assert!(!host.check_offline(start + Duration::from_secs(179), after));
        assert!(host.check_offline(start + Duration::from_secs(180), after));
        // Raised once; the alerts it had are kept beside it.
        assert!(!host.check_offline(start + Duration::from_secs(600), after));
        let alerts = host.current_alerts();
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[1].severity, Severity::Critical);
        assert!(alerts[1].text.starts_with("unreachable since "));
        assert!(
            alerts[1]
                .text
                .ends_with("(ssh: connect to host nas port 22: No route to host)")
        );

        // Data again: back to the snapshot's alerts.
        host.on_snapshot(Vec::new(), start + Duration::from_secs(700));
        assert!(host.current_alerts().is_empty());

        // 0 turns the check off.
        let mut host = HostState::new("nas", start);
        assert!(!host.check_offline(start + Duration::from_secs(86_400), Duration::ZERO));
    }

    #[test]
    fn level_and_cleared_pick_the_events_sent() {
        let mut watch = WatchConfig::default();
        let critical = event(AlertChange::Raised, Severity::Critical);
        let warning = event(AlertChange::Raised, Severity::Warning);
        let cleared = event(AlertChange::Cleared, Severity::Critical);
        assert!(wanted(&critical, &watch));
        assert!(!wanted(&warning, &watch));
        assert!(wanted(&cleared, &watch));

        watch.level = NotifyLevel::Warning;
        watch.cleared = false;
        assert!(wanted(&warning, &watch));
        assert!(!wanted(&cleared, &watch));

        watch.level = NotifyLevel::Off;
        assert!(!wanted(&critical, &watch));
    }

    #[test]
    fn message_and_environment() {
        let raised = event(AlertChange::Raised, Severity::Critical);
        assert_eq!(message(&raised), "[critical] nas: md0 (raid1) degraded");
        let cleared = event(AlertChange::Cleared, Severity::Critical);
        assert_eq!(message(&cleared), "[cleared] nas: md0 (raid1) degraded");
        let env = command_env(&cleared);
        let get = |key: &str| env.iter().find(|(k, _)| *k == key).unwrap().1.clone();
        assert_eq!(get("RIGTOP_HOST"), "nas");
        assert_eq!(get("RIGTOP_SEVERITY"), "critical");
        assert_eq!(get("RIGTOP_CHANGE"), "cleared");
        assert_eq!(get("RIGTOP_ALERT"), "md0 (raid1) degraded");
    }

    #[cfg(unix)]
    #[test]
    fn the_command_gets_the_alert_in_its_environment() {
        let raised = event(AlertChange::Raised, Severity::Critical);
        let output = run_for(
            "echo \"$RIGTOP_CHANGE $RIGTOP_MESSAGE\"",
            &raised,
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.trim(), "raised [critical] nas: md0 (raid1) degraded");
    }
}
