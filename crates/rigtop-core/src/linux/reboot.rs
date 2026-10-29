//! Whether the machine needs a reboot to finish an update.
//!
//! Debian and Ubuntu say so with `/run/reboot-required` (and the packages
//! behind it in `/run/reboot-required.pkgs`). Elsewhere, a running kernel
//! whose modules are gone from `/usr/lib/modules` was replaced by an update
//! (Arch removes the old kernel's modules), and new hardware or modules can
//! no longer be loaded until the next boot.

use std::fs;
use std::path::Path;

/// Package names shown in the reason before "+N more".
const MAX_PACKAGES: usize = 3;

/// Why a reboot is needed (possibly empty), or `None` when it is not.
/// `run_dir` is normally `/run`, `modules_dir` `/usr/lib/modules`, and
/// `release` the running kernel's (`/proc/sys/kernel/osrelease`).
pub fn reboot_required(run_dir: &Path, modules_dir: &Path, release: &str) -> Option<String> {
    let release = release.trim();
    if run_dir.join("reboot-required").exists() {
        let packages = fs::read_to_string(run_dir.join("reboot-required.pkgs")).unwrap_or_default();
        return Some(packages_reason(&packages));
    }
    kernel_replaced(modules_dir, release)
        .then(|| format!("running kernel {release} is no longer installed"))
}

/// "updated: a, b, c +2 more" from the lines of `reboot-required.pkgs`
/// (empty without any).
fn packages_reason(text: &str) -> String {
    let mut names: Vec<&str> = Vec::new();
    for name in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        return String::new();
    }
    let mut reason = format!(
        "updated: {}",
        names[..names.len().min(MAX_PACKAGES)].join(", ")
    );
    if names.len() > MAX_PACKAGES {
        reason.push_str(&format!(" +{} more", names.len() - MAX_PACKAGES));
    }
    reason
}

/// The modules of the running kernel are gone while others are installed.
/// Not judged without any modules (containers) or under WSL, whose kernel
/// comes from Windows.
fn kernel_replaced(modules_dir: &Path, release: &str) -> bool {
    if release.is_empty() || release.contains("microsoft") {
        return false;
    }
    let installed = fs::read_dir(modules_dir)
        .map(|entries| entries.flatten().next().is_some())
        .unwrap_or(false);
    installed && !modules_dir.join(release).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    #[test]
    fn debian_flag_with_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("run");
        let modules = tmp.path().join("modules");
        assert_eq!(reboot_required(&run, &modules, "7.0.0-38-generic"), None);

        write(&run, "reboot-required", "*** System restart required ***\n");
        assert_eq!(
            reboot_required(&run, &modules, "7.0.0-38-generic"),
            Some(String::new())
        );
        write(
            &run,
            "reboot-required.pkgs",
            "linux-image-7.0.0-39-generic\nlibc6\nlibc6\ndbus\nsystemd\nlibssl3\n",
        );
        assert_eq!(
            reboot_required(&run, &modules, "7.0.0-38-generic").as_deref(),
            Some("updated: linux-image-7.0.0-39-generic, libc6, dbus +2 more")
        );
    }

    #[test]
    fn replaced_kernel() {
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("run");
        let modules = tmp.path().join("modules");
        // No modules at all (a container): not judged.
        assert_eq!(reboot_required(&run, &modules, "6.17.5-arch1-1"), None);

        write(&modules, "6.17.5-arch1-1/modules.dep", "");
        assert_eq!(reboot_required(&run, &modules, "6.17.5-arch1-1"), None);

        // An update replaced the running kernel.
        fs::remove_dir_all(modules.join("6.17.5-arch1-1")).unwrap();
        write(&modules, "6.17.6-arch1-1/modules.dep", "");
        assert_eq!(
            reboot_required(&run, &modules, "6.17.5-arch1-1\n").as_deref(),
            Some("running kernel 6.17.5-arch1-1 is no longer installed")
        );
        // WSL's kernel is not installed from packages.
        assert_eq!(
            reboot_required(&run, &modules, "6.6.87.2-microsoft-standard-WSL2"),
            None
        );
    }
}
