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

/// Desaturated, so it doesn't compete with the hues spent on the tetrahedron's
/// axes — and separated from everything else neutral in the viewport by
/// *lightness* rather than hue. The reference tetrahedron is a mid grey and the
/// scene carries a pale warm line through the origin; a control that reads as
/// either of those is a control you have to think about. This sits above both,
/// faintly cool, and the dashes finish the job.
///
/// It is now *the* interface neutral rather than a colour of its own: the
/// gizmo's shafts fade to this same value at the origin, where they belong to
/// no axis. One neutral shared by everything that isn't an axis says something;
/// three near-identical greys said only that nobody had compared them.
const IDLE: egui::Color32 = crate::palette::axes::neutral_color32();
const ACTIVE: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);

/// Dashes around the ring, and how much of each dash's slot is drawn.
///
/// **Fixed in angle, not in arc length**, and that is the whole point. egui's
/// `dashed_line` measures dashes along the polyline in pixels, so as the ring's
/// radius changes — which it does constantly while the camera flies a path, as
/// the transform moves through 3-space — the circumference changes, the number
/// of dashes that fit changes, and the pattern slides around the seam where the
/// polyline closes. The result reads as the ring *spinning*, which is a motion
/// the app is not making and an outright lie about a rotation control.
///
/// Anchoring each dash to a fixed angle removes the failure at the source:
/// dash *k* always occupies the same wedge, so changing the radius scales the
/// dashes and moves nothing. There is no seam, because there is no running
/// arc-length to accumulate error in.
const DASHES: usize = 48;
const DASH_FILL: f32 = 0.55;
/// Points per dash. Three is enough for a dash spanning a fraction of a
/// forty-eighth of a turn to read as curved rather than straight.
const DASH_POINTS: usize = 3;

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

    let at = |t: f32| egui::pos2(c.x + r * t.cos(), c.y + r * t.sin());

    if held {
        // Solid: you have hold of it, and the dashes have done their job.
        let full = DASHES * DASH_POINTS;
        let points: Vec<egui::Pos2> = (0..=full)
            .map(|i| at(std::f32::consts::TAU * i as f32 / full as f32))
            .collect();
        painter.add(egui::Shape::line(points, stroke));
    } else {
        let slot = std::f32::consts::TAU / DASHES as f32;
        for i in 0..DASHES {
            let start = slot * i as f32;
            let points: Vec<egui::Pos2> = (0..=DASH_POINTS)
                .map(|j| at(start + slot * DASH_FILL * j as f32 / DASH_POINTS as f32))
                .collect();
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}
