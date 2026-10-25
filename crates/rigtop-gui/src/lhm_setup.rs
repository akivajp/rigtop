//! One-click setup of LibreHardwareMonitor, the source of temperatures,
//! fans and CPU power on Windows.
//!
//! The steps: install it with winget unless it is already there (winget
//! brings its PawnIO driver along), turn its web server on for this computer
//! only by editing `LibreHardwareMonitor.config` next to the executable, and
//! start it as administrator. Windows asks for permission for the driver
//! and for starting it.

use std::sync::{Arc, Mutex};
use std::thread;

use egui::{Id, RichText};

use crate::app::GuiState;
use crate::widgets::{BAD, DIM, OK, WARN};

/// Progress of a running setup, filled in by its thread.
#[derive(Debug, Default)]
struct Progress {
    steps: Vec<String>,
    outcome: Option<Result<String, String>>,
}

/// State of the setup window.
#[derive(Debug, Default)]
pub struct LhmSetup {
    open: bool,
    progress: Option<Arc<Mutex<Progress>>>,
}

impl LhmSetup {
    /// Opens the window.
    pub fn open(&mut self) {
        self.open = true;
    }

    fn busy(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| {
            p.lock()
                .unwrap_or_else(|e| e.into_inner())
                .outcome
                .is_none()
        })
    }
}

/// Port of a LibreHardwareMonitor web server on this computer, from the
/// configured URL; `None` when the URL points elsewhere (or is empty), so
/// setting it up here would not help.
pub fn local_port(url: &str) -> Option<u16> {
    let rest = url.trim().strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (authority, 80),
    };
    matches!(host, "127.0.0.1" | "localhost").then_some(port)
}

/// Whether LibreHardwareMonitor is among the processes of `state`.
fn lhm_running(state: &GuiState) -> bool {
    state.snapshot.as_ref().is_some_and(|s| {
        s.processes.iter().any(|p| {
            p.name
                .to_ascii_lowercase()
                .starts_with("librehardwaremonitor")
        })
    })
}

/// Draws the setup window when it is open.
pub fn dialog(ctx: &egui::Context, state: &mut GuiState) {
    if !state.lhm_setup.open {
        return;
    }
    let running = lhm_running(state);
    let port = local_port(&state.config.lhm.url);
    let setup = &mut state.lhm_setup;
    let busy = setup.busy();
    let mut close = false;
    // Port to set up on, when the window offers to.
    let mut offer = None;
    let modal = egui::Modal::new(Id::unique("lhm_setup")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading("Temperatures, fans and CPU power");
        ui.add_space(6.0);
        match (setup.progress.clone(), port) {
            (Some(progress), _) => {
                let progress = progress.lock().unwrap_or_else(|e| e.into_inner());
                for step in &progress.steps {
                    ui.label(step);
                }
                match &progress.outcome {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Windows may ask for permission.").color(DIM));
                        });
                    }
                    Some(Ok(message)) => {
                        ui.label(RichText::new(message).color(OK));
                    }
                    Some(Err(message)) => {
                        ui.label(RichText::new(message).color(BAD));
                    }
                }
            }
            (None, None) => {
                ui.label(
                    "The configuration ([lhm] url) points to another computer; \
                     set LibreHardwareMonitor up there.",
                );
            }
            (None, Some(_)) if running => {
                ui.label(
                    "LibreHardwareMonitor is running. In its window, turn on \
                     Options > Remote Web Server > Run; rigtop shows the values \
                     within 10 seconds.",
                );
            }
            (None, Some(port)) => {
                ui.label(
                    "Windows reports these only to programs with a hardware driver. \
                     rigtop reads them from LibreHardwareMonitor and will:",
                );
                ui.label(" - install it and its PawnIO driver with winget, unless it is installed");
                ui.label(format!(
                    " - turn on its web server for this computer only (127.0.0.1:{port})"
                ));
                ui.label(" - start it as administrator");
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Windows asks for permission for the driver and for starting it. \
                         To start it with Windows, tick Options > Run On Windows Startup \
                         in LibreHardwareMonitor afterwards.",
                    )
                    .color(DIM),
                );
                offer = Some(port);
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if let Some(port) = offer
                && ui.button(RichText::new("Set up").color(WARN)).clicked()
            {
                setup.progress = Some(start(ui.ctx(), port));
            }
            let label = if setup.progress.is_some() {
                "Close"
            } else {
                "Cancel"
            };
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                close = true;
            }
        });
    });
    if close || (modal.should_close() && !busy) {
        setup.open = false;
        setup.progress = None;
    }
}

