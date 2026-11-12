//! A [`DataSource`] for a remote host, reached by running `rigtop --agent`
//! through the system's `ssh` (OpenSSH; `ssh.exe` on Windows).
//!
//! Using the system client means `~/.ssh/config`, keys, agents and
//! `ProxyJump` all work as on the command line. Authentication must not
//! need a prompt (`BatchMode`), since there is no terminal to ask on.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::agent::{
    AgentMessage, ClientMessage, FEATURE_PING, FEATURE_PROCESS_CONTROL, FEATURE_PUBLIC_IP,
    FEATURE_WATCH_PROCESSES, PROTOCOL_VERSION, write_message,
};
use crate::public_ip::PublicIpLookup;
use crate::snapshot::Snapshot;
use crate::source::{DataSource, ProcessSignal, SourceError};

/// Command run on the remote host by default.
pub const DEFAULT_REMOTE_COMMAND: &str = "rigtop --agent";
/// Tried in order when the default command is not found: the install
/// script puts rigtop in `~/.local/bin` and `cargo install` in
/// `~/.cargo/bin`, which non-interactive SSH sessions often lack on their
/// `PATH`. Release binaries come first: they update without building.
pub const FALLBACK_REMOTE_COMMANDS: [&str; 2] =
    ["~/.local/bin/rigtop --agent", "~/.cargo/bin/rigtop --agent"];
/// Tried in order when a Windows host does not find the default command:
/// the SSH sessions of Windows' OpenSSH server get the system's `PATH`
/// only, without the user's, where the Microsoft Store (app execution
/// aliases) and Scoop (shims) put rigtop. In cmd.exe's syntax, the default
/// shell of that server.
pub const WINDOWS_FALLBACK_REMOTE_COMMANDS: [&str; 2] = [
    r#""%LOCALAPPDATA%\Microsoft\WindowsApps\rigtop.exe" --agent"#,
    r#""%USERPROFILE%\scoop\shims\rigtop.exe" --agent"#,
];
/// Exit status of POSIX shells for "command not found".
const NOT_FOUND_STATUS: i32 = 127;
/// Exit status of cmd.exe (and PowerShell) for a command that is not found.
const WINDOWS_NOT_FOUND_STATUS: i32 = 1;

/// Whether an agent that exited was not found, and by which kind of shell
/// (which decides the places to try next).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotFound {
    /// Found, or it failed for another reason.
    No,
    /// A POSIX shell did not find it.
    Posix,
    /// cmd.exe (or PowerShell) did not find it.
    Windows,
}

/// Minimum time between connection attempts after a failure.
const RECONNECT_DELAY: Duration = Duration::from_secs(5);
/// How long to wait for the first snapshot of a new connection (SSH
/// handshake plus the agent's warm-up sample).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to wait for the agent to answer a signal request.
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(10);
/// stderr lines kept to explain failures.
const STDERR_LINES: usize = 20;

/// Time between two pings of the agent (see [`AgentMessage::Pong`]).
const PING_EVERY: Duration = Duration::from_secs(5);

/// A ping unanswered this long is given up and sent again.
const PING_TIMEOUT: Duration = Duration::from_secs(30);

/// How to start the agent: a program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTarget {
    /// Short name for messages (e.g. the SSH destination).
    pub name: String,
    /// Program to run (normally `ssh`).
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Arguments to try in order when the remote command is not found
    /// (for the default command: [`FALLBACK_REMOTE_COMMANDS`]).
    pub fallback_args: Vec<Vec<String>>,
    /// The same when a Windows host does not find it (for the default
    /// command: [`WINDOWS_FALLBACK_REMOTE_COMMANDS`]).
    pub windows_fallback_args: Vec<Vec<String>>,
}

