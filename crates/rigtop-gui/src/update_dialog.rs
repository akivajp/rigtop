//! Updating rigtop: this machine's copy to a new release, or a remote host
//! that runs an older version. The window asks first, runs the update in
//! the background, then shows how it went; the title bar offers the update
//! and, once installed, a restart.

use std::path::Path;

use egui::{RichText, Ui};
use rigtop_core::hosts::HostKind;
use rigtop_core::update::{self, UpdateJob};

use crate::app::{HostEntry, ViewAction};
use crate::widgets::{BAD, DIM, OK, WARN};

/// What the title bar says about releases of rigtop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseNote {
    /// A newer release than this rigtop exists.
    Available(String),
    /// A newer release replaced this program; it runs after a restart.
    Installed(String),
}

/// "rigtop 0.12.1 · update…" or "restart for rigtop 0.12.1", if anything.
pub fn release_button(ui: &mut Ui, note: Option<&ReleaseNote>, actions: &mut Vec<ViewAction>) {
    let (text, hint, action) = match note {
        Some(ReleaseNote::Available(version)) => (
            format!("rigtop {version} · update…"),
            "a new release of rigtop is out: update this machine's copy",
            ViewAction::SelfUpdate,
        ),
        Some(ReleaseNote::Installed(version)) => (
            format!("restart for rigtop {version}"),
            "the new release is installed: restart rigtop to use it",
            ViewAction::Restart,
        ),
        None => return,
    };
    if ui
        .add(egui::Button::new(RichText::new(text).color(OK)).frame(false))
        .on_hover_text(hint)
        .clicked()
    {
        actions.push(action);
    }
}

/// "update agents (N)…": offers to update the `count` remote hosts that run
/// an older rigtop, when there are any.
pub fn agents_button(ui: &mut Ui, count: usize, actions: &mut Vec<ViewAction>) {
    if count == 0 {
        return;
    }
    if ui
        .add(
            egui::Button::new(RichText::new(format!("update agents ({count})…")).color(WARN))
                .frame(false),
        )
        .on_hover_text("update rigtop on every remote host that runs an older version")
        .clicked()
    {
        actions.push(ViewAction::UpdateAllAgents);
    }
}

/// What is being updated.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// The remote host at this index.
    Host(usize),
    /// This machine's copy, to this release.
    Local(String),
    /// These remote hosts, each in its own job.
    Hosts(Vec<usize>),
}

/// One host of a [`Target::Hosts`] update: its index, name, job and, once
/// done, the outcome.
type HostJob = (usize, String, UpdateJob, Option<Result<String, String>>);

/// Where the window is.
enum Step {
    Confirm,
    Running(UpdateJob),
    Done(Result<String, String>),
    /// Several hosts, running or done.
    Many(Vec<HostJob>),
}

/// What the app should do after a frame of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateEvent {
    /// The remote host at this index was updated: reconnect to it.
    Reconnect(usize),
    /// This release replaced this program.
    Installed(String),
    /// Restart now to run the new release.
    Restart,
}

/// The update window, for one target at a time.
#[derive(Default)]
pub struct UpdateDialog {
    /// The target, its name for messages and the step, while open.
    open: Option<(Target, String, Step)>,
}

impl UpdateDialog {
    /// Asks whether to update the remote host at `index`.
    pub fn ask(&mut self, index: usize, name: &str) {
        self.open_on(Target::Host(index), name);
    }

    /// Asks whether to update the remote hosts at `indexes` (those running
    /// an older rigtop).
    pub fn ask_all(&mut self, indexes: Vec<usize>) {
        let name = format!("{} hosts", indexes.len());
        self.open_on(Target::Hosts(indexes), &name);
    }

    /// Asks whether to update this machine's copy to `version`.
    pub fn ask_self(&mut self, version: &str) {
        self.open_on(Target::Local(version.to_owned()), "this machine");
    }

    fn open_on(&mut self, target: Target, name: &str) {
        // A running update keeps its window.
        let running = match &self.open {
            Some((_, _, Step::Running(_))) => true,
            Some((_, _, Step::Many(jobs))) => jobs.iter().any(|j| j.3.is_none()),
            _ => false,
        };
        if !running {
            self.open = Some((target, name.to_owned(), Step::Confirm));
        }
    }

