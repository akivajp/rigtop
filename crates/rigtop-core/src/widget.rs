//! Widgets: small panels shown beside the monitor, for whatever else someone
//! wants on the same screen. A widget is either built in (a large clock) or
//! fed by any command, which rigtop runs periodically and whose output it
//! renders.
//!
//! A command prints either plain text (one row per line) or JSON:
//!
//! ```json
//! {"rows": [
//!   {"label": "5h", "value": "42% · resets 14:30", "gauge": 0.42, "level": "ok"},
//!   {"value": "rigtop", "heading": true},
//!   {"label": "busy", "value": "2 subagents", "level": "warn"}
//! ]}
//! ```
//!
//! Every field is optional; `text` is accepted for `value`. `level` is one
//! of `normal`, `dim`, `ok`, `warn`, `bad`; `gauge` is a fraction (0-1)
//! drawn as a bar. Commands run on the machine that displays them, through
//! the shell (`sh -c`, or `cmd /C` on Windows).

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::config::{BuiltinWidget, WidgetConfig};

/// Upper bound for a command's output.
const MAX_OUTPUT: u64 = 256 * 1024;
/// How often a sleeping runner checks whether it should stop.
const STOP_POLL: Duration = Duration::from_millis(100);

/// What a command reported.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct WidgetOutput {
    pub rows: Vec<WidgetRow>,
}

/// One line of a widget.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct WidgetRow {
    /// Short name on the left (may be empty).
    pub label: String,
    /// The text.
    #[serde(alias = "text")]
    pub value: String,
    /// A bar for a fraction (0-1), e.g. a quota used.
    pub gauge: Option<f32>,
    /// Color of the value.
    pub level: Level,
    /// A section title (e.g. a project name) rather than a reading.
    pub heading: bool,
}

/// How a row is colored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Normal,
    Dim,
    Ok,
    Warn,
    Bad,
}

/// JSON if the output looks like it, else one row per line.
pub fn parse_output(stdout: &str) -> WidgetOutput {
    let trimmed = stdout.trim();
    if trimmed.starts_with('{')
        && let Ok(output) = serde_json::from_str::<WidgetOutput>(trimmed)
    {
        return output;
    }
    WidgetOutput {
        rows: trimmed
            .lines()
            .map(|line| WidgetRow {
                value: line.trim_end().to_owned(),
                ..Default::default()
            })
            .collect(),
    }
}

/// The latest result of a command widget.
#[derive(Debug, Clone, Default)]
pub struct WidgetState {
    /// The last successful output, kept while later runs fail.
    pub output: Option<WidgetOutput>,
    /// Why the last run failed.
    pub error: Option<String>,
    /// When the last run finished.
    pub updated: Option<Instant>,
}

/// A configured widget, ready to draw.
pub struct Widget {
    pub name: String,
    pub source: WidgetSource,
}

/// Where a widget's content comes from.
pub enum WidgetSource {
    /// A large clock, drawn by the front-end.
    Clock,
    /// A command run in the background.
    Command {
        runner: CommandRunner,
        interval: Duration,
    },
    /// Fixed content (tests, previews).
    Fixed(WidgetOutput),
    /// A configuration mistake to show in place of the widget.
    Invalid(String),
}

impl Widget {
    /// The command's latest state, or the fixed content.
    pub fn state(&self) -> Option<WidgetState> {
        match &self.source {
            WidgetSource::Command { runner, .. } => Some(runner.state()),
            WidgetSource::Fixed(output) => Some(WidgetState {
                output: Some(output.clone()),
                error: None,
                updated: Some(Instant::now()),
            }),
            WidgetSource::Clock | WidgetSource::Invalid(_) => None,
        }
    }

    /// Whether the last update is older than three intervals.
    pub fn is_stale(&self, state: &WidgetState) -> bool {
        match (&self.source, state.updated) {
            (WidgetSource::Command { interval, .. }, Some(at)) => {
                at.elapsed() > *interval * 3 + Duration::from_secs(5)
            }
            _ => false,
        }
    }
}

/// Starts the widgets of the configuration; `notify` is called whenever a
/// command produced new content (to repaint).
pub fn start_widgets<F>(configs: &[WidgetConfig], notify: F) -> Vec<Widget>
where
    F: Fn() + Clone + Send + 'static,
{
    configs
        .iter()
        .map(|config| {
            let source = match (&config.builtin, &config.command) {
                (Some(BuiltinWidget::Clock), None) => WidgetSource::Clock,
                (None, Some(command)) => {
                    let interval = Duration::from_secs(config.interval_secs.max(1));
                    WidgetSource::Command {
                        runner: CommandRunner::spawn(
                            command.clone(),
                            interval,
                            Duration::from_secs(config.timeout_secs.max(1)),
                            notify.clone(),
                        ),
                        interval,
                    }
                }
                (Some(_), Some(_)) => {
                    WidgetSource::Invalid("set either `command` or `builtin`, not both".to_owned())
                }
                (None, None) => WidgetSource::Invalid("set `command` or `builtin`".to_owned()),
            };
            Widget {
                name: config.name.clone(),
                source,
            }
        })
        .collect()
}

