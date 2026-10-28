//! Updating rigtop: this machine's copy to the latest release, and remote
//! hosts to this machine's version.
//!
//! The latest release is looked up with the system's `curl` (Windows 10 and
//! later ship one), from where `/releases/latest` redirects to, so no API
//! rate limit applies. An update feeds a script to `sh -s`, locally or over
//! the host's SSH connection settings. It finds the rigtop to replace and,
//! when that sits in a directory the user can write to, installs the
//! release through its `install.sh` (curl or wget, checksums). Copies
//! installed by `cargo install` or a package manager are left alone, with
//! the command to update them instead. No new dependency is needed.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::remote::RemoteTarget;

/// Where release files are downloaded from.
const RELEASES_URL: &str = "https://github.com/akivajp/rigtop/releases/download";
/// Redirects to the page of the latest release (`.../tag/v0.12.0`).
const LATEST_URL: &str = "https://github.com/akivajp/rigtop/releases/latest";

/// This rigtop's version, the one remote hosts are updated to.
pub fn target_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Whether an agent reporting `agent_version` is older than this rigtop,
/// so that updating it helps (`false` when unknown or not older).
pub fn agent_is_older(agent_version: &str) -> bool {
    match (
        parse_version(agent_version),
        parse_version(target_version()),
    ) {
        (Some(agent), Some(ours)) => agent < ours,
        _ => false,
    }
}

/// Whether `version` (e.g. of the latest release) is newer than this rigtop.
pub fn is_newer(version: &str) -> bool {
    match (parse_version(version), parse_version(target_version())) {
        (Some(other), Some(ours)) => other > ours,
        _ => false,
    }
}

/// "0.10.1" -> [0, 10, 1]; pre-release suffixes are ignored.
fn parse_version(version: &str) -> Option<Vec<u64>> {
    let core = version.split(['-', '+']).next()?;
    core.split('.').map(|part| part.parse().ok()).collect()
}

/// The program an agent command starts: its first word when that is a
/// path (`/opt/rigtop/rigtop --agent`), else `None` (found on `PATH`).
fn agent_binary(agent_command: &str) -> Option<&str> {
    agent_command
        .split_whitespace()
        .next()
        .filter(|program| program.contains('/'))
}

/// Puts `text` in single quotes for `sh`.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The script run on the remote host with `sh -s` to update the rigtop
/// that `agent_command` starts to `version` (e.g. "0.11.0").
pub fn update_script(version: &str, agent_command: &str) -> String {
    script(version, agent_binary(agent_command))
}

/// The script that updates this machine's copy at `exe` (the running
/// program, `rigtop` or `rigtop-gui`) to `version`.
pub fn self_update_script(version: &str, exe: &Path) -> String {
    script(version, Some(&exe.to_string_lossy()))
}

