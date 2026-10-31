//! Updating rigtop: this machine's copy to a new release (`U`), a remote
//! host that runs an older version (`u`), or every such host (`u` in the
//! overview). Asks first, runs the updates in the background, then shows
//! how they went.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Wrap};
use rigtop_core::update::UpdateJob;

use super::ui::{BAD, OK, WARN, colored, dim, panel};

/// What is being updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The remote host at this index.
    Host(usize),
    /// This machine's copy, to this release.
    Local { version: String },
    /// These remote hosts, each in its own job.
    Hosts(Vec<usize>),
}

/// One host of a [`Target::Hosts`] update.
pub struct HostJob {
    pub index: usize,
    pub name: String,
    pub job: UpdateJob,
    /// Its outcome, once done.
    pub result: Option<Result<String, String>>,
}

/// The update window: asking, running, or showing the outcome.
pub enum UpdateWindow {
    Confirm {
        target: Target,
        text: String,
    },
    Running {
        target: Target,
        /// Where it runs, for the message ("zefat", "this machine").
        name: String,
        job: UpdateJob,
    },
    Done {
        target: Target,
        result: Result<String, String>,
    },
    /// Several hosts at once ([`Target::Hosts`]), running or done.
    Many {
        jobs: Vec<HostJob>,
    },
}

impl UpdateWindow {
    /// Draws the window in the middle of `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(4).min(76);
        let rows = match self {
            UpdateWindow::Many { jobs } => jobs.len() as u16 + 5,
            _ => 11,
        };
        let height = rows.max(11).min(area.height);
        if width < 20 || height < 5 {
            return;
        }
        let rect = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + (area.height - height) / 2,
            width,
            height,
        };
        let (color, lines) = match self {
            UpdateWindow::Confirm { text, .. } => (
                WARN,
                vec![
                    Line::from(text.clone()),
                    Line::from(""),
                    Line::from(dim("y: update    any other key: cancel")),
                ],
            ),
            UpdateWindow::Running { name, .. } => (
                WARN,
                vec![Line::from(format!("Updating rigtop on {name}…"))],
            ),
            UpdateWindow::Many { jobs } => {
                let done = jobs.iter().all(|j| j.result.is_some());
                let mut lines: Vec<Line> = jobs
                    .iter()
                    .map(|j| match &j.result {
                        None => Line::from(format!("{}: updating…", j.name)),
                        Some(Ok(message)) => {
                            Line::from(colored(format!("{}: {message}", j.name), OK))
                        }
                        Some(Err(error)) => {
                            Line::from(colored(format!("{}: not updated: {error}", j.name), BAD))
                        }
                    })
                    .collect();
                lines.push(Line::from(""));
                lines.push(Line::from(dim(if done {
                    "updated hosts reconnect    any key: close"
                } else {
                    "updated hosts reconnect as they finish"
                })));
                let failed = jobs.iter().any(|j| matches!(j.result, Some(Err(_))));
                (
                    if done && !failed {
                        OK
                    } else if failed {
                        BAD
                    } else {
                        WARN
                    },
                    lines,
                )
            }
            UpdateWindow::Done { target, result } => {
                let (color, text) = match (result, target) {
                    (Ok(message), Target::Host(_)) => {
                        (OK, format!("Updated: {message}. Reconnecting…"))
                    }
                    (Ok(message), Target::Local { .. }) => (
                        OK,
                        format!("Updated: {message}. Quit and start rigtop again to use it."),
                    ),
                    (Ok(message), Target::Hosts(_)) => (OK, format!("Updated: {message}.")),
                    (Err(error), _) => (BAD, format!("Not updated: {error}")),
                };
                (
                    color,
                    vec![
                        Line::from(colored(text, color)),
                        Line::from(""),
                        Line::from(dim("any key: close")),
                    ],
                )
            }
        };
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(panel("update", color)),
            rect,
        );
    }
}
