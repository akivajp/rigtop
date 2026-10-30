//! The agent protocol for remote monitoring, and the agent loop itself.
//!
//! A client runs `ssh host rigtop --agent`; the agent writes snapshots to
//! stdout and reads commands from stdin. Both directions use JSON Lines: one
//! JSON object per line, tagged by `"type"`:
//!
//! ```text
//! agent  -> client  {"type":"hello","protocol":1,"version":"0.1.0","features":["watch_processes"]}
//! agent  -> client  {"type":"snapshot","snapshot":{...}}
//! client -> agent   {"type":"set_interval","ms":2000}
//! client -> agent   {"type":"set_process_limit","limit":10}
//! client -> agent   {"type":"watch_processes","pids":[1234]}
//! client -> agent   {"type":"signal","pid":1234,"signal":"Terminate"}
//! agent  -> client  {"type":"signal_result","pid":1234,"signal":"Terminate","error":null}
//! ```
//!
//! The agent exits when stdin reaches end of file, so it never outlives the
//! SSH connection that started it.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::snapshot::Snapshot;
use crate::source::{DataSource, MIN_POLL_INTERVAL, ProcessSignal};

/// Version of the wire format. Bumped on incompatible changes; adding
/// fields to snapshots does not need a bump (all snapshot types default
/// missing fields).
pub const PROTOCOL_VERSION: u32 = 1;

/// [`AgentMessage::Hello`] feature: the agent understands
/// [`ClientMessage::WatchProcesses`] (0.19 and later). Older agents answer
/// commands they do not know with an error, so clients check first.
pub const FEATURE_WATCH_PROCESSES: &str = "watch_processes";

/// The agent finds processes by name for [`ClientMessage::WatchProcesses`]
/// (`names`); agents before 0.21 ignore them.
pub const FEATURE_WATCH_PROCESS_NAMES: &str = "watch_process_names";

/// Messages from the agent to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    /// First message after start-up.
    Hello {
        /// [`PROTOCOL_VERSION`] of the agent.
        protocol: u32,
        /// rigtop version of the agent.
        version: String,
        /// Commands added since protocol 1 that the agent understands
        /// (e.g. [`FEATURE_WATCH_PROCESSES`]); empty before 0.19.
        #[serde(default)]
        features: Vec<String>,
    },
    /// A new measurement.
    Snapshot {
        /// The measurement (processes possibly limited, see
        /// [`ClientMessage::SetProcessLimit`]).
        snapshot: Box<Snapshot>,
    },
    /// Outcome of a [`ClientMessage::Signal`].
    SignalResult {
        pid: u32,
        signal: ProcessSignal,
        /// `None` on success.
        error: Option<String>,
    },
    /// A problem the agent wants to report without exiting.
    Error { message: String },
}

/// Messages from the client to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Change the sampling interval.
    SetInterval { ms: u64 },
    /// Send only the top `limit` processes by CPU (`None`: all). Used for
    /// hosts shown in an overview rather than in detail.
    SetProcessLimit { limit: Option<usize> },
    /// Always send these processes, and list them in
    /// [`Snapshot::watched_pids`], so the client can tell when they exit.
    /// Only for agents with [`FEATURE_WATCH_PROCESSES`]. `names`: also
    /// watch every process they match (see
    /// [`crate::DataSource::watch_process_names`]), for agents with
    /// [`FEATURE_WATCH_PROCESS_NAMES`].
    WatchProcesses {
        pids: Vec<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        names: Vec<String>,
    },
    /// Send a signal to a process on the agent's host.
    Signal { pid: u32, signal: ProcessSignal },
}

/// Writes one message as a JSON line and flushes it.
pub fn write_message<W: Write, M: Serialize>(writer: &mut W, message: &M) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// What the stdin reader thread hands to the agent loop.
enum Input {
    Command(ClientMessage),
    Invalid(String),
    Closed,
}

