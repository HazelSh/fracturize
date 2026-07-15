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
    "drag: move in view plane · ctrl+drag: uniform scale · scroll: chaos weight";
/// Dragging an origin→axis gizmo edge
pub const HINT_AXIS: &str =
    "drag: slide along axis · ctrl+drag: uniform scale · scroll: chaos weight";
/// Dragging an outer gizmo edge
pub const HINT_ROT_EDGE: &str =
    "drag: rotate around axis · ctrl+drag: uniform scale · scroll: chaos weight";
/// Nothing hinted, nothing hovered: the bare-viewport default
pub const HINT_VIEWPORT: &str = "drag: orbit · shift/middle-drag: pan · scroll: zoom";

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
    if resp.hovered() {
        ui_state.status_hint = Some(hint.to_string());
    }
    resp.on_hover_text(tooltip)
}