/// Runs a command every interval on its own thread. Dropping it stops the
/// thread after the current run (without waiting for it).
pub struct CommandRunner {
    state: Arc<Mutex<WidgetState>>,
    stop: Arc<AtomicBool>,
}

impl CommandRunner {
    pub fn spawn<F>(command: String, interval: Duration, timeout: Duration, notify: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        let state = Arc::new(Mutex::new(WidgetState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, stopping) = (Arc::clone(&state), Arc::clone(&stop));
        let spawned = thread::Builder::new()
            .name("rigtop-widget".to_owned())
            .spawn(move || {
                while !stopping.load(Ordering::Relaxed) {
                    let result = run_command(&command, timeout);
                    {
                        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                        match result {
                            Ok(stdout) => {
                                state.output = Some(parse_output(&stdout));
                                state.error = None;
                            }
                            Err(error) => state.error = Some(error),
                        }
                        state.updated = Some(Instant::now());
                    }
                    notify();
                    let wake = Instant::now() + interval;
                    while Instant::now() < wake && !stopping.load(Ordering::Relaxed) {
                        thread::sleep(STOP_POLL);
                    }
                }
            });
        if let Err(err) = spawned {
            state.lock().unwrap_or_else(|e| e.into_inner()).error =
                Some(format!("cannot start a thread: {err}"));
        }
        Self { state, stop }
    }

    pub fn state(&self) -> WidgetState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for CommandRunner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Runs `command` through the shell, killing it after `timeout`. Returns its
/// output, or why it failed (the last line of its stderr if any).
pub fn run_command(command: &str, timeout: Duration) -> Result<String, String> {
    let mut process = if cfg!(windows) {
        let mut process = Command::new("cmd");
        process.args(["/C", command]);
        process
    } else {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    };
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up from the GUI.
        process.creation_flags(0x0800_0000);
    }
    let mut child = process
        .spawn()
        .map_err(|err| format!("cannot run: {err}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    // Read both pipes on threads so a chatty command cannot block on them.
    let out = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.take(MAX_OUTPUT).read_to_string(&mut text);
        text
    });
    let err = thread::spawn(move || {
        BufReader::new(stderr.take(MAX_OUTPUT))
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .last()
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(err.to_string()),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let last_error = err.join().ok().flatten();
    if status.success() {
        Ok(stdout)
    } else {
        Err(last_error.unwrap_or_else(|| format!("exited with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_and_plain_text() {
        let json = r#"{"rows":[
            {"label":"5h","value":"42%","gauge":0.42,"level":"ok"},
            {"text":"rigtop","heading":true},
            {}
        ]}"#;
        let output = parse_output(json);
        assert_eq!(output.rows.len(), 3);
        assert_eq!(output.rows[0].gauge, Some(0.42));
        assert_eq!(output.rows[0].level, Level::Ok);
        assert_eq!(output.rows[1].value, "rigtop", "`text` is `value`");
        assert!(output.rows[1].heading);

        let text = parse_output("USD/JPY 151.2\nEUR/JPY 163.9\n\n");
        assert_eq!(text.rows.len(), 2);
        assert_eq!(text.rows[1].value, "EUR/JPY 163.9");
        // Not valid JSON: shown as text rather than lost.
        assert_eq!(parse_output("{oops").rows[0].value, "{oops");
    }

    #[cfg(unix)]
    #[test]
    fn runs_commands_with_errors_and_timeouts() {
        let second = Duration::from_secs(1);
        assert_eq!(run_command("echo hello", second).unwrap().trim(), "hello");
        assert_eq!(
            run_command("echo first >&2; echo 'no data' >&2; exit 3", second),
            Err("no data".to_owned())
        );
        assert!(run_command("exit 4", second).unwrap_err().contains("exit"));
        let started = Instant::now();
        assert!(
            run_command("sleep 5", second)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn widgets_from_configuration() {
        let config =
            |name: &str, command: Option<&str>, builtin: Option<BuiltinWidget>| WidgetConfig {
                name: name.to_owned(),
                command: command.map(str::to_owned),
                builtin,
                ..Default::default()
            };
        let (tx, rx) = std::sync::mpsc::channel();
        let notify = move || {
            let _ = tx.send(());
        };
        let widgets = start_widgets(
            &[
                config("Clock", None, Some(BuiltinWidget::Clock)),
                config("Echo", Some(r#"echo '{"rows":[{"value":"hi"}]}'"#), None),
                config("Empty", None, None),
            ],
            notify,
        );
        assert!(matches!(widgets[0].source, WidgetSource::Clock));
        assert!(matches!(widgets[2].source, WidgetSource::Invalid(_)));
        rx.recv_timeout(Duration::from_secs(5)).expect("first run");
        let state = widgets[1].state().unwrap();
        assert_eq!(state.output.unwrap().rows[0].value, "hi");
        assert!(state.error.is_none());
    }
}