impl RemoteTarget {
    /// Runs `remote_command` on `destination` (anything `ssh` accepts:
    /// a `~/.ssh/config` host, `user@host`, ...).
    pub fn ssh(destination: &str, remote_command: &str) -> Self {
        let options = [
            // No password or host key prompts: fail instead of hanging.
            "BatchMode=yes",
            "ConnectTimeout=10",
            // Notice dead connections within ~45 s.
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
        ];
        let mut args = vec!["-T".to_owned()];
        for option in options {
            args.push("-o".to_owned());
            args.push(option.to_owned());
        }
        // `--` keeps a destination starting with "-" from being an option.
        args.push("--".to_owned());
        args.push(destination.to_owned());
        args.push(remote_command.to_owned());
        // Only the default command falls back; one set by the user is
        // taken as it is.
        let fallbacks = |commands: &[&str]| -> Vec<Vec<String>> {
            if remote_command != DEFAULT_REMOTE_COMMAND {
                return Vec::new();
            }
            commands
                .iter()
                .map(|command| {
                    let mut fallback = args.clone();
                    *fallback.last_mut().expect("command pushed above") = (*command).to_owned();
                    fallback
                })
                .collect()
        };
        let fallback_args = fallbacks(&FALLBACK_REMOTE_COMMANDS);
        let windows_fallback_args = fallbacks(&WINDOWS_FALLBACK_REMOTE_COMMANDS);
        Self {
            name: destination.to_owned(),
            program: "ssh".to_owned(),
            args,
            fallback_args,
            windows_fallback_args,
        }
    }
}

/// What the reader thread passes on.
enum Incoming {
    Message(AgentMessage),
    /// A [`AgentMessage::Pong`], with when it arrived (the reader sees it
    /// at once; the source only reads on its next poll).
    Pong {
        id: u64,
        at: Instant,
    },
    /// stdout closed: the agent or ssh exited.
    Closed,
}

/// One running agent process.
struct Connection {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Incoming>,
    /// Last lines of the process's stderr (ssh errors, "command not found").
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// Whether a snapshot has arrived on this connection yet.
    established: bool,
}

