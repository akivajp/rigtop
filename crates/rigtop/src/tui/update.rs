//! Updating rigtop: this machine's copy to a new release (`U`), or a remote
//! host that runs an older version (`u`). Asks first, runs the update in
//! the background, then shows how it went.

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
}

impl UpdateWindow {
    /// Draws the window in the middle of `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(4).min(76);
        let height = 11.min(area.height);
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
            UpdateWindow::Done { target, result } => {
                let (color, text) = match (result, target) {
                    (Ok(message), Target::Host(_)) => {
                        (OK, format!("Updated: {message}. Reconnecting…"))
                    }
                    (Ok(message), Target::Local { .. }) => (
                        OK,
                        format!("Updated: {message}. Quit and start rigtop again to use it."),
                    ),
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
