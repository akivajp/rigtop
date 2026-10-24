//! Background thread that owns the data source.
//!
//! Polling (and, for remote sources, waiting on the network) happens off the
//! UI thread so that input stays responsive. The UI sends [`Command`]s and
//! receives [`Event`]s over channels.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{DataSource, MIN_POLL_INTERVAL, ProcessSignal, Snapshot};

/// Requests from the UI to the worker.
#[derive(Debug)]
pub enum Command {
    /// Send a signal to a process on the monitored host.
    Signal { pid: u32, signal: ProcessSignal },
    /// Change the polling interval.
    SetInterval(Duration),
}

/// Messages from the worker to the UI.
#[derive(Debug)]
pub enum Event {
    /// A new measurement.
    Snapshot(Box<Snapshot>),
    /// Outcome of a [`Command::Signal`].
    SignalResult {
        pid: u32,
        signal: ProcessSignal,
        result: Result<(), String>,
    },
    /// Polling failed; the worker keeps trying.
    Error(String),
}

/// Handle to the worker thread. Dropping it stops the thread.
pub struct Worker {
    commands: Option<Sender<Command>>,
    events: Receiver<Event>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// Starts the worker. `make_source` runs on the worker thread, so slow
    /// initialization (NVML, warm-up sample, SSH connection) does not delay
    /// the first frame.
    pub fn spawn<S, F>(make_source: F, interval: Duration) -> Self
    where
        S: DataSource + 'static,
        F: FnOnce() -> S + Send + 'static,
    {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("rigtop-collector".to_owned())
            .spawn(move || run(make_source(), interval, &command_rx, &event_tx))
            .expect("failed to spawn the collector thread");
        Self {
            commands: Some(command_tx),
            events: event_rx,
            handle: Some(handle),
        }
    }

    /// Sends a command; silently ignored if the worker has stopped.
    pub fn send(&self, command: Command) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(command);
        }
    }

    /// Returns the next pending event without blocking.
    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the command channel makes the worker loop exit.
        self.commands.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Worker loop: poll on schedule, execute commands in between.
fn run<S: DataSource>(
    mut source: S,
    mut interval: Duration,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
) {
    // The source has just taken its warm-up sample; the first real sample
    // only needs the minimum interval, so the UI fills quickly.
    let mut last_poll = Instant::now();
    let mut next_poll = last_poll + MIN_POLL_INTERVAL;
    loop {
        let timeout = next_poll.saturating_duration_since(Instant::now());
        match commands.recv_timeout(timeout) {
            Ok(Command::Signal { pid, signal }) => {
                let result = source
                    .signal_process(pid, signal)
                    .map_err(|e| e.to_string());
                if events
                    .send(Event::SignalResult {
                        pid,
                        signal,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Ok(Command::SetInterval(new_interval)) => {
                interval = new_interval.max(MIN_POLL_INTERVAL);
                next_poll = last_poll + interval;
            }
            Err(RecvTimeoutError::Timeout) => {
                let event = match source.poll() {
                    Ok(snapshot) => Event::Snapshot(Box::new(snapshot)),
                    Err(err) => Event::Error(err.to_string()),
                };
                last_poll = Instant::now();
                // Keep a steady cadence, but never try to catch up on
                // missed ticks (e.g. after the machine was suspended).
                next_poll = (next_poll + interval).max(last_poll + MIN_POLL_INTERVAL);
                if events.send(event).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceError;

    /// A source that counts polls and records signals.
    struct FakeSource {
        polls: u64,
        signals: Sender<(u32, ProcessSignal)>,
    }

    impl DataSource for FakeSource {
        fn poll(&mut self) -> Result<Snapshot, SourceError> {
            self.polls += 1;
            Ok(Snapshot {
                timestamp_ms: self.polls,
                ..Default::default()
            })
        }

        fn signal_process(&mut self, pid: u32, signal: ProcessSignal) -> Result<(), SourceError> {
            self.signals.send((pid, signal)).unwrap();
            if pid == 1 {
                Err(SourceError::Disconnected("denied".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    fn recv(worker: &Worker) -> Event {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = worker.try_recv() {
                return event;
            }
            assert!(Instant::now() < deadline, "no event from worker");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn delivers_snapshots_and_signal_results() {
        let (signal_tx, signal_rx) = mpsc::channel();
        let worker = Worker::spawn(
            move || FakeSource {
                polls: 0,
                signals: signal_tx,
            },
            MIN_POLL_INTERVAL,
        );

        match recv(&worker) {
            Event::Snapshot(s) => assert_eq!(s.timestamp_ms, 1),
            other => panic!("unexpected {other:?}"),
        }

        worker.send(Command::Signal {
            pid: 1,
            signal: ProcessSignal::Kill,
        });
        // Skip snapshots until the signal result arrives.
        let result = loop {
            if let Event::SignalResult { pid, result, .. } = recv(&worker) {
                assert_eq!(pid, 1);
                break result;
            }
        };
        assert!(result.unwrap_err().contains("denied"));
        assert_eq!(signal_rx.recv().unwrap(), (1, ProcessSignal::Kill));
        // Dropping the worker joins the thread without hanging.
        drop(worker);
    }
}
