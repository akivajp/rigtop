//! Filled braille graph in the style of btop.
//!
//! Each terminal cell holds a 2x4 braille dot matrix, so a graph of `w x h`
//! cells shows `2w` samples with `4h` levels of vertical resolution. Rows are
//! colored by height (green at the bottom to red at the top by default),
//! which ratatui's `Chart` cannot do since it uses one color per dataset.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

/// Faint dots drawn along the base where a sample is zero, so that an idle
/// graph still reads as a graph rather than empty space.
const BASELINE: Color = Color::Rgb(60, 60, 72);

/// Bit for each dot of a braille cell, indexed by `[column][row from top]`.
const DOT_BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// A filled area graph of recent samples (oldest first, newest at the right edge).
pub struct BrailleGraph<'a> {
    samples: &'a [f64],
    max: f64,
    /// Grow downward from the top edge instead of upward (used for upload
    /// graphs mirrored under download graphs).
    inverted: bool,
    /// Maps the vertical position (0.0 bottom .. 1.0 top) to a color.
    color: fn(f64) -> Color,
}

impl<'a> BrailleGraph<'a> {
    /// Creates a graph scaled so that `max` fills the full height.
    pub fn new(samples: &'a [f64], max: f64) -> Self {
        Self {
            samples,
            max,
            inverted: false,
            color: heat_color,
        }
    }

    /// Draws the graph hanging from the top edge.
    pub fn inverted(mut self, inverted: bool) -> Self {
        self.inverted = inverted;
        self
    }

    /// Sets the height-to-color mapping.
    pub fn color(mut self, color: fn(f64) -> Color) -> Self {
        self.color = color;
        self
    }

    /// Number of samples needed to fill `width` cells.
    pub fn capacity(width: u16) -> usize {
        width as usize * 2
    }
}

impl Widget for BrailleGraph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.max <= 0.0 {
            return;
        }
        let dot_cols = Self::capacity(area.width);
        let dot_rows = area.height as usize * 4;
        // Right-align the samples: the newest sample sits at the right edge.
        let start = self.samples.len().saturating_sub(dot_cols);
        let visible = &self.samples[start..];
        let offset = dot_cols - visible.len();
        // Filled height of each dot column, in dots.
        let heights: Vec<usize> = (0..dot_cols)
            .map(|x| {
                x.checked_sub(offset).map_or(0, |i| {
                    let ratio = (visible[i] / self.max).clamp(0.0, 1.0);
                    let dots = (ratio * dot_rows as f64).round() as usize;
                    // Show any non-zero value as at least one dot.
                    if dots == 0 && visible[i] > 0.0 {
                        1
                    } else {
                        dots
                    }
                })
            })
            .collect();

        for cy in 0..area.height as usize {
            // Distance of this cell row from the graph's base, in cells.
            let level = if self.inverted {
                cy
            } else {
                area.height as usize - 1 - cy
            };
            let color = (self.color)((level as f64 + 0.5) / area.height as f64);
            for cx in 0..area.width as usize {
                let mut bits = 0u8;
                for (col, column_bits) in DOT_BITS.iter().enumerate() {
                    let height = heights[cx * 2 + col];
                    for (row, bit) in column_bits.iter().enumerate() {
                        // Dot index counted from the base of the graph.
                        let from_base = if self.inverted {
                            level * 4 + row
                        } else {
                            level * 4 + (3 - row)
                        };
                        if from_base < height {
                            bits |= bit;
                        }
                    }
                }
                let (bits, style) = if bits != 0 {
                    (bits, Style::default().fg(color))
                } else if level == 0 && cx * 2 + 1 >= offset {
                    // Baseline under columns that have samples: the dot row
                    // touching the base (bottom row, or top row when inverted).
                    let base_bits = if self.inverted {
                        0x01 | 0x08
                    } else {
                        0x40 | 0x80
                    };
                    (base_bits, Style::default().fg(BASELINE))
                } else {
                    continue;
                };
                let symbol = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
                buf[(area.x + cx as u16, area.y + cy as u16)]
                    .set_char(symbol)
                    .set_style(style);
            }
        }
    }
}

/// Green (low) -> yellow -> red (high), like btop's default CPU gradient.
pub fn heat_color(ratio: f64) -> Color {
    let ratio = ratio.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8, t: f64| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
    let (green, yellow, red) = ((80, 200, 80), (220, 200, 60), (230, 70, 70));
    let (from, to, t) = if ratio < 0.5 {
        (green, yellow, ratio * 2.0)
    } else {
        (yellow, red, (ratio - 0.5) * 2.0)
    };
    Color::Rgb(
        lerp(from.0, to.0, t),
        lerp(from.1, to.1, t),
        lerp(from.2, to.2, t),
    )
}

/// Light blue to purple, for network download graphs.
pub fn rx_color(ratio: f64) -> Color {
    let t = ratio.clamp(0.0, 1.0);
    Color::Rgb((90.0 + 110.0 * t) as u8, (160.0 - 60.0 * t) as u8, 230)
}

/// Teal to pink, for network upload graphs.
pub fn tx_color(ratio: f64) -> Color {
    let t = ratio.clamp(0.0, 1.0);
    Color::Rgb(
        (90.0 + 140.0 * t) as u8,
        (200.0 - 90.0 * t) as u8,
        (170.0 + 30.0 * t) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(samples: &[f64], max: f64, width: u16, height: u16, inverted: bool) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        BrailleGraph::new(samples, max)
            .inverted(inverted)
            .render(area, &mut buf);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn full_and_empty_columns() {
        // One cell, two samples: left column full, right column empty.
        let rows = render(&[1.0, 0.0], 1.0, 1, 1, false);
        assert_eq!(rows, vec!["⡇"]);
    }

    #[test]
    fn idle_graph_shows_a_baseline_only_where_samples_exist() {
        // Three cells, two zero samples: only the last cell has data.
        let rows = render(&[0.0, 0.0], 1.0, 3, 2, false);
        assert_eq!(rows, vec!["   ", "  ⣀"]);
        let rows = render(&[0.0, 0.0], 1.0, 3, 2, true);
        assert_eq!(rows, vec!["  ⠉", "   "]);
    }

    #[test]
    fn half_height_fills_bottom_cell_only() {
        let rows = render(&[0.5, 0.5], 1.0, 1, 2, false);
        assert_eq!(rows, vec![" ", "⣿"]);
    }

    #[test]
    fn inverted_graph_hangs_from_the_top() {
        let rows = render(&[0.5, 0.5], 1.0, 1, 2, true);
        assert_eq!(rows, vec!["⣿", " "]);
    }

    #[test]
    fn newest_samples_are_right_aligned() {
        // Two cells (4 dot columns), one sample: only the last dot column is drawn.
        let rows = render(&[1.0], 1.0, 2, 1, false);
        assert_eq!(rows, vec![" ⢸"]);
    }

    #[test]
    fn tiny_values_still_show_one_dot() {
        let rows = render(&[0.001, 0.0], 1.0, 1, 1, false);
        assert_eq!(rows, vec!["⡀"]);
    }

    #[test]
    fn heat_gradient_endpoints() {
        assert_eq!(heat_color(0.0), Color::Rgb(80, 200, 80));
        assert_eq!(heat_color(1.0), Color::Rgb(230, 70, 70));
    }
}
