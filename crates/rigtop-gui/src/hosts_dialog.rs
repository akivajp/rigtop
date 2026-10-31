//! A window for adding, editing, testing and removing remote hosts, so that
//! nobody has to find and edit the configuration file by hand.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use egui::{RichText, Ui};
use rigtop_core::DataSource;
use rigtop_core::config::HostConfig;
use rigtop_core::remote::{DEFAULT_REMOTE_COMMAND, RemoteSource, RemoteTarget};

use crate::widgets::{BAD, DIM, OK, WARN};

/// Result of a connection test, filled in by a background thread.
type TestResult = Arc<Mutex<Option<Result<String, String>>>>;

/// The host being added or edited.
#[derive(Debug, Clone, Default)]
struct HostForm {
    /// Index in the host list when editing; `None` when adding.
    index: Option<usize>,
    name: String,
    ssh: String,
    command: String,
}

impl HostForm {
    fn from_host(index: usize, host: &HostConfig) -> Self {
        Self {
            index: Some(index),
            name: host.name.clone(),
            ssh: host.ssh.clone().unwrap_or_default(),
            command: host.command.clone().unwrap_or_default(),
        }
    }

    /// Builds the host, keeping settings the form does not edit (sections).
    fn to_host(&self, previous: Option<&HostConfig>) -> HostConfig {
        let optional = |s: &str| {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_owned())
        };
        HostConfig {
            name: self.name.trim().to_owned(),
            ssh: optional(&self.ssh),
            command: optional(&self.command),
            sections: previous.and_then(|h| h.sections.clone()),
            alerts: previous.and_then(|h| h.alerts.clone()),
        }
    }

    /// Why the form cannot be saved, if it cannot.
    fn problem(&self, hosts: &[HostConfig], reserved: &[String]) -> Option<String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Some("enter a name".to_owned());
        }
        let taken = hosts
            .iter()
            .enumerate()
            .any(|(i, h)| h.name == name && Some(i) != self.index)
            || reserved.iter().any(|r| r == name);
        taken.then(|| format!("\"{name}\" is already used"))
    }
}

/// State of the hosts window.
#[derive(Default)]
pub struct HostsDialog {
    open: bool,
    form: Option<HostForm>,
    test: Option<TestResult>,
    /// Outcome of the last save (message, success).
    message: Option<(String, bool)>,
}

impl HostsDialog {
    /// Opens the window with a fresh state.
    pub fn open(&mut self) {
        self.open = true;
        self.message = None;
    }

    /// Shows the outcome of saving to the user.
    pub fn report(&mut self, message: String, ok: bool) {
        self.message = Some((message, ok));
    }
}

/// Draws the window. Returns the new host list when the user saved a host
/// or removed one; the caller writes it to the configuration file.
///
/// `fixed` are hosts that cannot be edited here (from `--host`, and the
/// local machine); their names are reserved.
pub fn show(
    ctx: &egui::Context,
    dialog: &mut HostsDialog,
    hosts: &[HostConfig],
    fixed: &[String],
    config_path: Option<&Path>,
) -> Option<Vec<HostConfig>> {
    if !dialog.open {
        return None;
    }
    let mut open = true;
    let mut result = None;
    let screen = ctx.content_rect();
    egui::Window::new("Hosts")
        .open(&mut open)
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(egui::pos2(screen.center().x, screen.top() + 60.0))
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .show(ctx, |ui| {
            result = contents(ui, dialog, hosts, fixed, config_path);
        });
    if !open {
        dialog.open = false;
        dialog.form = None;
        dialog.test = None;
    }
    result
}