impl Connection {
    fn spawn(target: &RemoteTarget, args: &[String]) -> Result<Self, SourceError> {
        let mut command = Command::new(&target.program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: a GUI must not flash a console for ssh.exe.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|err| {
            SourceError::Disconnected(format!("cannot start {}: {err}", target.program))
        })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");

        let (tx, incoming) = mpsc::channel();
        thread::Builder::new()
            .name("rigtop-remote-reader".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    // Lines that are not protocol messages (e.g. a login
                    // banner printed by a shell profile) are skipped.
                    let incoming = match serde_json::from_str::<AgentMessage>(&line) {
                        Ok(AgentMessage::Pong { id }) => Incoming::Pong {
                            id,
                            at: Instant::now(),
                        },
                        Ok(message) => Incoming::Message(message),
                        Err(_) => continue,
                    };
                    if tx.send(incoming).is_err() {
                        return;
                    }
                }
                let _ = tx.send(Incoming::Closed);
            })
            .map_err(SourceError::Io)?;

        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let lines = Arc::clone(&stderr);
        thread::Builder::new()
            .name("rigtop-remote-stderr".to_owned())
            .spawn(move || {
                // Bytes, not `lines()`: cmd.exe answers in the system's code
                // page (not UTF-8 on, say, a Japanese Windows), and such a
                // line must neither end the reading nor be lost.
                let mut reader = BufReader::new(stderr_pipe);
                let mut bytes = Vec::new();
                loop {
                    bytes.clear();
                    match reader.read_until(b'\n', &mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let line = String::from_utf8_lossy(&bytes)
                        .trim_end_matches(['\r', '\n'])
                        .to_owned();
                    let mut lines = lines.lock().unwrap_or_else(|e| e.into_inner());
                    if lines.len() == STDERR_LINES {
                        lines.pop_front();
                    }
                    lines.push_back(line);
                }
            })
            .map_err(SourceError::Io)?;

        Ok(Self {
            child,
            stdin,
            incoming,
            stderr,
            established: false,
        })
    }

    fn send(&mut self, message: &ClientMessage) -> Result<(), SourceError> {
        write_message(&mut self.stdin, message).map_err(SourceError::Io)
    }

    /// Explanation for a lost connection, from the process's stderr, and
    /// whether the remote command was not found. `windows_attempt` says
    /// that one of the Windows places was tried, whose "not found" has
    /// nothing but its exit status to go by.
    fn failure(&mut self, windows_attempt: bool) -> (String, NotFound) {
        // Give the stderr thread a moment to collect the final lines.
        thread::sleep(Duration::from_millis(100));
        let status = self.child.try_wait().ok().flatten();
        let lines = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        let detail = lines
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .cloned()
            .unwrap_or_default();
        // ssh passes the remote exit status on (its own failures are 255).
        let code = status.and_then(|s| s.code());
        // cmd.exe names the command in quotes at the start of its message
        // ("'rigtop' is not recognized as ..."), whatever the language; the
        // rest may be in a code page that is not UTF-8.
        let names_rigtop = lines
            .iter()
            .any(|l| l.contains("'rigtop'") || l.contains("\"rigtop\""));
        let not_found = if code == Some(NOT_FOUND_STATUS) || detail.contains("command not found") {
            NotFound::Posix
        } else if code == Some(WINDOWS_NOT_FOUND_STATUS)
            && (windows_attempt || names_rigtop || detail.contains("not recognized"))
        {
            NotFound::Windows
        } else {
            NotFound::No
        };
        let reason = match (detail.is_empty(), status) {
            (false, _) => detail,
            (true, Some(status)) => format!("connection closed ({status})"),
            (true, None) => "connection closed".to_owned(),
        };
        (reason, not_found)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing stdin makes the agent exit; killing ssh ends the session
        // right away instead of waiting for it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Snapshots from `rigtop --agent` on another machine.
pub struct RemoteSource {
    target: RemoteTarget,
    connection: Option<Connection>,
    last_attempt: Option<Instant>,
    interval: Duration,
    process_limit: Option<usize>,
    /// See [`DataSource::watch_processes`].
    watched_pids: Vec<u32>,
    /// See [`DataSource::watch_process_names`].
    watched_names: Vec<String>,
    /// The connected agent understands [`ClientMessage::WatchProcesses`].
    agent_watches: bool,
    /// The connected agent can pause, resume and renice processes.
    agent_controls: bool,
    /// The connected agent answers pings.
    agent_pings: bool,
    /// See [`DataSource::set_public_ip`].
    public_ip: Option<PublicIpLookup>,
    /// The connected agent looks up public addresses.
    agent_public_ip: bool,
    /// The ping waiting for its answer: its id and when it was sent.
    ping: Option<(u64, Instant)>,
    /// When the last ping went out, and the round trip it measured.
    last_ping: Option<Instant>,
    link_rtt: Option<Duration>,
    /// Snapshots received while waiting for something else (signal results).
    pending: VecDeque<Snapshot>,
    /// rigtop version from the agent's greeting.
    agent_version: String,
    /// Why the last connection failed, repeated while waiting to retry.
    last_error: Option<String>,
    /// Which command starts the agent: 0 for the configured one, then the
    /// fallbacks in order.
    attempt: usize,
    /// The host answered like Windows: the fallbacks are the Windows ones.
    windows: bool,
    /// The command was not found and the next one should be tried at once.
    retry_now: bool,
}

impl RemoteSource {
    /// Creates a source; the connection is made on the first poll.
    pub fn new(target: RemoteTarget) -> Self {
        Self {
            target,
            connection: None,
            last_attempt: None,
            interval: Duration::from_secs(1),
            process_limit: None,
            watched_pids: Vec::new(),
            watched_names: Vec::new(),
            agent_watches: false,
            agent_controls: false,
            agent_pings: false,
            public_ip: None,
            agent_public_ip: false,
            ping: None,
            last_ping: None,
            link_rtt: None,
            pending: VecDeque::new(),
            agent_version: String::new(),
            last_error: None,
            attempt: 0,
            windows: false,
            retry_now: false,
        }
    }

    /// The target this source connects to.
    pub fn target(&self) -> &RemoteTarget {
        &self.target
    }

    /// Returns the connection, (re)connecting if needed and allowed.
    fn connection(&mut self) -> Result<&mut Connection, SourceError> {
        if self.connection.is_none() {
            if let Some(at) = self.last_attempt {
                let wait = RECONNECT_DELAY.saturating_sub(at.elapsed());
                if !wait.is_zero() {
                    let reason = self.last_error.as_deref().unwrap_or("not connected");
                    return Err(SourceError::Disconnected(format!(
                        "{}: {reason} (retrying in {}s)",
                        self.target.name,
                        wait.as_secs() + 1
                    )));
                }
            }
            self.last_attempt = Some(Instant::now());
            let fallbacks = if self.windows {
                &self.target.windows_fallback_args
            } else {
                &self.target.fallback_args
            };
            let args = match self.attempt {
                0 => &self.target.args,
                n => fallbacks.get(n - 1).unwrap_or(&self.target.args),
            };
            let mut connection = match Connection::spawn(&self.target, args) {
                Ok(connection) => connection,
                Err(err) => {
                    self.last_error = Some(match &err {
                        SourceError::Disconnected(reason) => reason.clone(),
                        other => other.to_string(),
                    });
                    return Err(err);
                }
            };
            // Known from the greeting of this agent.
            self.agent_watches = false;
            self.agent_controls = false;
            self.agent_pings = false;
            self.agent_public_ip = false;
            self.ping = None;
            self.link_rtt = None;
            // Agents start with their own default; send our settings.
            let sent = connection
                .send(&ClientMessage::SetInterval {
                    ms: self.interval.as_millis() as u64,
                })
                .and_then(|()| {
                    connection.send(&ClientMessage::SetProcessLimit {
                        limit: self.process_limit,
                    })
                });
            self.connection = Some(connection);
            if sent.is_err() {
                // ssh already exited (bad host, auth failure): its stderr
                // says why, which beats "broken pipe".
                return Err(self.disconnect(None));
            }
        }
        Ok(self.connection.as_mut().expect("connected above"))
    }

    /// Drops the connection and returns an error explaining why.
    fn disconnect(&mut self, reason: Option<String>) -> SourceError {
        let windows_attempt = self.windows && self.attempt > 0;
        let (mut reason, not_found) = match (reason, self.connection.as_mut()) {
            (Some(reason), _) => (reason, NotFound::No),
            (None, Some(connection)) => connection.failure(windows_attempt),
            (None, None) => ("connection closed".to_owned(), NotFound::No),
        };
        self.connection = None;
        if not_found != NotFound::No {
            // The answer to the configured command says which shell it is.
            if self.attempt == 0 {
                self.windows = not_found == NotFound::Windows;
            }
            let fallbacks = if self.windows {
                &self.target.windows_fallback_args
            } else {
                &self.target.fallback_args
            };
            if self.attempt < fallbacks.len() {
                self.attempt += 1;
                self.retry_now = true;
                self.last_attempt = None;
            } else if !fallbacks.is_empty() {
                // None worked; start over with the PATH next time, in case
                // rigtop gets installed there.
                self.attempt = 0;
                reason = if self.windows {
                    // cmd.exe's own words may be unreadable (see the
                    // stderr thread), so they are left out.
                    "rigtop was not found: Windows gives SSH sessions the system's PATH \
                     only, and there is no copy from the Microsoft Store or Scoop \
                     (set `command` for this host)"
                        .to_owned()
                } else {
                    format!(
                        "rigtop is not on the PATH, in ~/.local/bin or in ~/.cargo/bin ({reason})"
                    )
                };
                self.windows = false;
            }
        }
        let error = SourceError::Disconnected(format!("{}: {reason}", self.target.name));
        self.last_error = Some(reason);
        error
    }

    /// Waits up to `timeout` for the next message.
    fn next_message(&mut self, timeout: Duration) -> Result<Option<AgentMessage>, SourceError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let connection = self.connection()?;
            match connection.incoming.recv_timeout(remaining) {
                Ok(Incoming::Message(message)) => return Ok(Some(message)),
                // Answers to pings are taken here, whoever waits.
                Ok(Incoming::Pong { id, at }) => {
                    if let Some((sent_id, sent)) = self.ping
                        && sent_id == id
                    {
                        self.link_rtt = Some(at.saturating_duration_since(sent));
                        self.ping = None;
                    }
                }
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.disconnect(None));
                }
                Err(RecvTimeoutError::Timeout) => return Ok(None),
            }
        }
    }

    /// Pings the agent if it is time (agents that answer only).
    fn maybe_ping(&mut self) {
        if !self.agent_pings {
            return;
        }
        let now = Instant::now();
        let due = self
            .last_ping
            .is_none_or(|at| now.duration_since(at) >= PING_EVERY);
        let waiting = self
            .ping
            .is_some_and(|(_, sent)| now.duration_since(sent) < PING_TIMEOUT);
        if !due || waiting {
            return;
        }
        let id = self.ping.map_or(1, |(id, _)| id + 1);
        if let Some(connection) = self.connection.as_mut()
            && connection.send(&ClientMessage::Ping { id }).is_ok()
        {
            self.ping = Some((id, now));
            self.last_ping = Some(now);
        }
    }

    /// Handles messages that are not what the caller waits for. Returns an
    /// error for fatal ones (protocol mismatch).
    /// Sends the watched processes to the connected agent.
    fn send_watched(&mut self) {
        if let Some(connection) = self.connection.as_mut() {
            let _ = connection.send(&ClientMessage::WatchProcesses {
                pids: self.watched_pids.clone(),
                // Agents without the feature ignore them.
                names: self.watched_names.clone(),
            });
        }
    }

    /// Asks the connected agent for its public address (or to stop).
    fn send_public_ip(&mut self) {
        if let Some(connection) = self.connection.as_mut() {
            let lookup = self.public_ip.as_ref();
            let _ = connection.send(&ClientMessage::SetPublicIp {
                url: lookup.map(|l| l.url.clone()),
                interval_secs: lookup.map_or(0, |l| l.interval.as_secs()),
            });
        }
    }

    fn absorb(&mut self, message: AgentMessage) -> Result<(), SourceError> {
        match message {
            AgentMessage::Hello {
                protocol, version, ..
            } if protocol != PROTOCOL_VERSION => {
                let reason = format!(
                    "agent speaks protocol {protocol} (rigtop {version}), this client speaks {PROTOCOL_VERSION}; \
                     install the same rigtop version on both sides"
                );
                // Do not retry quickly against an incompatible agent.
                self.last_attempt = Some(Instant::now());
                Err(self.disconnect(Some(reason)))
            }
            AgentMessage::Hello {
                version, features, ..
            } => {
                self.agent_version = version;
                self.agent_watches = features.iter().any(|f| f == FEATURE_WATCH_PROCESSES);
                self.agent_controls = features.iter().any(|f| f == FEATURE_PROCESS_CONTROL);
                self.agent_pings = features.iter().any(|f| f == FEATURE_PING);
                self.agent_public_ip = features.iter().any(|f| f == FEATURE_PUBLIC_IP);
                if self.agent_public_ip && self.public_ip.is_some() {
                    self.send_public_ip();
                }
                if self.agent_watches
                    && !(self.watched_pids.is_empty() && self.watched_names.is_empty())
                {
                    self.send_watched();
                }
                Ok(())
            }
            AgentMessage::SignalResult { .. } | AgentMessage::Pong { .. } => Ok(()),
            AgentMessage::Snapshot { mut snapshot } => {
                // Agents before 0.9.1 do not put their version in snapshots.
                if snapshot.host.rigtop_version.is_empty() {
                    snapshot.host.rigtop_version = self.agent_version.clone();
                }
                if let Some(connection) = self.connection.as_mut() {
                    connection.established = true;
                }
                self.last_error = None;
                self.pending.push_back(*snapshot);
                Ok(())
            }
            AgentMessage::Error { message } => Err(SourceError::Remote(message)),
        }
    }
}

