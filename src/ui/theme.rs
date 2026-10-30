//! The interface's colours, by what they mean.
//!
//! Chrome says four things with colour: something went well, something wants
//! a look, something went wrong, and something has a caveat. Each has one
//! colour, here, so a success notice, a finished render and a connected agent
//! are the same green, and changing what "warning" looks like is one line. The
//! warning and error colours are egui's own visuals, so they follow the theme;
//! success has no egui counterpart, so it is a constant.
//!
//! The fifth is the selection colour (below): "this is the one". It is a
//! preference, and every selected thing in the interface reads it from here.
//!
//! Not for domain colours: the timeline's track classes, the xaos heatmap and
//! a transform's swatch are about the scene, not about how the chrome is
//! feeling, and they live with what they draw.

use egui::{Color32, Visuals};

/// Done, connected, under budget. The render dialog's finished panel, a
/// notice that something was sent or kept, a frametime that makes 60 fps.
pub const SUCCESS: Color32 = Color32::from_rgb(90, 220, 120);

/// A caveat on a value rather than a problem: a matrix that is sheared, a
/// frametime over budget. Distinct from [`warn`], which asks for attention.
pub const CAUTION: Color32 = Color32::from_rgb(230, 175, 60);

/// Wants a look: waiting on you, partial, paused, about to overwrite.
pub fn warn(v: &Visuals) -> Color32 {
    v.warn_fg_color
}

/// Something failed or can't be done.
pub fn error(v: &Visuals) -> Color32 {
    v.error_fg_color
}

/// Record is armed: the record button's dot. Red because every recorder
/// since tape has made it red, and the same red as [`error`] so there is one
/// red in the interface. Nothing else uses it; a recording isn't a failure.
pub fn recording(v: &Visuals) -> Color32 {
    error(v)
}

// === The selection colour ===
//
// One colour says "this is the one": the chosen segment of a radio, a lit
// toggle, the selected row of any list, the selected key, the filled part of
// a progress bar. It is egui's `visuals.selection`, so egui's own widgets
// (selectable labels, toggles, text selection) and ours agree, and it is a
// preference (Settings: "highlight colour"), applied by `ui::draw` when it
// changes. Anything painted by hand reads it through the helpers below rather
// than mixing its own shade, so changing the preference changes all of it.

/// The selection colour egui ships with, and ours until someone picks another.
pub const DEFAULT_SELECTION: Color32 = Color32::from_rgb(0, 92, 128);

/// The whole `Selection` for a chosen fill: the fill, and a pale stroke made
/// from it (selected text, outlines on a filled selection). For egui's own
/// fill this is egui's own stroke exactly, so the default changes nothing.
pub fn selection_for(fill: Color32) -> egui::style::Selection {
    let default = Visuals::dark().selection;
    if fill == DEFAULT_SELECTION {
        return default;
    }
    let pale = fill.lerp_to_gamma(Color32::WHITE, 0.75);
    egui::style::Selection { bg_fill: fill, stroke: egui::Stroke::new(default.stroke.width, pale) }
}

/// A selected row's backing, under its text: the selection colour at half
/// strength, so the text on it stays readable.
pub fn selected_tint(v: &Visuals) -> Color32 {
    v.selection.bg_fill.gamma_multiply(0.5)
}

/// The edge drawn round something selected. Brighter than the fill, so a
/// selection reads as an outlined box (a clear edge, not just a shade) on
/// the dark panels and the near-black lanes alike.
pub fn selected_edge(v: &Visuals) -> Color32 {
    v.selection.bg_fill.lerp_to_gamma(v.selection.stroke.color, 0.35)
}

/// Paint a selected row: the tint, and a 1px edge inside it.
pub fn paint_selected(painter: &egui::Painter, rect: egui::Rect, corner: f32, v: &Visuals) {
    painter.rect_filled(rect, corner, selected_tint(v));
    painter.rect_stroke(rect, corner, egui::Stroke::new(1.0, selected_edge(v)), egui::StrokeKind::Inside);
}
