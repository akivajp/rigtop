//! Updating rigtop on a remote host to this machine's version.
//!
//! A script is fed to `sh -s` over the host's SSH connection settings. It
//! finds the rigtop that the agent command runs and, when that sits in a
//! directory the user can write to, replaces it with the release of this
//! version through the release's `install.sh` (curl or wget, checksums).
//! Copies installed by `cargo install` or a package manager are left alone,
//! with the command to update them instead. No new dependency is needed on
//! either side.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use crate::remote::RemoteTarget;

/// Where release files are downloaded from.
const RELEASES_URL: &str = "https://github.com/akivajp/rigtop/releases/download";

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
    let tag = shell_quote(&format!("v{version}"));
    let url = shell_quote(&format!("{RELEASES_URL}/v{version}/install.sh"));
    // A leading "~/" is expanded by the remote shell; quoting would stop it.
    let find = match agent_binary(agent_command) {
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
    */.cargo/bin/rigtop)
        echo "rigtop update: $bin was installed with cargo; update it with: cargo install rigtop --version {version} --locked" >&2
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
        let program = target.program.clone();
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

        let error = run(&update_script("1.2.3", "/nowhere/rigtop --agent")).unwrap_err();
        assert!(error.contains("not found"), "{error}");
    }
}
