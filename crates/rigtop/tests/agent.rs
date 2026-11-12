//! End-to-end tests of `rigtop --agent` with `RemoteSource`, running the
//! agent binary directly instead of through ssh.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rigtop_core::DataSource;
use rigtop_core::remote::{RemoteSource, RemoteTarget};

fn agent_source() -> RemoteSource {
    RemoteSource::new(RemoteTarget {
        name: "local-agent".to_owned(),
        program: env!("CARGO_BIN_EXE_rigtop").to_owned(),
        args: vec!["--agent".to_owned()],
        fallback_args: Vec::new(),
        windows_fallback_args: Vec::new(),
    })
}

/// rigtop missing from the PATH and from the first fallback place: the
/// agent starts through the second fallback command, as
/// `~/.cargo/bin/rigtop --agent` would after `~/.local/bin/rigtop`.
#[cfg(unix)]
#[test]
fn falls_back_when_the_command_is_not_found() {
    let mut source = RemoteSource::new(RemoteTarget {
        name: "no-path".to_owned(),
        program: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "echo 'sh: 1: rigtop: not found' >&2; exit 127".to_owned(),
        ],
        fallback_args: vec![
            vec![
                "-c".to_owned(),
                "echo 'sh: 1: ~/.local/bin/rigtop: not found' >&2; exit 127".to_owned(),
            ],
            vec![
                "-c".to_owned(),
                format!("exec '{}' --agent", env!("CARGO_BIN_EXE_rigtop")),
            ],
        ],
        windows_fallback_args: Vec::new(),
    });
    let snapshot = source.poll().expect("snapshot through the fallback");
    assert!(!snapshot.host.hostname.is_empty());
}

/// A Windows host whose SSH session does not find rigtop (the user's PATH
/// is missing there): cmd.exe's answer, here as a Japanese Windows gives it
/// (in its code page, not UTF-8), leads to the Windows places, not to
/// `~/.local/bin`, and the agent starts from the second of them.
#[cfg(unix)]
#[test]
fn falls_back_to_the_windows_places_when_cmd_does_not_find_it() {
    // "'rigtop' は、内部コマンドまたは外部コマンド、" in code page 932.
    let cmd_says = r"printf '\047rigtop\047 \202\315\201A\223\340\225\224\r\n\221\200\215\354\r\n' >&2; exit 1";
    let mut source = RemoteSource::new(RemoteTarget {
        name: "windows".to_owned(),
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), cmd_says.to_owned()],
        fallback_args: vec![vec![
            "-c".to_owned(),
            "echo 'a POSIX place was tried on a Windows host' >&2; exit 2".to_owned(),
        ]],
        windows_fallback_args: vec![
            // No copy from the Store: only the exit status says so.
            vec!["-c".to_owned(), "exit 1".to_owned()],
            vec![
                "-c".to_owned(),
                format!("exec '{}' --agent", env!("CARGO_BIN_EXE_rigtop")),
            ],
        ],
    });
    let snapshot = source
        .poll()
        .expect("snapshot through the Windows fallback");
    assert!(!snapshot.host.hostname.is_empty());
}

#[test]
fn remote_source_measures_the_round_trip() {
    let mut source = agent_source();
    source.set_interval(Duration::from_millis(250));
    let first = source.poll().expect("first snapshot");
    assert_eq!(first.link_rtt_ms, None, "not measured yet");
    // The first ping goes out with the first snapshot.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = source.poll().expect("snapshot");
        if let Some(ms) = snapshot.link_rtt_ms {
            assert!((0.0..2_000.0).contains(&ms), "{ms}");
            break;
        }
        assert!(Instant::now() < deadline, "no round trip measured");
    }
}

#[test]
fn remote_source_receives_snapshots_and_limits_processes() {
    let mut source = agent_source();
    source.set_interval(Duration::from_millis(300));
    let first = source.poll().expect("first snapshot");
    assert!(first.process_count > 1);
    assert!(!first.host.hostname.is_empty());

    source.set_process_limit(Some(5));
    // The limit applies from the next snapshot the agent takes.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = source.poll().expect("snapshot");
        if snapshot.processes.len() <= 5 {
            assert!(snapshot.process_count > 5, "the total is still reported");
            break;
        }
        assert!(Instant::now() < deadline, "process limit not applied");
    }
}

#[cfg(unix)]
#[test]
fn remote_source_kills_processes_on_the_agent_host() {
    use rigtop_core::ProcessSignal;

    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let mut source = agent_source();
    source.poll().expect("connected");
    source
        .signal_process(child.id(), ProcessSignal::Kill)
        .expect("signal through the agent");
    assert!(!child.wait().unwrap().success());
    // Errors from the agent come back too.
    let err = source
        .signal_process(child.id(), ProcessSignal::Kill)
        .unwrap_err();
    assert!(err.to_string().contains("remote"), "{err}");
}

#[test]
fn agent_exits_when_stdin_closes() {
    let mut agent = Command::new(env!("CARGO_BIN_EXE_rigtop"))
        .arg("--agent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(agent.stdout.take().unwrap());
    let mut hello = String::new();
    stdout.read_line(&mut hello).unwrap();
    assert!(hello.contains(r#""type":"hello""#), "{hello}");

    drop(agent.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = agent.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "agent kept running after stdin closed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