/// The update script for the program at `binary`, or the `rigtop` on PATH.
fn script(version: &str, binary: Option<&str>) -> String {
    let tag = shell_quote(&format!("v{version}"));
    let url = shell_quote(&format!("{RELEASES_URL}/v{version}/install.sh"));
    // A leading "~/" is expanded by the remote shell; quoting would stop it.
    let find = match binary {
        Some(path) => match path.strip_prefix("~/") {
            Some(rest) => format!("bin=\"$HOME\"/{}", shell_quote(rest)),
            None => format!("bin={}", shell_quote(path)),
        },
        // Like the agent connection: PATH first, then cargo's directory.
        None => "bin=$(command -v rigtop 2>/dev/null || true)\n\
                 [ -n \"$bin\" ] || [ ! -x \"$HOME/.cargo/bin/rigtop\" ] || bin=\"$HOME/.cargo/bin/rigtop\""
            .to_owned(),
    };
    format!(
        r#"set -eu
{find}
if [ -z "$bin" ] || [ ! -e "$bin" ]; then
    echo "rigtop update: rigtop not found on this host" >&2
    exit 1
fi
case "$bin" in
    */.cargo/bin/*)
        echo "rigtop update: $bin was installed with cargo; update it with: cargo install $(basename "$bin") --version {version} --locked" >&2
        exit 1
        ;;
esac
dir=$(dirname "$bin")
if [ ! -w "$dir" ]; then
    echo "rigtop update: $dir is not writable (a package manager's copy?); update it with the package manager or the .deb of v{version}" >&2
    exit 1
fi
# Keep rigtop-gui only where it was installed.
gui=--tui-only
[ ! -x "$dir/rigtop-gui" ] || gui=
script=$(mktemp)
trap 'rm -f "$script"' EXIT
if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 {url} -o "$script"
elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$script" {url}
else
    echo "rigtop update: needs curl or wget" >&2
    exit 1
fi
sh "$script" --version {tag} --bin-dir "$dir" $gui
"#
    )
}

/// Why a host whose OS is `os` (from its snapshot) cannot be updated from
/// here, if it cannot: the update script needs a POSIX shell.
pub fn unsupported_reason(os: &str) -> Option<String> {
    os.starts_with("Windows").then(|| {
        "on Windows, update rigtop there with `scoop update rigtop`, or with the zip \
         from the releases page"
            .to_owned()
    })
}

/// Why this machine's copy cannot update itself, if it cannot: on Windows
/// a running program cannot be replaced in place (see the README).
pub fn self_update_unsupported() -> Option<String> {
    cfg!(windows).then(|| {
        "on Windows, quit rigtop and run `scoop update rigtop`, or replace it with the \
         zip from the releases page"
            .to_owned()
    })
}

/// What updating this machine's copy to `version` does, shown before asking.
pub fn describe_self(version: &str) -> String {
    format!(
        "Update rigtop from {ours} to {version}? This downloads the v{version} release \
         from GitHub, checks it and replaces the rigtop you are running (when it is in a \
         directory you can write to; copies from cargo or a package manager are left \
         alone). The new version runs after a restart.",
        ours = target_version()
    )
}

/// One sentence on what an update does, shown before asking.
pub fn describe(host: &str, agent_version: &str) -> String {
    format!(
        "Update rigtop on {host} from {agent_version} to {version}? Over SSH, this \
         downloads the v{version} release from GitHub, checks it and replaces the \
         rigtop the agent runs (when it is in a directory you can write to; copies \
         from cargo or a package manager are left alone). The connection then restarts.",
        version = target_version()
    )
}

/// An update running in the background.
pub struct UpdateJob {
    result: Receiver<Result<String, String>>,
}

impl UpdateJob {
    /// Starts updating the host `target` connects to, to this version.
    pub fn start(target: &RemoteTarget) -> Self {
        let agent_command = target.args.last().cloned().unwrap_or_default();
        let script = update_script(target_version(), &agent_command);
        // Same SSH options and destination, but a shell reading the script.
        let mut args = target.args.clone();
        if let Some(last) = args.last_mut() {
            *last = "sh -s".to_owned();
        }
        Self::spawn(target.program.clone(), args, script)
    }

    /// Starts updating this machine's copy at `exe` to `version`. `exe`
    /// must be taken at startup: once replaced, the running program's
    /// path reads "... (deleted)" on Linux.
    pub fn start_self(version: &str, exe: &Path) -> Self {
        let script = self_update_script(version, exe);
        Self::spawn("sh".to_owned(), vec!["-s".to_owned()], script)
    }

    fn spawn(program: String, args: Vec<String>, script: String) -> Self {
        let (tx, result) = mpsc::channel();
        thread::Builder::new()
            .name("rigtop-update".to_owned())
            .spawn(move || {
                let result = run_script(&program, &args, &script).map(|line| {
                    // install.sh ends with "rigtop install: done: rigtop 0.11.0".
                    line.strip_prefix("rigtop install: done: ")
                        .map_or(line.clone(), str::to_owned)
                });
                let _ = tx.send(result);
            })
            .expect("spawn update thread");
        Self { result }
    }

    /// The outcome once finished: the last line of the output, or why it
    /// failed.
    pub fn poll(&self) -> Option<Result<String, String>> {
        self.result.try_recv().ok()
    }
}

/// How often [`ReleaseChecker`] looks for a new release by default.
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(3600);

/// Looks for new releases now and then, in the background.
pub struct ReleaseChecker {
    interval: Duration,
    next_check: Instant,
    running: Option<Receiver<Result<String, String>>>,
    /// Newest version seen that is newer than this rigtop.
    newer: Option<String>,
    fetch: fn() -> Result<String, String>,
}

impl ReleaseChecker {
    /// Checks right away, then every `interval`.
    pub fn new(interval: Duration) -> Self {
        Self::with_fetch(interval, latest_release)
    }

    fn with_fetch(interval: Duration, fetch: fn() -> Result<String, String>) -> Self {
        Self {
            interval,
            next_check: Instant::now(),
            running: None,
            newer: None,
            fetch,
        }
    }

    /// A newer release than this rigtop, once one has been seen (e.g.
    /// "0.12.1"). Call regularly: it also starts the next check when due.
    pub fn poll(&mut self) -> Option<&str> {
        if let Some(running) = &self.running
            && let Ok(result) = running.try_recv()
        {
            self.running = None;
            // A failed check (offline, no curl) just waits for the next one.
            if let Ok(version) = result
                && is_newer(&version)
            {
                self.newer = Some(version);
            }
        }
        if self.running.is_none() && Instant::now() >= self.next_check {
            self.next_check = Instant::now() + self.interval;
            let (tx, rx) = mpsc::channel();
            let fetch = self.fetch;
            let started = thread::Builder::new()
                .name("rigtop-release-check".to_owned())
                .spawn(move || {
                    let _ = tx.send(fetch());
                });
            if started.is_ok() {
                self.running = Some(rx);
            }
        }
        self.newer.as_deref()
    }
}

/// The version of the latest release (e.g. "0.12.0"), from where GitHub's
/// `/releases/latest` redirects to.
pub fn latest_release() -> Result<String, String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut command = Command::new("curl");
    command
        .args([
            "-fsSL",
            "--max-time",
            "30",
            "-o",
            null,
            "-w",
            "%{url_effective}",
        ])
        .arg(LATEST_URL)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a GUI must not flash a console for curl.exe.
        command.creation_flags(0x0800_0000);
    }
    let output = command
        .output()
        .map_err(|err| format!("cannot run curl: {err}"))?;
    if !output.status.success() {
        return Err(format!("curl exited with {}", output.status));
    }
    version_from_release_url(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "no release found".to_owned())
}

/// ".../releases/tag/v0.12.0" -> "0.12.0".
fn version_from_release_url(url: &str) -> Option<String> {
    let tag = url.trim().rsplit_once("/tag/")?.1;
    let version = tag.strip_prefix('v')?;
    parse_version(version).map(|_| version.to_owned())
}

/// Runs `program args` with `script` on its stdin and returns the last
/// line it printed, or, when it fails, the last line of its errors.
fn run_script(program: &str, args: &[String], script: &str) -> Result<String, String> {
    let mut command = Command::new(program);
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
    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot start {program}: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A shell that exits early closes the pipe; its status says why.
        let _ = stdin.write_all(script.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("{program} failed: {err}"))?;
    let last_line = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    };
    if output.status.success() {
        Ok(last_line(&output.stdout).unwrap_or_else(|| "done".to_owned()))
    } else {
        Err(last_line(&output.stderr)
            .or_else(|| last_line(&output.stdout))
            .unwrap_or_else(|| format!("{program} exited with {}", output.status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(parse_version("0.10.1"), Some(vec![0, 10, 1]));
        assert_eq!(parse_version("1.2.0-rc.1"), Some(vec![1, 2, 0]));
        assert_eq!(parse_version(""), None);
        assert!(agent_is_older("0.0.9"));
        assert!(!agent_is_older(target_version()));
        assert!(!agent_is_older("999.0.0"));
        assert!(!agent_is_older(""), "unknown versions are not offered");
    }

    #[test]
    fn newer_releases_are_found() {
        assert_eq!(
            version_from_release_url("https://github.com/akivajp/rigtop/releases/tag/v0.12.1\n"),
            Some("0.12.1".to_owned())
        );
        assert_eq!(
            version_from_release_url("https://github.com/akivajp/rigtop/releases"),
            None
        );
        assert!(is_newer("999.0.0"));
        assert!(!is_newer(target_version()));

        let mut checker =
            ReleaseChecker::with_fetch(Duration::from_secs(3600), || Ok("999.0.0".to_owned()));
        // The first call starts the check; the answer comes a moment later.
        let mut found = None;
        for _ in 0..100 {
            if let Some(version) = checker.poll() {
                found = Some(version.to_owned());
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(found.as_deref(), Some("999.0.0"));

        let mut checker = ReleaseChecker::with_fetch(Duration::from_secs(3600), || {
            Ok(target_version().to_owned())
        });
        checker.poll();
        thread::sleep(Duration::from_millis(50));
        assert!(checker.poll().is_none(), "the same version is not news");
    }

    /// Asks GitHub (network): `cargo test -p rigtop-core -- --ignored latest_release`.
    #[test]
    #[ignore]
    fn latest_release_from_github() {
        let version = latest_release().unwrap();
        assert!(parse_version(&version).is_some(), "{version}");
    }

    #[test]
    fn self_update_script_targets_the_running_program() {
        let script = self_update_script("1.2.3", Path::new("/home/me/.local/bin/rigtop-gui"));
        assert!(script.contains("bin='/home/me/.local/bin/rigtop-gui'"));
    }

    #[test]
    fn windows_hosts_get_directions_instead() {
        assert!(unsupported_reason("Windows 11 Pro").is_some());
        assert!(unsupported_reason("Linux (Ubuntu 24.04)").is_none());
    }

    #[test]
    fn script_uses_the_agent_command_path() {
        assert!(update_script("1.2.3", "rigtop --agent").contains("command -v rigtop"));
        let script = update_script("1.2.3", "/opt/my rig/rigtop --agent");
        // The first word only; spaces in paths need `command -v`.
        assert!(script.contains("bin='/opt/my'"));
        let script = update_script("1.2.3", "~/bin/rigtop --agent");
        assert!(script.contains(r#"bin="$HOME"/'bin/rigtop'"#));
        assert!(
            script.contains(
                "'https://github.com/akivajp/rigtop/releases/download/v1.2.3/install.sh'"
            )
        );
        assert!(script.contains("--version 'v1.2.3'"));
    }

    /// Runs the script with `sh` in a sandbox: a fake rigtop on PATH and a
    /// fake curl whose "install.sh" prints the arguments it got.
    #[cfg(unix)]
    #[test]
    fn script_installs_next_to_the_running_rigtop() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let tools = tmp.path().join("tools");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&tools).unwrap();
        let executable = |path: &std::path::Path, text: &str| {
            fs::write(path, text).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        executable(&bin.join("rigtop"), "#!/bin/sh\n");
        // `curl ... URL -o FILE` writes a stand-in install.sh to FILE.
        executable(
            &tools.join("curl"),
            "#!/bin/sh\nwhile [ $# -gt 1 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n\
             printf 'echo installed \"$@\"\\n' > \"$out\"\n",
        );
        let path = format!("{}:{}:/usr/bin:/bin", bin.display(), tools.display());
        let run = |script: &str| {
            run_script(
                "env",
                &[format!("PATH={path}"), "sh".to_owned(), "-s".to_owned()],
                script,
            )
        };

        let output = run(&update_script("1.2.3", "rigtop --agent")).unwrap();
        assert_eq!(
            output,
            format!(
                "installed --version v1.2.3 --bin-dir {} --tui-only",
                bin.display()
            )
        );

        // A copy the user cannot replace is left alone, with a reason.
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o555)).unwrap();
        let result = run(&update_script("1.2.3", "rigtop --agent"));
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        // Root can write anywhere (e.g. in containers); then it installs.
        if let Err(error) = result {
            assert!(error.contains("is not writable"), "{error}");
        }

        // cargo's copy is cargo's to update.
        let cargo = tmp.path().join(".cargo/bin");
        fs::create_dir_all(&cargo).unwrap();
        executable(&cargo.join("rigtop"), "#!/bin/sh\n");
        let command = format!("{}/rigtop --agent", cargo.display());
        let error = run(&update_script("1.2.3", &command)).unwrap_err();
        assert!(
            error.contains("cargo install rigtop --version 1.2.3"),
            "{error}"
        );

        // Updating this machine's copy works the same, for rigtop-gui too.
        executable(&bin.join("rigtop-gui"), "#!/bin/sh\n");
        let output = run(&self_update_script("1.2.3", &bin.join("rigtop-gui"))).unwrap();
        assert_eq!(
            output,
            format!("installed --version v1.2.3 --bin-dir {}", bin.display())
        );

        let error = run(&update_script("1.2.3", "/nowhere/rigtop --agent")).unwrap_err();
        assert!(error.contains("not found"), "{error}");
    }
}
