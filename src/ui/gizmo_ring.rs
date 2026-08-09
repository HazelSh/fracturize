//! The roll ring: the selected transform's rotate-against-the-camera control.
//!
//! Painted in screen space through egui rather than built as world geometry,
//! and that is the honest choice rather than the cheap one. The view plane is a
//! screen-space idea with no world shape to be faithful to, so a screen-space
//! circle isn't an approximation of anything — it *is* the control. Two
//! practical consequences fall out of it: the ring is exactly screen-constant
//! (grabbable around a transform that is tiny on screen, not absurd around a
//! huge one), and it costs no vertex buffer churn. `indicators.rs` rebuilds
//! only when the *matrix* changes, so a camera-facing ring built there would
//! have sat still while the camera orbited around it.
//!
//! Dashed at rest, solid while held. Dashing is what says "this is interface,
//! not an edge of the tetrahedron" — the three local-axis rotate edges are
//! solid and coloured, and the ring must not read as a fourth one. It is the
//! one part whose held state *drops* its distinguishing mark rather than
//! gaining one, which is exactly how a ring reads as engaged.
//!
//! Same layer as `labels.rs`: `Order::Background`, under the panels, because
//! this is viewport decoration and must not paint across a window's interior.

use crate::app::App;

/// Neutral, so it doesn't compete with the six hues already spent on the
/// tetrahedron's axes and faces — and so it reads as chrome rather than as part
/// of the shape.
const IDLE: egui::Color32 = egui::Color32::from_rgb(150, 152, 160);
const ACTIVE: egui::Color32 = egui::Color32::from_rgb(236, 238, 245);

/// Segments in the drawn circle. Enough that the dashes read as arcs rather
/// than as a polygon at the sizes this is drawn at.
const SEGMENTS: usize = 96;

pub fn draw(ctx: &egui::Context, app: &App) {
    if !app.show_gizmos {
        return;
    }
    let Some(idx) = app.selected_transform() else { return };
    let Some(spec) = app.scene.transforms.get(idx) else { return };

    let (w, h) = app.surface_size();
    if w == 0 || h == 0 {
        return;
    }
    let Some((centre, radius)) =
        crate::pick::roll_ring(spec.matrix, app.current_view_proj(), w as f32, h as f32)
    else {
        return;
    };

    // Projection is in physical pixels; egui works in logical points.
    let ppp = ctx.pixels_per_point();
    let c = egui::pos2(centre.0 / ppp, centre.1 / ppp);
    let r = radius / ppp;

    let held = app.rolling();
    let hovered = app.hovering_roll();
    let color = if held || hovered { ACTIVE } else { IDLE };
    let width = if held { 2.0 } else { 1.0 };

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("fracturize_roll_ring"),
    ));
    let stroke = egui::Stroke::new(width, color);

    let points: Vec<egui::Pos2> = (0..=SEGMENTS)
        .map(|i| {
            let t = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
            egui::pos2(c.x + r * t.cos(), c.y + r * t.sin())
        })
        .collect();

    if held {
        // Solid: you have hold of it, and the dashes have done their job.
        painter.add(egui::Shape::line(points, stroke));
    } else {
        painter.add(egui::Shape::dashed_line(&points, stroke, 6.0, 6.0));
    }
}
