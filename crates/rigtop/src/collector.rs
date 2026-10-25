//! `rigtop --install-collector`: sets up the root systemd timer that copies
//! SMART data and btrfs scrub results to world-readable files, so a
//! `cargo install` is enough (no repository checkout for `contrib/`).
//!
//! The files are the ones in `contrib/collect/`, embedded at build time.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Embedded files and where they go (relative to `/`), with their modes.
const FILES: &[(&str, &str, u32)] = &[
    (
        "usr/local/libexec/rigtop-collect",
        include_str!("../collect/rigtop-collect"),
        0o755,
    ),
    (
        "etc/systemd/system/rigtop-collect.service",
        include_str!("../collect/rigtop-collect.service"),
        0o644,
    ),
    (
        "etc/systemd/system/rigtop-collect.timer",
        include_str!("../collect/rigtop-collect.timer"),
        0o644,
    ),
];

/// Where the collector writes (the service's StateDirectory).
const DATA_DIR: &str = "/var/lib/rigtop";
const TIMER: &str = "rigtop-collect.timer";
const SERVICE: &str = "rigtop-collect.service";

/// Installs the collector and starts its timer. Needs root.
pub fn install() -> Result<()> {
    require_systemd()?;
    let written =
        write_files(Path::new("/")).map_err(|e| with_sudo_hint(e, "--install-collector"))?;
    for path in &written {
        println!("wrote {}", path.display());
    }
    if find_program("smartctl").is_none() {
        eprintln!(
            "warning: smartctl not found, so only btrfs scrub results will be collected. \
             Install smartmontools (Debian/Ubuntu: apt install smartmontools; \
             Arch: pacman -S smartmontools)."
        );
    }
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", TIMER])?;
    println!("enabled {TIMER} (every 10 minutes)");
    // Collect right away instead of waiting for the timer's first run.
    println!("collecting once now…");
    systemctl(&["start", SERVICE])?;
    println!(
        "done: rigtop reads {DATA_DIR} from now on. \
         Check with `systemctl list-timers {TIMER}`; undo with --uninstall-collector."
    );
    Ok(())
}

/// Stops the timer and removes the collector and its data. Needs root.
pub fn uninstall() -> Result<()> {
    require_systemd()?;
    // Not installed (or already stopped) is fine.
    let _ = systemctl(&["disable", "--now", TIMER]);
    for (relative, _, _) in FILES {
        let path = Path::new("/").join(relative);
        match fs::remove_file(&path) {
            Ok(()) => println!("removed {}", path.display()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(with_sudo_hint(err, "--uninstall-collector"))
                    .with_context(|| format!("cannot remove {}", path.display()));
            }
        }
    }
    systemctl(&["daemon-reload"])?;
    match fs::remove_dir_all(DATA_DIR) {
        Ok(()) => println!("removed {DATA_DIR}"),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("cannot remove {DATA_DIR}")),
    }
    Ok(())
}

/// Writes the embedded files under `root` with their modes, creating
/// directories; returns the paths written.
fn write_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    use std::os::unix::fs::PermissionsExt;

    let mut written = Vec::new();
    for (relative, content, mode) in FILES {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(*mode))?;
        written.push(path);
    }
    Ok(written)
}

/// Turns "permission denied" into a hint to use sudo with the full path:
/// sudo's `secure_path` usually lacks `~/.cargo/bin`.
fn with_sudo_hint(err: io::Error, option: &str) -> anyhow::Error {
    if err.kind() != io::ErrorKind::PermissionDenied {
        return err.into();
    }
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "rigtop".to_owned());
    anyhow::anyhow!("{err}: run as root, e.g. sudo {exe} {option}")
}

fn require_systemd() -> Result<()> {
    if !Path::new("/run/systemd/system").is_dir() {
        bail!(
            "systemd is not running; run contrib/collect/rigtop-collect as root \
             from cron instead (see the README)"
        );
    }
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .context("cannot run systemctl")?;
    if !status.success() {
        bail!("systemctl {} failed ({status})", args.join(" "));
    }
    Ok(())
}

/// Looks `name` up on the PATH and in the sbin directories root's PATH has.
fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/sbin", "/usr/local/sbin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn writes_the_files_with_their_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let written = write_files(tmp.path()).unwrap();
        assert_eq!(written.len(), 3);
        let mode = |relative: &str| {
            fs::metadata(tmp.path().join(relative))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("usr/local/libexec/rigtop-collect"), 0o755);
        assert_eq!(mode("etc/systemd/system/rigtop-collect.timer"), 0o644);
    }

    #[test]
    fn service_runs_the_installed_script() {
        let (script, ..) = FILES[0];
        let service = FILES[1].1;
        assert!(
            service.contains(&format!("ExecStart=/{script}")),
            "the service must start the script where it is installed"
        );
        assert!(FILES[0].1.starts_with("#!/bin/sh"));
        assert!(FILES[2].1.contains("OnUnitActiveSec"));
    }
}
