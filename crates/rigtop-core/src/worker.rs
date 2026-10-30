//! Background thread that owns the data source.
//!
//! Polling (and, for remote sources, waiting on the network) happens off the
//! UI thread so that input stays responsive. The UI sends [`Command`]s and
//! receives [`Event`]s over channels.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{DataSource, MIN_POLL_INTERVAL, ProcessSignal, Snapshot};

/// Events kept for the UI before new ones are dropped (a few seconds of
/// snapshots at the default interval).
const EVENT_QUEUE: usize = 8;

/// Requests from the UI to the worker.
#[derive(Debug)]
pub enum Command {
    /// Send a signal to a process on the monitored host.
    Signal { pid: u32, signal: ProcessSignal },
    /// Change the polling interval.
    SetInterval(Duration),
    /// Limit snapshots to the top processes (`None`: all).
    SetProcessLimit(Option<usize>),
    /// Processes to report the exit of (see [`DataSource::watch_processes`]).
    WatchProcesses(Vec<u32>),
    /// Processes to report the exit of by name (see
    /// [`DataSource::watch_process_names`]).
    WatchProcessNames(Vec<String>),
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
        Self::spawn_with_notify(make_source, interval, || {})
    }

    /// Like [`Worker::spawn`], and calls `notify` after every event so that
    /// an event-driven UI (e.g. egui's `request_repaint`) can wake up instead
    /// of polling.
    pub fn spawn_with_notify<S, F, N>(make_source: F, interval: Duration, notify: N) -> Self
    where
        S: DataSource + 'static,
        F: FnOnce() -> S + Send + 'static,
        N: Fn() + Send + 'static,
    {
        let (command_tx, command_rx) = mpsc::channel();
        // Bounded: a UI that stops draining (e.g. a hidden window) must not
        // make the queue grow without limit; events beyond it are dropped.
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_QUEUE);
        let handle = thread::Builder::new()
            .name("rigtop-collector".to_owned())
            .spawn(move || {
                let events = Notifying {
                    sender: event_tx,
                    notify,
                };
                run(make_source(), interval, &command_rx, &events)
            })
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
/// Event sender that calls a notification hook after each send.
struct Notifying<N: Fn()> {
    sender: SyncSender<Event>,
    notify: N,
}

impl<N: Fn()> Notifying<N> {
    /// Queues an event without blocking, so that polling (and draining a
    /// remote connection) continues even if the UI stops reading. When the
    /// queue is full the event is dropped. Fails only when the UI is gone.
    fn send(&self, event: Event) -> Result<(), TrySendError<Event>> {
        match self.sender.try_send(event) {
            Ok(()) | Err(TrySendError::Full(_)) => {
                // Wake the UI either way: a full queue needs draining.
                (self.notify)();
                Ok(())
            }
            Err(err @ TrySendError::Disconnected(_)) => Err(err),
        }
    }
}

fn run<S: DataSource, N: Fn()>(
    mut source: S,
    mut interval: Duration,
    commands: &Receiver<Command>,
    events: &Notifying<N>,
) {
    // The source has just taken its warm-up sample; the first real sample
    // only needs the minimum interval, so the UI fills quickly.
    source.set_interval(interval);
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
                source.set_interval(interval);
                next_poll = last_poll + interval;
            }
            Ok(Command::SetProcessLimit(limit)) => source.set_process_limit(limit),
            Ok(Command::WatchProcesses(pids)) => source.watch_processes(pids),
            Ok(Command::WatchProcessNames(names)) => source.watch_process_names(names),
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
    fn notify_runs_after_each_event() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (signal_tx, _signal_rx) = mpsc::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let worker = Worker::spawn_with_notify(
            move || FakeSource {
                polls: 0,
                signals: signal_tx,
            },
            MIN_POLL_INTERVAL,
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        recv(&worker);
        // The event is sent before the notification; wait for the latter.
        let deadline = Instant::now() + Duration::from_secs(5);
        while count.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "notify was not called");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn undrained_events_stay_bounded() {
        let (signal_tx, _signal_rx) = mpsc::channel();
        let worker = Worker::spawn(
            move || FakeSource {
                polls: 0,
                signals: signal_tx,
            },
            MIN_POLL_INTERVAL,
        );
        // About 15 polls happen while nobody reads.
        thread::sleep(MIN_POLL_INTERVAL * 15);
        let mut queued = 0;
        while worker.try_recv().is_some() {
            queued += 1;
        }
        assert!(
            (1..=EVENT_QUEUE).contains(&queued),
            "{queued} events queued"
        );
        // The worker kept polling: the next snapshot is a recent one.
        let next = recv(&worker);
        assert!(matches!(next, Event::Snapshot(s) if s.timestamp_ms > EVENT_QUEUE as u64));
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
