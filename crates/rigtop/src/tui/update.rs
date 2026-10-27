//! Updating rigtop on a remote host that runs an older version (`u`):
//! asks first, runs the update over SSH in the background, then shows how
//! it went.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Wrap};
use rigtop_core::update::UpdateJob;

use super::ui::{BAD, OK, WARN, colored, dim, panel};

/// The update window: asking, running, or showing the outcome.
pub enum UpdateWindow {
    Confirm {
        host: usize,
        text: String,
    },
    Running {
        host: usize,
        name: String,
        job: UpdateJob,
    },
    Done {
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
        let (title, color, lines) = match self {
            UpdateWindow::Confirm { text, .. } => (
                "update",
                WARN,
                vec![
                    Line::from(text.clone()),
                    Line::from(""),
                    Line::from(dim("y: update    any other key: cancel")),
                ],
            ),
            UpdateWindow::Running { name, .. } => (
                "update",
                WARN,
                vec![Line::from(format!("Updating rigtop on {name}…"))],
            ),
            UpdateWindow::Done { result } => {
                let (color, text) = match result {
                    Ok(message) => (OK, format!("Updated: {message}. Reconnecting…")),
                    Err(error) => (BAD, format!("Not updated: {error}")),
                };
                (
                    "update",
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
                .block(panel(title, color)),
            rect,
        );
    }
}