fn contents(
    ui: &mut Ui,
    dialog: &mut HostsDialog,
    hosts: &[HostConfig],
    fixed: &[String],
    config_path: Option<&Path>,
) -> Option<Vec<HostConfig>> {
    let mut result = None;
    ui.label(
        RichText::new(
            "Remote hosts are monitored over SSH; they need rigtop installed and key-based login.",
        )
        .color(DIM),
    );
    ui.add_space(6.0);

    // Hosts from `--host` are listed but cannot be edited.
    let from_command_line: Vec<&String> = fixed
        .iter()
        .filter(|n| *n != rigtop_core::hosts::LOCAL_NAME)
        .collect();
    if hosts.is_empty() && from_command_line.is_empty() {
        ui.label(RichText::new("No remote hosts yet.").color(DIM));
    } else {
        egui::Grid::new("hosts_list")
            .striped(true)
            .num_columns(4)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("SSH destination").strong());
                ui.label(RichText::new("Agent command").strong());
                ui.label("");
                ui.end_row();
                for (index, host) in hosts.iter().enumerate() {
                    ui.label(&host.name);
                    ui.label(host.destination());
                    ui.label(host.command.as_deref().unwrap_or(DEFAULT_REMOTE_COMMAND));
                    ui.horizontal(|ui| {
                        if ui.button("edit").clicked() {
                            dialog.form = Some(HostForm::from_host(index, host));
                            dialog.test = None;
                        }
                        if ui.button(RichText::new("remove").color(BAD)).clicked() {
                            let mut new_hosts = hosts.to_vec();
                            new_hosts.remove(index);
                            result = Some(new_hosts);
                            dialog.form = None;
                        }
                    });
                    ui.end_row();
                }
                for name in &from_command_line {
                    ui.label(RichText::new(*name).color(DIM));
                    ui.label(RichText::new("from --host (not saved)").color(DIM));
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                }
            });
    }
    ui.add_space(6.0);

    if dialog.form.is_none() && ui.button("add host").clicked() {
        dialog.form = Some(HostForm::default());
        dialog.test = None;
    }
    if let Some(form) = &mut dialog.form {
        ui.separator();
        ui.label(
            RichText::new(if form.index.is_some() {
                "Edit host"
            } else {
                "Add host"
            })
            .strong(),
        );
        egui::Grid::new("host_form")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                let fields = [
                    ("Name", &mut form.name, "e.g. zefat"),
                    (
                        "SSH destination",
                        &mut form.ssh,
                        "same as the name (or user@host)",
                    ),
                    ("Agent command", &mut form.command, DEFAULT_REMOTE_COMMAND),
                ];
                for (label, value, hint) in fields {
                    let label = ui.label(label);
                    ui.add(egui::TextEdit::singleline(value).hint_text(hint))
                        .labelled_by(label.id);
                    ui.end_row();
                }
            });
        let problem = form.problem(hosts, fixed);
        if let Some(problem) = &problem {
            ui.label(RichText::new(problem).color(WARN));
        }
        let mut close_form = false;
        ui.horizontal(|ui| {
            let previous = form.index.and_then(|i| hosts.get(i));
            if ui
                .add_enabled(problem.is_none(), egui::Button::new("save"))
                .clicked()
            {
                let host = form.to_host(previous);
                let mut new_hosts = hosts.to_vec();
                match form.index {
                    Some(index) if index < new_hosts.len() => new_hosts[index] = host,
                    _ => new_hosts.push(host),
                }
                result = Some(new_hosts);
                close_form = true;
            }
            let testing = dialog
                .test
                .as_ref()
                .is_some_and(|t| t.lock().map(|r| r.is_none()).unwrap_or(false));
            if ui
                .add_enabled(
                    problem.is_none() && !testing,
                    egui::Button::new("test connection"),
                )
                .clicked()
            {
                dialog.test = Some(start_test(ui.ctx(), &form.to_host(previous)));
            }
            if ui.button("cancel").clicked() {
                close_form = true;
            }
        });
        if let Some(test) = &dialog.test {
            match &*test.lock().unwrap_or_else(|e| e.into_inner()) {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("connecting…").color(DIM));
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
        if close_form {
            dialog.form = None;
            dialog.test = None;
        }
    }

    if let Some((message, ok)) = &dialog.message {
        ui.label(RichText::new(message).color(if *ok { OK } else { BAD }));
    }
    ui.separator();
    match config_path {
        Some(path) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Saved in {}", path.display())).color(DIM));
                if ui.small_button("open folder").clicked()
                    && let Some(folder) = path.parent()
                {
                    let _ = crate::config_file::open_folder(folder);
                }
            });
        }
        None => {
            ui.label(
                RichText::new(
                    "No configuration directory on this system: changes cannot be saved.",
                )
                .color(WARN),
            );
        }
    }
    result
}

/// Starts the agent through ssh in the background and reports what came back.
fn start_test(ctx: &egui::Context, host: &HostConfig) -> TestResult {
    let result: TestResult = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&result);
    let repaint = ctx.clone();
    let command = host
        .command
        .clone()
        .unwrap_or_else(|| DEFAULT_REMOTE_COMMAND.to_owned());
    let target = RemoteTarget::ssh(host.destination(), &command);
    thread::spawn(move || {
        let mut source = RemoteSource::new(target);
        let outcome = source
            .poll()
            .map(|snapshot| {
                format!(
                    "connected: {} ({}), {} processes",
                    snapshot.host.hostname, snapshot.host.os, snapshot.process_count
                )
            })
            .map_err(|err| err.to_string());
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
        repaint.request_repaint();
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_validation_and_conversion() {
        let hosts = vec![HostConfig::from_destination("zefat")];
        let fixed = vec!["local".to_owned()];
        let mut form = HostForm::default();
        assert_eq!(
            form.problem(&hosts, &fixed).as_deref(),
            Some("enter a name")
        );
        form.name = "zefat".to_owned();
        assert!(
            form.problem(&hosts, &fixed)
                .unwrap()
                .contains("already used")
        );
        form.name = "local".to_owned();
        assert!(
            form.problem(&hosts, &fixed).is_some(),
            "the local name is reserved"
        );
        form.name = " nas ".to_owned();
        form.ssh = "admin@nas.local".to_owned();
        assert!(form.problem(&hosts, &fixed).is_none());
        let host = form.to_host(None);
        assert_eq!(host.name, "nas");
        assert_eq!(host.ssh.as_deref(), Some("admin@nas.local"));
        assert_eq!(host.command, None, "empty means the default");

        // Editing keeps the host's own name and its section order.
        let mut existing = HostConfig::from_destination("zefat");
        existing.sections = Some(vec![rigtop_core::config::OverviewSection::Storage]);
        let edit = HostForm::from_host(0, &existing);
        assert!(edit.problem(&hosts, &fixed).is_none());
        assert_eq!(edit.to_host(Some(&existing)).sections, existing.sections);
    }
}
