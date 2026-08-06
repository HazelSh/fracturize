//! Status-bar hover hints: the bottom bar's left-hand context string,
//! resolved fresh every frame per the plan's three-tier precedence:
//! 1. a widget opted in via [`hinted`] and is currently hovered (panels draw
//!    before the status bar, so this frame's hover already landed);
//! 2. else, if the pointer isn't over egui at all, a gizmo-part hint from
//!    `App::hovered_hint`;
//! 3. else the viewport default ([`HINT_VIEWPORT`]).

use super::UiState;

/// Dragging a gizmo's origin dot
pub const HINT_ORIGIN: &str =
    "drag: move in view plane (shift: fine) · ctrl+drag: uniform scale · alt+scroll: chaos weight";
/// Dragging an origin→axis gizmo edge
pub const HINT_AXIS: &str =
    "drag: slide along axis (shift: fine) · ctrl+drag: uniform scale · alt+scroll: chaos weight";
/// Dragging an outer gizmo edge
pub const HINT_ROT_EDGE: &str =
    "drag: rotate around axis (shift: fine, alt: snap 15°) · alt+scroll: chaos weight";
/// Nothing hinted, nothing hovered: the bare-viewport default
pub const HINT_VIEWPORT: &str =
    "drag: orbit · shift/middle-drag: pan · right-drag: roll · scroll: zoom";

/// Wrap a widget response so that, while hovered, it both shows a tooltip
/// and sets this frame's status-bar hint. Every interactive Phase 2+ control
/// should route its response through this (only exception, per the plan: a
/// bare camera drag/zoom on empty viewport space, which has no widget to
/// hang a hint off of and instead falls back to `HINT_VIEWPORT`).
pub fn hinted(
    resp: egui::Response,
    ui_state: &mut UiState,
    tooltip: impl Into<egui::WidgetText>,
    hint: &str,
) -> egui::Response {
    // `contains_pointer` rather than `hovered`: egui never reports a disabled
    // widget as hovered, and a greyed control is precisely when you most want
    // to be told what it is and why you can't have it yet.
    if resp.contains_pointer() {
        ui_state.status_hint = Some(hint.to_string());
    }
    // Enabled and disabled tooltips are separate paths in egui, and
    // `on_hover_text` alone silently shows nothing once a widget is greyed —
    // so every "disabled rather than hidden, because it says why" control in
    // this UI was in fact saying nothing at all. Attach both.
    let tooltip = tooltip.into();
    resp.on_hover_text(tooltip.clone()).on_disabled_hover_text(tooltip)
}
