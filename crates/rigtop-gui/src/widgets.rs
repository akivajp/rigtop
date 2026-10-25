//! Drawing primitives in the style of btop: colors, filled area graphs and
//! segmented meters, painted directly with egui's painter.

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};

/// Window background.
pub const BACKGROUND: Color32 = Color32::from_rgb(22, 22, 30);
/// Panel border.
pub const BORDER: Color32 = Color32::from_rgb(70, 70, 88);
/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(215, 215, 225);
/// Secondary text (labels, units, commands).
pub const DIM: Color32 = Color32::from_rgb(125, 125, 140);
/// Unfilled meter segments and graph baselines.
pub const EMPTY: Color32 = Color32::from_rgb(52, 52, 64);
/// Title colors per panel (same as the TUI).
pub const CPU_TITLE: Color32 = Color32::from_rgb(120, 210, 120);
pub const MEM_TITLE: Color32 = Color32::from_rgb(230, 200, 90);
pub const GPU_TITLE: Color32 = Color32::from_rgb(200, 130, 230);
pub const STORAGE_TITLE: Color32 = Color32::from_rgb(90, 200, 220);
pub const NET_TITLE: Color32 = Color32::from_rgb(110, 150, 240);
pub const PROC_TITLE: Color32 = Color32::from_rgb(240, 150, 90);
pub const WIDGET_TITLE: Color32 = Color32::from_rgb(225, 140, 185);
/// Status colors.
pub const OK: Color32 = Color32::from_rgb(110, 200, 110);
pub const WARN: Color32 = Color32::from_rgb(230, 200, 70);
pub const BAD: Color32 = Color32::from_rgb(235, 80, 80);
/// Selected table row.
pub const SELECTION: Color32 = Color32::from_rgb(45, 55, 80);

/// Green (low) -> yellow -> red (high), like btop's default gradient.
pub fn heat_color(ratio: f32) -> Color32 {
    let ratio = ratio.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    let (green, yellow, red) = ((80, 200, 80), (220, 200, 60), (230, 70, 70));
    let (from, to, t) = if ratio < 0.5 {
        (green, yellow, ratio * 2.0)
    } else {
        (yellow, red, (ratio - 0.5) * 2.0)
    };
    Color32::from_rgb(
        lerp(from.0, to.0, t),
        lerp(from.1, to.1, t),
        lerp(from.2, to.2, t),
    )
}

/// Light blue to purple, for download graphs.
pub fn rx_color(ratio: f32) -> Color32 {
    let t = ratio.clamp(0.0, 1.0);
    Color32::from_rgb((90.0 + 110.0 * t) as u8, (160.0 - 60.0 * t) as u8, 230)
}

/// Teal to pink, for upload graphs.
pub fn tx_color(ratio: f32) -> Color32 {
    let t = ratio.clamp(0.0, 1.0);
    Color32::from_rgb(
        (90.0 + 140.0 * t) as u8,
        (200.0 - 90.0 * t) as u8,
        (170.0 + 30.0 * t) as u8,
    )
}

/// Color for a percentage (0 green .. 100 red).
pub fn percent_color(percent: f32) -> Color32 {
    heat_color(percent / 100.0)
}

/// Color for a temperature: green below ~40 °C, red from ~85 °C.
pub fn temperature_color(celsius: f32) -> Color32 {
    heat_color((celsius - 40.0) / 45.0)
}

/// Filled area graph of `samples` (oldest first) scaled to `max`, newest at
/// the right edge. The fill is colored by height with `color`.
///
/// `inverted` draws the graph hanging from the top (upload under download).
pub fn area_graph(
    ui: &mut Ui,
    size: Vec2,
    samples: &[f64],
    max: f64,
    color: fn(f32) -> Color32,
    inverted: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    paint_area_graph(ui, rect, samples, max, color, inverted);
    response
}