/// Runs the setup in the background.
fn start(ctx: &egui::Context, port: u16) -> Arc<Mutex<Progress>> {
    let progress = Arc::new(Mutex::new(Progress::default()));
    let shared = Arc::clone(&progress);
    let repaint = ctx.clone();
    thread::spawn(move || {
        let step = |text: String| {
            shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .steps
                .push(text);
            repaint.request_repaint();
        };
        let outcome = run(port, &step);
        shared.lock().unwrap_or_else(|e| e.into_inner()).outcome = Some(outcome);
        repaint.request_repaint();
    });
    progress
}

#[cfg(not(windows))]
fn run(_port: u16, _step: &dyn Fn(String)) -> Result<String, String> {
    Err("LibreHardwareMonitor is for Windows.".to_owned())
}

#[cfg(windows)]
fn run(port: u16, step: &dyn Fn(String)) -> Result<String, String> {
    use std::fs;

    let exe = match windows::find_exe() {
        Some(exe) => exe,
        None => {
            step("Installing LibreHardwareMonitor and PawnIO with winget…".to_owned());
            let result = windows::winget_install();
            // winget fails when the package is already installed; what
            // counts is whether the executable is there now.
            match (windows::find_exe(), result) {
                (Some(exe), _) => exe,
                (None, Err(err)) => return Err(err),
                (None, Ok(())) => {
                    return Err("Installed, but LibreHardwareMonitor.exe was not found; \
                                start it from a new terminal with LibreHardwareMonitor."
                        .to_owned());
                }
            }
        }
    };
    step(format!("Found {}", exe.display()));

    let config = exe.with_file_name("LibreHardwareMonitor.config");
    let existing = fs::read_to_string(&config).ok();
    let updated = update_config(
        existing.as_deref(),
        &[
            ("runWebServerMenuItem", "true"),
            ("listenerIp", "127.0.0.1"),
            ("listenerPort", &port.to_string()),
        ],
    );
    fs::write(&config, updated).map_err(|err| {
        format!(
            "Cannot write {}: {err}. Turn on Options > Remote Web Server > Run in \
             LibreHardwareMonitor instead.",
            config.display()
        )
    })?;
    step(format!("Turned on its web server (127.0.0.1:{port})"));

    windows::start_elevated(&exe)?;
    step("Started LibreHardwareMonitor".to_owned());
    Ok("Done: rigtop shows the values within 10 seconds.".to_owned())
}

/// Sets `settings` in LibreHardwareMonitor's settings file (an
/// `<appSettings>` list of `<add key=".." value=".." />`), keeping the rest.
#[cfg_attr(not(windows), allow(dead_code))]
fn update_config(existing: Option<&str>, settings: &[(&str, &str)]) -> String {
    let mut text = existing
        .filter(|t| t.contains("<appSettings"))
        .unwrap_or("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<configuration>\n  <appSettings />\n</configuration>\n")
        .replace("<appSettings />", "<appSettings>\n  </appSettings>");
    for (key, value) in settings {
        let element = format!("<add key=\"{key}\" value=\"{value}\" />");
        match text.find(&format!("key=\"{key}\"")) {
            Some(at) => {
                let start = text[..at].rfind('<').unwrap_or(at);
                let end = text[at..].find("/>").map_or(text.len(), |e| at + e + 2);
                text.replace_range(start..end, &element);
            }
            None => {
                let at = text.rfind("</appSettings>").unwrap_or(text.len());
                text.insert_str(at, &format!("  {element}\n  "));
            }
        }
    }
    text
}

#[cfg(windows)]
mod windows {
    use std::env;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const EXE: &str = "LibreHardwareMonitor.exe";
    /// Directory prefix of winget's portable installs of the package.
    const PACKAGE_DIR: &str = "LibreHardwareMonitor.LibreHardwareMonitor_";