/// Runs the agent until `input` is closed or `output` fails.
///
/// `source` is polled every `interval` (changeable by the client); each
/// snapshot is written to `output`. Commands are read from `input` on a
/// separate thread.
pub fn run_agent<S, R, W>(
    mut source: S,
    mut interval: Duration,
    input: R,
    mut output: W,
) -> io::Result<()>
where
    S: DataSource,
    R: BufRead + Send + 'static,
    W: Write,
{
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("rigtop-agent-input".to_owned())
        .spawn(move || {
            for line in input.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let message = match serde_json::from_str::<ClientMessage>(&line) {
                    Ok(command) => Input::Command(command),
                    Err(err) => Input::Invalid(format!("invalid command {line:?}: {err}")),
                };
                if tx.send(message).is_err() {
                    return;
                }
            }
            let _ = tx.send(Input::Closed);
        })?;

    let hello = AgentMessage::Hello {
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        features: vec![
            FEATURE_WATCH_PROCESSES.to_owned(),
            FEATURE_WATCH_PROCESS_NAMES.to_owned(),
        ],
    };
    // A failed write means the client went away: stop quietly.
    if write_message(&mut output, &hello).is_err() {
        return Ok(());
    }

    let mut process_limit: Option<usize> = None;
    let mut next_poll = Instant::now() + MIN_POLL_INTERVAL;
    loop {
        let timeout = next_poll.saturating_duration_since(Instant::now());
        let reply = match rx.recv_timeout(timeout) {
            Ok(Input::Command(ClientMessage::SetInterval { ms })) => {
                interval = Duration::from_millis(ms).max(MIN_POLL_INTERVAL);
                next_poll = Instant::now() + interval;
                None
            }
            Ok(Input::Command(ClientMessage::SetProcessLimit { limit })) => {
                process_limit = limit;
                // The source can skip work only the full list needs.
                source.set_process_limit(limit);
                None
            }
            Ok(Input::Command(ClientMessage::WatchProcesses { pids, names })) => {
                source.watch_processes(pids);
                source.watch_process_names(names);
                None
            }
            Ok(Input::Command(ClientMessage::Signal { pid, signal })) => {
                let error = source
                    .signal_process(pid, signal)
                    .err()
                    .map(|e| e.to_string());
                Some(AgentMessage::SignalResult { pid, signal, error })
            }
            Ok(Input::Invalid(message)) => Some(AgentMessage::Error { message }),
            Ok(Input::Closed) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {
                let message = match source.poll() {
                    Ok(mut snapshot) => {
                        if let Some(limit) = process_limit {
                            snapshot.truncate_processes(limit);
                        }
                        AgentMessage::Snapshot {
                            snapshot: Box::new(snapshot),
                        }
                    }
                    Err(err) => AgentMessage::Error {
                        message: err.to_string(),
                    },
                };
                next_poll = (next_poll + interval).max(Instant::now() + MIN_POLL_INTERVAL);
                Some(message)
            }
        };
        if let Some(message) = reply
            && write_message(&mut output, &message).is_err()
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::ProcessInfo;
    use crate::source::SourceError;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeSource {
        watched_pids: Vec<u32>,
    }

    impl DataSource for FakeSource {
        fn poll(&mut self) -> Result<Snapshot, SourceError> {
            Ok(Snapshot {
                watched_pids: self.watched_pids.clone(),
                processes: (1..=20)
                    .map(|pid| ProcessInfo {
                        pid,
                        cpu_percent: pid as f32,
                        ..Default::default()
                    })
                    .collect(),
                process_count: 20,
                ..Default::default()
            })
        }

        fn signal_process(&mut self, pid: u32, _signal: ProcessSignal) -> Result<(), SourceError> {
            if pid == 1 {
                Err(SourceError::Disconnected("denied".to_owned()))
            } else {
                Ok(())
            }
        }

        fn watch_processes(&mut self, pids: Vec<u32>) {
            self.watched_pids = pids;
        }
    }

    /// A writer whose contents can be read after the agent has finished.
    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn messages_round_trip_as_tagged_json() {
        let command = ClientMessage::Signal {
            pid: 42,
            signal: ProcessSignal::Kill,
        };
        let line = serde_json::to_string(&command).unwrap();
        assert_eq!(line, r#"{"type":"signal","pid":42,"signal":"Kill"}"#);
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&line).unwrap(),
            command
        );
        let limit: ClientMessage =
            serde_json::from_str(r#"{"type":"set_process_limit","limit":null}"#).unwrap();
        assert_eq!(limit, ClientMessage::SetProcessLimit { limit: None });

        // Names are left out when empty, as 0.19 and 0.20 clients send it.
        let watch = ClientMessage::WatchProcesses {
            pids: vec![7],
            names: Vec::new(),
        };
        let line = serde_json::to_string(&watch).unwrap();
        assert_eq!(line, r#"{"type":"watch_processes","pids":[7]}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&line).unwrap(), watch);
        let named: ClientMessage =
            serde_json::from_str(r#"{"type":"watch_processes","pids":[],"names":["rsync"]}"#)
                .unwrap();
        assert_eq!(
            named,
            ClientMessage::WatchProcesses {
                pids: Vec::new(),
                names: vec!["rsync".to_owned()]
            }
        );
    }

    #[test]
    fn agent_answers_commands_and_stops_at_end_of_input() {
        // Commands are processed before the first poll (which waits
        // MIN_POLL_INTERVAL); end of input stops the agent.
        let input = Cursor::new(
            "{\"type\":\"set_process_limit\",\"limit\":3}\n\
             {\"type\":\"signal\",\"pid\":1,\"signal\":\"Terminate\"}\n\
             not json\n",
        );
        let output = SharedBuffer::default();
        run_agent(
            FakeSource::default(),
            Duration::from_secs(1),
            input,
            output.clone(),
        )
        .unwrap();

        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        let messages: Vec<AgentMessage> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(
            &messages[0],
            AgentMessage::Hello {
                protocol: PROTOCOL_VERSION,
                features,
                ..
            } if features.iter().any(|f| f == FEATURE_WATCH_PROCESSES)
        ));
        assert!(matches!(
            &messages[1],
            AgentMessage::SignalResult { pid: 1, error: Some(e), .. } if e.contains("denied")
        ));
        assert!(
            matches!(&messages[2], AgentMessage::Error { message } if message.contains("invalid command"))
        );
        assert_eq!(
            messages.len(),
            3,
            "no snapshot before the first interval: {text}"
        );
    }

    #[test]
    fn snapshots_respect_the_process_limit() {
        // Keep the input open (a pipe) until a snapshot has arrived.
        let (reader, mut writer) = std::io::pipe().unwrap();
        let output = SharedBuffer::default();
        let out = output.clone();
        let agent = thread::spawn(move || {
            run_agent(
                FakeSource::default(),
                MIN_POLL_INTERVAL,
                io::BufReader::new(reader),
                out,
            )
        });
        writeln!(writer, r#"{{"type":"set_process_limit","limit":3}}"#).unwrap();
        // pid 2 uses little CPU but is watched: sent anyway.
        writeln!(writer, r#"{{"type":"watch_processes","pids":[2]}}"#).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
            let found = text.lines().find_map(|l| match serde_json::from_str(l) {
                Ok(AgentMessage::Snapshot { snapshot }) => Some(snapshot),
                _ => None,
            });
            if let Some(snapshot) = found {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "no snapshot: {text}");
            thread::sleep(Duration::from_millis(20));
        };
        let pids: Vec<u32> = snapshot.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, [20, 19, 18, 2], "top CPU first, then watched ones");
        assert_eq!(snapshot.watched_pids, [2]);
        assert_eq!(snapshot.process_count, 20);
        drop(writer);
        agent.join().unwrap().unwrap();
    }
}