impl RemoteSource {
    /// One attempt at [`DataSource::poll`].
    fn poll_once(&mut self) -> Result<Snapshot, SourceError> {
        // A new connection needs time for SSH and the agent's warm-up.
        let established = self.connection.as_ref().is_some_and(|c| c.established);
        let timeout = if established {
            self.interval * 3 + Duration::from_secs(5)
        } else {
            CONNECT_TIMEOUT
        };
        let deadline = Instant::now() + timeout;
        loop {
            // Drain everything already received; keep only the newest snapshot.
            while let Some(message) = self.next_message(Duration::ZERO)? {
                self.absorb(message)?;
            }
            if let Some(mut newest) = self.pending.pop_back() {
                self.pending.clear();
                self.maybe_ping();
                // The latest round trip, even if it came after the snapshot.
                newest.link_rtt_ms = self.link_rtt.map(|rtt| rtt.as_secs_f32() * 1000.0);
                return Ok(newest);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(self.disconnect(Some("no data from the agent (timed out)".to_owned())));
            }
            if let Some(message) = self.next_message(remaining)? {
                self.absorb(message)?;
            }
        }
    }
}

impl DataSource for RemoteSource {
    /// Returns the newest snapshot from the agent, waiting for one if none
    /// arrived since the previous poll.
    fn poll(&mut self) -> Result<Snapshot, SourceError> {
        loop {
            match self.poll_once() {
                // Not found: try the next place (~/.local/bin, then
                // ~/.cargo/bin) right away.
                Err(_) if std::mem::take(&mut self.retry_now) => continue,
                result => return result,
            }
        }
    }

