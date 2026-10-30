//! The interface's colours, by what they mean.
//!
//! Chrome says four things with colour: something went well, something wants
//! a look, something went wrong, and something has a caveat. Each has one
//! colour, here, so a success notice, a finished render and a connected agent
//! are the same green, and changing what "warning" looks like is one line. The
//! warning and error colours are egui's own visuals, so they follow the theme;
//! success has no egui counterpart, so it is a constant.
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