/// Paints an area graph into `rect` (see [`area_graph`]).
pub fn paint_area_graph(
    ui: &Ui,
    rect: Rect,
    samples: &[f64],
    max: f64,
    color: fn(f32) -> Color32,
    inverted: bool,
) {
    let painter = ui.painter_at(rect);
    // Baseline so an idle graph still reads as a graph.
    let base_y = if inverted { rect.top() } else { rect.bottom() };
    painter.line_segment(
        [pos2(rect.left(), base_y), pos2(rect.right(), base_y)],
        Stroke::new(1.0, EMPTY),
    );
    if samples.len() < 2 || max <= 0.0 || rect.width() < 2.0 {
        return;
    }
    // One sample every STEP points; only as many as fit, newest on the right.
    const STEP: f32 = 2.0;
    let capacity = (rect.width() / STEP) as usize + 1;
    let visible = &samples[samples.len().saturating_sub(capacity)..];
    let x_of = |i: usize| rect.right() - (visible.len() - 1 - i) as f32 * STEP;
    let ratio_of = |v: f64| (v / max).clamp(0.0, 1.0) as f32;
    let y_of = |ratio: f32| {
        if inverted {
            rect.top() + ratio * rect.height()
        } else {
            rect.bottom() - ratio * rect.height()
        }
    };

    // A triangle strip between the curve and the base. Vertex colors make
    // the gradient: the base is the "low" color, the curve takes the color
    // of its own height.
    let mut mesh = Mesh::default();
    for (i, &value) in visible.iter().enumerate() {
        let ratio = ratio_of(value);
        let x = x_of(i);
        let base = mesh.vertices.len() as u32;
        mesh.vertices
            .push(vertex(pos2(x, base_y), color(0.0).gamma_multiply(0.6)));
        mesh.vertices
            .push(vertex(pos2(x, y_of(ratio)), color(ratio)));
        if i > 0 {
            mesh.add_triangle(base - 2, base - 1, base);
            mesh.add_triangle(base - 1, base, base + 1);
        }
    }
    painter.add(mesh);
}

fn vertex(pos: Pos2, color: Color32) -> Vertex {
    Vertex {
        pos,
        uv: WHITE_UV,
        color,
    }
}

/// Paints a horizontal meter: a rounded track with a smooth fill whose
/// color follows btop's gradient along the full length (green at the left,
/// red at the right end), so a fuller meter also reads "hotter".
/// A meter filled in one color (for amounts left, where full is good).
pub fn paint_solid_meter(ui: &Ui, rect: Rect, ratio: f32, color: Color32) {
    let painter = ui.painter_at(rect);
    let bar_height = (rect.height() * 0.6).clamp(3.0, 10.0);
    let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), bar_height));
    let radius = bar_height / 2.0;
    painter.rect_filled(bar, radius, EMPTY);
    let fill_width = bar.width() * ratio.clamp(0.0, 1.0);
    if fill_width >= 0.5 {
        let fill = Rect::from_min_size(bar.min, vec2(fill_width.max(bar_height), bar_height));
        painter.rect_filled(fill, radius, color);
    }
}

pub fn paint_meter(ui: &Ui, rect: Rect, ratio: f32) {
    let painter = ui.painter_at(rect);
    let bar_height = (rect.height() * 0.6).clamp(3.0, 10.0);
    let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), bar_height));
    let radius = bar_height / 2.0;
    painter.rect_filled(bar, radius, EMPTY);

    let ratio = ratio.clamp(0.0, 1.0);
    let fill_width = bar.width() * ratio;
    if fill_width < 0.5 {
        return;
    }
    // A strip of quads with per-vertex colors gives a smooth gradient. The
    // fill is drawn as a capsule: half-discs at both ends, gradient between.
    const SLICES: usize = 24;
    let mut mesh = Mesh::default();
    let left = bar.left() + radius.min(fill_width / 2.0);
    let right = (bar.left() + fill_width - radius).max(left);
    for i in 0..=SLICES {
        let t = i as f32 / SLICES as f32;
        let x = left + (right - left) * t;
        // Color by position along the whole meter, not just the filled part.
        let color = heat_color((x - bar.left()) / bar.width());
        let index = mesh.vertices.len() as u32;
        mesh.vertices.push(vertex(pos2(x, bar.top()), color));
        mesh.vertices.push(vertex(pos2(x, bar.bottom()), color));
        if i > 0 {
            mesh.add_triangle(index - 2, index - 1, index);
            mesh.add_triangle(index - 1, index, index + 1);
        }
    }
    painter.add(mesh);
    let cap_radius = radius.min(fill_width / 2.0);
    painter.circle_filled(pos2(left, bar.center().y), cap_radius, heat_color(0.0));
    painter.circle_filled(
        pos2(right, bar.center().y),
        cap_radius,
        heat_color((right - bar.left()) / bar.width()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_gradient_endpoints() {
        assert_eq!(heat_color(0.0), Color32::from_rgb(80, 200, 80));
        assert_eq!(heat_color(1.0), Color32::from_rgb(230, 70, 70));
        assert_eq!(heat_color(2.0), heat_color(1.0));
    }
}