    fn signal_process(&mut self, pid: u32, signal: ProcessSignal) -> Result<(), SourceError> {
        if signal.is_control() {
            // Connect first, so the agent's features are known.
            self.connection()?;
            if !self.agent_controls {
                return Err(SourceError::Remote(format!(
                    "rigtop {} on the host cannot do this; it needs 0.22 or later",
                    if self.agent_version.is_empty() {
                        "(unknown)"
                    } else {
                        &self.agent_version
                    }
                )));
            }
        }
        if self
            .connection()?
            .send(&ClientMessage::Signal { pid, signal })
            .is_err()
        {
            return Err(self.disconnect(None));
        }
        let deadline = Instant::now() + SIGNAL_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SourceError::Remote(format!(
                    "no answer to the signal for {pid}"
                )));
            }
            match self.next_message(remaining)? {
                Some(AgentMessage::SignalResult {
                    pid: answered,
                    error,
                    ..
                }) if answered == pid => {
                    return match error {
                        None => Ok(()),
                        Some(error) => Err(SourceError::Remote(error)),
                    };
                }
                Some(other) => self.absorb(other)?,
                None => {}
            }
        }
    }

    fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
        if let Some(connection) = self.connection.as_mut() {
            let _ = connection.send(&ClientMessage::SetInterval {
                ms: interval.as_millis() as u64,
            });
        }
    }

    fn set_process_limit(&mut self, limit: Option<usize>) {
        self.process_limit = limit;
        if let Some(connection) = self.connection.as_mut() {
            let _ = connection.send(&ClientMessage::SetProcessLimit { limit });
        }
    }

    fn watch_processes(&mut self, pids: Vec<u32>) {
        self.watched_pids = pids;
        // Older agents would answer with an error; their snapshots leave
        // `watched_pids` empty, so the client knows it cannot tell.
        if self.agent_watches {
            self.send_watched();
        }
    }

    fn watch_process_names(&mut self, names: Vec<String>) {
        self.watched_names = names;
        if self.agent_watches {
            self.send_watched();
        }
    }

    fn set_public_ip(&mut self, lookup: Option<PublicIpLookup>) {
        if self.public_ip == lookup {
            return;
        }
        self.public_ip = lookup;
        // Older agents would answer with an error; they report no address.
        if self.agent_public_ip {
            self.send_public_ip();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_target_uses_batch_mode_and_keeps_the_destination_safe() {
        let target = RemoteTarget::ssh("zefat", DEFAULT_REMOTE_COMMAND);
        assert_eq!(target.program, "ssh");
        assert_eq!(target.name, "zefat");
        assert!(target.args.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        let n = target.args.len();
        assert_eq!(&target.args[n - 3..], ["--", "zefat", "rigtop --agent"]);
        // The default command falls back to ~/.local/bin, then ~/.cargo/bin ...
        let fallbacks: Vec<&str> = target
            .fallback_args
            .iter()
            .map(|args| args[n - 1].as_str())
            .collect();
        assert_eq!(fallbacks, FALLBACK_REMOTE_COMMANDS);
        assert_eq!(&target.fallback_args[0][n - 3..n - 1], ["--", "zefat"]);
        // ... and on a Windows host, to where the Store and Scoop put it ...
        let windows: Vec<&str> = target
            .windows_fallback_args
            .iter()
            .map(|args| args[n - 1].as_str())
            .collect();
        assert_eq!(windows, WINDOWS_FALLBACK_REMOTE_COMMANDS);
        assert!(windows[0].starts_with(r#""%LOCALAPPDATA%\Microsoft\WindowsApps\rigtop.exe""#));
        // ... a command the user chose does not.
        let custom = RemoteTarget::ssh("zefat", "/opt/rigtop/bin/rigtop --agent");
        assert!(custom.fallback_args.is_empty());
        assert!(custom.windows_fallback_args.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_windows_host_without_rigtop_gets_a_readable_message() {
        // cmd.exe on a Japanese Windows: the command in quotes, the rest in
        // code page 932 (not UTF-8), exit status 1.
        let cmd_says = r"printf '\047rigtop\047 \202\315\201A\r\n\221\200\215\354\r\n' >&2; exit 1";
        let args = |script: &str| vec!["-c".to_owned(), script.to_owned()];
        let mut source = RemoteSource::new(RemoteTarget {
            name: "win".to_owned(),
            program: "sh".to_owned(),
            args: args(cmd_says),
            fallback_args: vec![args("exit 3")],
            windows_fallback_args: vec![args("exit 1"), args("exit 1")],
        });
        let err = source.poll().unwrap_err().to_string();
        assert!(
            err.contains("Microsoft Store or Scoop") && err.contains("`command`"),
            "{err}"
        );
        assert!(!err.contains('\u{fffd}'), "no unreadable bytes: {err}");
        assert_eq!(source.attempt, 0, "the PATH comes first again next time");
        assert!(!source.windows);

        // An exit status of 1 alone (an agent that fails) is not "not found".
        let mut source = RemoteSource::new(RemoteTarget {
            name: "failing".to_owned(),
            program: "sh".to_owned(),
            args: args("echo 'Error: something else' >&2; exit 1"),
            fallback_args: vec![args("exit 3")],
            windows_fallback_args: vec![args("exit 1")],
        });
        let err = source.poll().unwrap_err().to_string();
        assert!(err.contains("something else"), "{err}");
        assert_eq!(source.attempt, 0);
    }

    /// `sh -c <script>` standing in for ssh.
    #[cfg(unix)]
    fn sh_target(script: &str, fallbacks: &[&str]) -> RemoteTarget {
        let args = |script: &str| vec!["-c".to_owned(), script.to_owned()];
        RemoteTarget {
            name: "fake".to_owned(),
            program: "sh".to_owned(),
            args: args(script),
            fallback_args: fallbacks.iter().map(|script| args(script)).collect(),
            windows_fallback_args: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_not_found_tries_the_fallback_at_once() {
        // Found nowhere: every attempt in one poll, and a clear message.
        let missing = "echo 'zsh:1: command not found: rigtop' >&2; exit 127";
        let mut source = RemoteSource::new(sh_target(missing, &[missing, missing]));
        let err = source.poll().unwrap_err().to_string();
        assert!(
            err.contains("not on the PATH, in ~/.local/bin or in ~/.cargo/bin"),
            "{err}"
        );
        assert_eq!(source.attempt, 0, "the PATH comes first again next time");

        // Other failures do not switch to the fallbacks.
        let mut source = RemoteSource::new(sh_target(
            "echo 'Permission denied (publickey)' >&2; exit 255",
            &[missing],
        ));
        let err = source.poll().unwrap_err().to_string();
        assert!(err.contains("Permission denied"), "{err}");
        assert_eq!(source.attempt, 0);
    }

    #[test]
    fn missing_program_is_reported_and_retried_later() {
        let mut source = RemoteSource::new(RemoteTarget {
            name: "nowhere".to_owned(),
            program: "/nonexistent/ssh".to_owned(),
            args: Vec::new(),
            fallback_args: Vec::new(),
            windows_fallback_args: Vec::new(),
        });
        let err = source.poll().unwrap_err().to_string();
        assert!(err.contains("cannot start"), "{err}");
        // The next attempt waits for the reconnect delay, repeating the cause.
        let err = source.poll().unwrap_err().to_string();
        assert!(
            err.contains("retrying") && err.contains("cannot start"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stderr_explains_an_agent_that_exits() {
        // Stands in for ssh failing, e.g. "rigtop: command not found".
        let mut source = RemoteSource::new(RemoteTarget {
            name: "fake".to_owned(),
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "echo 'rigtop: command not found' >&2; exit 127".to_owned(),
            ],
            fallback_args: Vec::new(),
            windows_fallback_args: Vec::new(),
        });
        let err = source.poll().unwrap_err().to_string();
        assert!(err.contains("fake: rigtop: command not found"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn incompatible_protocol_is_rejected() {
        let mut source = RemoteSource::new(RemoteTarget {
            name: "old".to_owned(),
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                r#"echo '{"type":"hello","protocol":999,"version":"9.9.9"}'; sleep 5"#.to_owned(),
            ],
            fallback_args: Vec::new(),
            windows_fallback_args: Vec::new(),
        });
        let err = source.poll().unwrap_err().to_string();
        assert!(err.contains("protocol 999"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn old_agents_get_their_version_from_the_greeting() {
        // An agent before 0.9.1: no version in its snapshots.
        let mut source = RemoteSource::new(RemoteTarget {
            name: "old".to_owned(),
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                r#"echo '{"type":"hello","protocol":1,"version":"0.8.2"}'
                   echo '{"type":"snapshot","snapshot":{"host":{"hostname":"nas"}}}'
                   sleep 5"#
                    .to_owned(),
            ],
            fallback_args: Vec::new(),
            windows_fallback_args: Vec::new(),
        });
        let snapshot = source.poll().unwrap();
        assert_eq!(snapshot.host.hostname, "nas");
        assert_eq!(snapshot.host.rigtop_version, "0.8.2");
        assert_eq!(
            crate::view::version_note(&snapshot).as_deref(),
            Some("agent 0.8.2")
        );
    }
}