    /// Draws the window. `exe` is the running program (taken at startup).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        hosts: &[HostEntry],
        exe: Option<&Path>,
    ) -> Vec<UpdateEvent> {
        let mut events = Vec::new();
        let Some((target, name, step)) = self.open.as_mut() else {
            return events;
        };
        let (target, name) = (target.clone(), name.clone());
        if let Step::Running(job) = step
            && let Some(result) = job.poll()
        {
            if result.is_ok() {
                events.push(match &target {
                    Target::Host(index) => UpdateEvent::Reconnect(*index),
                    Target::Local(version) => UpdateEvent::Installed(version.clone()),
                    Target::Hosts(_) => unreachable!("runs as Step::Many"),
                });
            }
            *step = Step::Done(result);
        }
        // Each host reconnects as soon as its update is done.
        if let Step::Many(jobs) = step {
            for (index, _, job, outcome) in jobs.iter_mut().filter(|j| j.3.is_none()) {
                if let Some(result) = job.poll() {
                    if result.is_ok() {
                        events.push(UpdateEvent::Reconnect(*index));
                    }
                    *outcome = Some(result);
                }
            }
        }
        let version_of = |index: usize| {
            hosts
                .get(index)
                .and_then(|h| h.state.snapshot.as_ref())
                .map(|s| (s.host.rigtop_version.clone(), s.host.os.clone()))
                .unwrap_or_default()
        };
        let (agent_version, os) = match &target {
            Target::Host(index) => version_of(*index),
            Target::Local(_) | Target::Hosts(_) => Default::default(),
        };
        let unsupported = match &target {
            Target::Host(_) => update::unsupported_reason(&os),
            Target::Local(_) => update::self_update_unsupported(),
            Target::Hosts(_) => None,
        };
        let mut close = false;
        let mut start = false;
        let mut restart = false;
        let screen = ctx.content_rect();
        egui::Window::new("Update rigtop")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .show(ctx, |ui| match step {
                Step::Confirm => match &unsupported {
                    Some(reason) => {
                        ui.label(format!("{name} cannot be updated from here: {reason}."));
                        close = ui.button("close").clicked();
                    }
                    None => {
                        ui.label(match &target {
                            Target::Host(_) => update::describe(&name, &agent_version),
                            Target::Local(version) => update::describe_self(version),
                            Target::Hosts(indexes) => {
                                let names: Vec<(String, String)> = indexes
                                    .iter()
                                    .filter_map(|&i| {
                                        Some((hosts.get(i)?.spec.name.clone(), version_of(i).0))
                                    })
                                    .collect();
                                update::describe_many(&names)
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            start = ui.button("update").clicked();
                            close = ui.button("cancel").clicked();
                        });
                    }
                },
                Step::Running(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Updating rigtop on {name}…"));
                    });
                    // The job finishes on another thread; look again soon.
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(250));
                }
                Step::Many(jobs) => {
                    for (_, host, _, result) in jobs.iter() {
                        ui.horizontal(|ui| match result {
                            None => {
                                ui.spinner();
                                ui.label(format!("{host}: updating…"));
                            }
                            Some(Ok(message)) => {
                                ui.label(RichText::new(format!("{host}: {message}")).color(OK));
                            }
                            Some(Err(error)) => {
                                ui.label(
                                    RichText::new(format!("{host}: not updated: {error}"))
                                        .color(BAD),
                                );
                            }
                        });
                    }
                    if jobs.iter().all(|j| j.3.is_some()) {
                        ui.label(RichText::new("Updated hosts reconnect.").color(DIM));
                        close = ui.button("close").clicked();
                    } else {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(250));
                    }
                }
                Step::Done(result) => {
                    outcome(ui, &target, result);
                    ui.horizontal(|ui| {
                        if result.is_ok() && matches!(target, Target::Local(_)) {
                            restart = ui.button("restart now").clicked();
                            close = ui.button("later").clicked();
                        } else {
                            close = ui.button("close").clicked();
                        }
                    });
                }
            });
        if start {
            *step = match &target {
                Target::Hosts(indexes) => Step::Many(
                    indexes
                        .iter()
                        .filter_map(|&i| match &hosts.get(i)?.spec.kind {
                            HostKind::Remote(remote) => Some((
                                i,
                                hosts[i].spec.name.clone(),
                                UpdateJob::start(remote),
                                None,
                            )),
                            _ => None,
                        })
                        .collect(),
                ),
                Target::Host(index) => match hosts.get(*index).map(|h| &h.spec.kind) {
                    Some(HostKind::Remote(remote)) => Step::Running(UpdateJob::start(remote)),
                    _ => Step::Done(Err(format!("{name} is not a remote host"))),
                },
                Target::Local(version) => match exe {
                    Some(exe) => Step::Running(UpdateJob::start_self(version, exe)),
                    None => Step::Done(Err("the running program was not found".to_owned())),
                },
            };
        }
        if close || restart {
            self.open = None;
        }
        if restart {
            events.push(UpdateEvent::Restart);
        }
        events
    }
}

fn outcome(ui: &mut Ui, target: &Target, result: &Result<String, String>) {
    match result {
        Ok(message) => {
            ui.label(RichText::new(format!("Updated: {message}.")).color(OK));
            let next = match target {
                Target::Host(_) | Target::Hosts(_) => "Reconnecting to the new version…",
                Target::Local(_) => "Restart rigtop to use it.",
            };
            ui.label(RichText::new(next).color(DIM));
        }
        Err(error) => {
            ui.label(RichText::new(format!("Not updated: {error}")).color(BAD));
        }
    }
}
