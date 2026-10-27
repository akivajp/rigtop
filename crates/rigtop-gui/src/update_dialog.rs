//! A window for updating rigtop on a remote host that runs an older
//! version: asks first, runs the update over SSH in the background, then
//! shows how it went.

use egui::{RichText, Ui};
use rigtop_core::hosts::HostKind;
use rigtop_core::update::{self, UpdateJob};

use crate::app::HostEntry;
use crate::widgets::{BAD, DIM, OK};

/// Where the window is.
enum Step {
    Confirm,
    Running(UpdateJob),
    Done(Result<String, String>),
}

/// The update window, for one host at a time.
#[derive(Default)]
pub struct UpdateDialog {
    /// The host (index and name) and the step, while the window is open.
    open: Option<(usize, String, Step)>,
}

impl UpdateDialog {
    /// Asks whether to update the host at `index`.
    pub fn ask(&mut self, index: usize, name: &str) {
        // A running update keeps its window.
        if !matches!(self.open, Some((_, _, Step::Running(_)))) {
            self.open = Some((index, name.to_owned(), Step::Confirm));
        }
    }

    /// Draws the window. Returns the index of a host whose update just
    /// finished well, to reconnect to it.
    pub fn show(&mut self, ctx: &egui::Context, hosts: &[HostEntry]) -> Option<usize> {
        let (index, name, step) = self.open.as_mut()?;
        let (index, name) = (*index, name.clone());
        let mut updated = None;
        if let Step::Running(job) = step
            && let Some(result) = job.poll()
        {
            if result.is_ok() {
                updated = Some(index);
            }
            *step = Step::Done(result);
        }
        let (agent_version, os) = hosts
            .get(index)
            .and_then(|h| h.state.snapshot.as_ref())
            .map(|s| (s.host.rigtop_version.clone(), s.host.os.clone()))
            .unwrap_or_default();
        let mut close = false;
        let mut start = false;
        let screen = ctx.content_rect();
        egui::Window::new("Update rigtop")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .show(ctx, |ui| match step {
                Step::Confirm => match update::unsupported_reason(&os) {
                    Some(reason) => {
                        ui.label(format!("{name} cannot be updated from here: {reason}."));
                        close = ui.button("close").clicked();
                    }
                    None => {
                        ui.label(update::describe(&name, &agent_version));
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
                Step::Done(result) => {
                    outcome(ui, result);
                    close = ui.button("close").clicked();
                }
            });
        if start {
            *step = match hosts.get(index).map(|h| &h.spec.kind) {
                Some(HostKind::Remote(target)) => Step::Running(UpdateJob::start(target)),
                _ => Step::Done(Err(format!("{name} is not a remote host"))),
            };
        }
        if close {
            self.open = None;
        }
        updated
    }
}

fn outcome(ui: &mut Ui, result: &Result<String, String>) {
    match result {
        Ok(message) => {
            ui.label(RichText::new(format!("Updated: {message}.")).color(OK));
            ui.label(RichText::new("Reconnecting to the new version…").color(DIM));
        }
        Err(error) => {
            ui.label(RichText::new(format!("Not updated: {error}")).color(BAD));
        }
    }
}
