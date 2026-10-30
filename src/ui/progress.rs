//! Progress bars in the house shape.
//!
//! egui's `ProgressBar` is a pill: fully rounded ends, no border, a block of
//! colour floating on the panel. That is the touch-first look this interface
//! doesn't use (UI-GUIDELINES.md §3, "Shape"): a control here is a
//! rounded-square box with an edge you can see. So the bar is drawn here, as
//! one: the window's corner radius, a recessed trough with a 1px border, and
//! the done part filled in the selection colour, clipped to the same box so
//! its corners match the trough's at every fraction.
//!
//! It never animates. A bar with no fraction to show says so in its caption
//! instead of pulsing.

/// How a bar's fill reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Working: the selection colour.
    Running,
    /// Paused or stopping: wants a look ([`super::theme::warn`]).
    Held,
}

/// A full-width bar at `fraction` (0–1) with `text` centred on it. The text
/// should come from a fixed-width source (`ui::num`), since it changes every
/// frame.
pub fn bar(ui: &mut egui::Ui, fraction: f32, text: impl Into<egui::WidgetText>, tone: Tone) -> egui::Response {
    let height = ui.spacing().interact_size.y + 2.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let v = ui.visuals();
    let corner = v.window_corner_radius;
    let fill = match tone {
        Tone::Running => v.selection.bg_fill,
        Tone::Held => super::theme::warn(v).gamma_multiply(0.6),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, corner, v.extreme_bg_color);
    let done = fraction.clamp(0.0, 1.0);
    if done > 0.0 {
        // The whole box in the fill colour, clipped at the fraction: the left
        // corners stay the trough's, and the leading edge is a straight cut.
        let mut cut = rect;
        cut.max.x = rect.min.x + rect.width() * done;
        painter.with_clip_rect(cut).rect_filled(rect, corner, fill);
    }
    painter.rect_stroke(rect, corner, v.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);

    let galley = text.into().into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
    let at = rect.center() - galley.size() / 2.0;
    ui.painter().galley(at, galley, ui.visuals().strong_text_color());
    resp
}