    /// The executable from winget (user or machine scope) or on the PATH.
    pub fn find_exe() -> Option<PathBuf> {
        let roots = [
            env::var_os("LOCALAPPDATA")
                .map(|d| PathBuf::from(d).join(r"Microsoft\WinGet\Packages")),
            env::var_os("ProgramFiles").map(|d| PathBuf::from(d).join(r"WinGet\Packages")),
        ];
        for root in roots.into_iter().flatten() {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let exe = entry.path().join(EXE);
                if entry.file_name().to_string_lossy().starts_with(PACKAGE_DIR) && exe.is_file() {
                    return Some(exe);
                }
            }
        }
        env::var_os("PATH").and_then(|path| {
            env::split_paths(&path)
                .map(|dir| dir.join(EXE))
                .find(|exe| exe.is_file())
        })
    }

    /// `winget install` of the package (its PawnIO dependency included).
    pub fn winget_install() -> Result<(), String> {
        let output = Command::new("winget")
            .args([
                "install",
                "--id",
                "LibreHardwareMonitor.LibreHardwareMonitor",
                "--exact",
                "--source",
                "winget",
                "--accept-source-agreements",
                "--accept-package-agreements",
                "--disable-interactivity",
            ])
            // CREATE_NO_WINDOW: no console flashing up from the GUI.
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|err| {
                format!(
                    "Cannot run winget ({err}). Install \"App Installer\" from the \
                     Microsoft Store, or install LibreHardwareMonitor as the README describes."
                )
            })?;
        if output.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        // The last message, skipping progress bars and spinners.
        let last = text
            .split(['\r', '\n'])
            .map(str::trim)
            .rfind(|l| l.len() > 1 && !l.contains(['█', '▒']))
            .unwrap_or("no output");
        Err(format!("winget failed: {last}"))
    }

    /// Starts `exe` with administrator rights (the UAC prompt).
    pub fn start_elevated(exe: &Path) -> Result<(), String> {
        use windows_sys::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
        };
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        let verb = wide(OsStr::new("runas"));
        let file = wide(exe.as_os_str());
        let dir = wide(exe.parent().unwrap_or(exe).as_os_str());
        // SAFETY: the strings are NUL-terminated and outlive the calls. COM
        // is initialized because ShellExecute may use shell extensions.
        let result = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            );
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                dir.as_ptr(),
                SW_SHOWNORMAL,
            )
        } as isize;
        // Values above 32 mean success; 5 (access denied) is a declined prompt.
        match result {
            r if r > 32 => Ok(()),
            5 => {
                Err("Not started: permission to run it as administrator was not given.".to_owned())
            }
            r => Err(format!("Cannot start {} (error {r}).", exe.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_port_only_for_this_computer() {
        assert_eq!(local_port("http://127.0.0.1:8085/data.json"), Some(8085));
        assert_eq!(local_port("http://localhost:9000/data.json"), Some(9000));
        assert_eq!(local_port("http://localhost/data.json"), Some(80));
        assert_eq!(local_port("http://192.168.1.5:8085/data.json"), None);
        assert_eq!(local_port(""), None);
    }

    #[test]
    fn update_config_sets_keys_and_keeps_the_rest() {
        let settings = [
            ("runWebServerMenuItem", "true"),
            ("listenerIp", "127.0.0.1"),
        ];
        // No file yet.
        let created = update_config(None, &settings);
        assert!(created.contains("<add key=\"runWebServerMenuItem\" value=\"true\" />"));
        assert!(created.contains("<add key=\"listenerIp\" value=\"127.0.0.1\" />"));
        assert!(created.trim_end().ends_with("</configuration>"));

        // As LibreHardwareMonitor saves it.
        let existing = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<configuration>\r\n  \
            <appSettings>\r\n    <add key=\"minTrayMenuItem\" value=\"true\" />\r\n    \
            <add key=\"runWebServerMenuItem\" value=\"false\" />\r\n  </appSettings>\r\n</configuration>";
        let updated = update_config(Some(existing), &settings);
        assert!(updated.contains("<add key=\"minTrayMenuItem\" value=\"true\" />"));
        assert!(updated.contains("<add key=\"runWebServerMenuItem\" value=\"true\" />"));
        assert!(!updated.contains("value=\"false\""));
        assert_eq!(updated.matches("runWebServerMenuItem").count(), 1);
        assert!(updated.contains("listenerIp"));

        // An empty list is written as a self-closing element.
        let empty = "<configuration><appSettings /></configuration>";
        assert!(update_config(Some(empty), &settings).contains("value=\"127.0.0.1\" />"));
    }
}
