//! The orientation gizmo: a small clickable axis cross in a viewport corner.
//!
//! Blender, Fusion, Onshape and everything after them put one of these in a
//! viewport corner, and Fracturize needs it *more* than they do — because the
//! default orbit is a trackball, which deliberately accumulates roll (the price
//! is stated honestly in the Camera panel's own tooltip). Until now the only
//! thing on screen saying which way is
//! up was a numeric readout in a panel that is closed by default. A corner axis
//! cross pays directly for the trackball decision.
//!
//! Clicking a label flies to that axis, which is Blender's numpad 1/3/7 — and
//! for an IFS those aren't a nicety: authoring rotational symmetry is much
//! easier when you can get exactly down an axis. Both features come out of the
//! one control, which is why it's worth drawing rather than adding six buttons
//! to a panel.

use glam::Vec3;

use crate::app::App;
use crate::camera::OrbitCamera;
use crate::rot::Orientation;

/// Radius of the cross, in egui points.
const RADIUS: f32 = 26.0;
/// Half-width of a ball's hit target — comfortably bigger than the ball.
const HIT: f32 = 11.0;
/// Distance from the viewport's bottom-right corner.
const MARGIN: egui::Vec2 = egui::vec2(-16.0, -34.0);

/// The six labelled directions, as (label, world direction, colour).
///
/// Coloured by axis rather than by sign — the two ends of one axis are the same
/// axis, and giving them different hues would say otherwise. The negative ends
/// are drawn hollow, which is the convention everyone else uses too.
///
/// The hues are [`crate::palette::axes`], shared with the transform gizmos: the
/// cross in the corner is the legend for the coloured shafts out in the scene,
/// and a legend that disagrees with the thing it labels is worse than none.
const AXES: [(&str, Vec3, egui::Color32); 3] = [
    ("X", Vec3::X, crate::palette::axes::axis_color32(0)),
    ("Y", Vec3::Y, crate::palette::axes::axis_color32(1)),
    ("Z", Vec3::Z, crate::palette::axes::axis_color32(2)),
];

pub fn draw(ctx: &egui::Context, app: &mut App) {
    // It is a gizmo — a handle drawn over the picture that you grab to move
    // the camera — so it goes away with the rest of them. Somewhere to look
    // at the fractal with nothing on top of it is the whole point of the
    // toggle, and one cross left in the corner spoils that.
    if !app.show_gizmos {
        return;
    }

    let mut go: Option<Vec3> = None;
    // The zoom carries the world with it; see `App::zoom_frame`. Identity
    // unless this scene has an infinite zoom and the camera has folded.
    let frame = app.zoom_frame();
    let carried = frame != crate::rot::Orientation::IDENTITY;

    egui::Area::new(egui::Id::new("fracturize_axis_widget"))
        .anchor(egui::Align2::RIGHT_BOTTOM, MARGIN)
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let size = egui::Vec2::splat(RADIUS * 2.0 + HIT);
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let centre = rect.center();
            let painter = ui.painter();

            // World → camera. The camera looks down its own −Z, so a ball with
            // camera-space z > 0 is on the near side and gets drawn last.
            let to_cam = app.camera.orientation.inverse();

            // (screen position, camera-space depth, label, colour, world dir)
            let mut balls: Vec<(egui::Pos2, f32, &str, egui::Color32, Vec3, bool)> = Vec::new();
            for (name, axis, color) in AXES {
                for positive in [true, false] {
                    // Carried, so that both what is drawn and where a click
                    // flies to are the same direction. Clicking a ball has
                    // always meant "look down the way this one points"; under
                    // a zoom the way it points is the carried axis, and
                    // sending the camera to the uncarried one would fly it off
                    // the ball you clicked.
                    let world = frame.rotate(if positive { axis } else { -axis });
                    let d = to_cam.rotate(world);
                    // Screen y grows downward, camera y grows up.
                    let at = centre + egui::vec2(d.x, -d.y) * RADIUS;
                    balls.push((at, d.z, name, color, world, positive));
                }
            }
            // Painter's algorithm: far balls first, so the near ones overlap
            // them rather than the other way round.
            balls.sort_by(|a, b| a.1.total_cmp(&b.1));

            let pointer = ui.input(|i| i.pointer.interact_pos());
            for (at, depth, name, color, world, positive) in &balls {
                // Fade with depth so the cross reads as a solid object rather
                // than six dots in a plane. Never all the way out: a hidden
                // control you can still click is worse than a dim one.
                let t = (depth + 1.0) * 0.5;
                let fade = 0.45 + 0.55 * t;
                let color = color.gamma_multiply(fade);

                let hovered = pointer.is_some_and(|p| p.distance(*at) <= HIT);
                let r = if hovered { 8.0 } else { 6.5 };

                painter.line_segment(
                    [centre, *at],
                    egui::Stroke::new(1.5, color.gamma_multiply(0.6)),
                );
                if *positive {
                    painter.circle_filled(*at, r, color);
                } else {
                    // Hollow for the negative end — same axis, other way.
                    painter.circle_filled(*at, r, ui.visuals().window_fill);
                    painter.circle_stroke(*at, r, egui::Stroke::new(1.5, color));
                }
                if *positive || hovered {
                    painter.text(
                        *at,
                        egui::Align2::CENTER_CENTER,
                        name,
                        egui::FontId::monospace(9.0),
                        if *positive {
                            egui::Color32::from_rgb(20, 20, 24)
                        } else {
                            color
                        },
                    );
                }
                if hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let _ = world;
            }

            // One response for the whole cross, hit-tested against the balls —
            // six separate widgets would each need their own id and would fight
            // over the overlapping ones.
            let resp = ui.interact(
                rect,
                egui::Id::new("fracturize_axis_widget_hit"),
                egui::Sense::click(),
            );
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    // Nearest ball to the click, near side first — the one that
                    // was drawn on top is the one you meant.
                    let hit = balls
                        .iter()
                        .rev()
                        .find(|(at, ..)| p.distance(*at) <= HIT);
                    if let Some((.., world, _)) = hit {
                        go = Some(*world);
                    }
                }
            }
            let hint = if carried {
                "Which way the world is pointing. Click a ball to look down that \
                 axis — solid is the positive end, hollow the negative.\n\nThe \
                 zoom carries it: each period turns the camera by the map's \
                 rotation, so the cross keeps turning with the descent instead \
                 of snapping back at the seam."
            } else {
                "Which way the world is pointing. Click a ball to look down that \
                 axis — solid is the positive end, hollow the negative."
            };
            super::hints::hinted(
                resp,
                &mut app.ui_state,
                hint,
                "click an axis: look down it",
            );
        });

    if let Some(dir) = go {
        look_down(app, dir);
    }
}

/// Put the camera on `dir`, looking back at the focus — Blender's numpad
/// 1/3/7, generalised to all six.
///
/// Keeps the distance and the focus: this is a change of *angle*, and moving
/// the eye as well would lose the thing you were looking at. Glides rather than
/// cuts, like every other discrete framing change.
fn look_down(app: &mut App, dir: Vec3) {
    let z = dir.normalize();
    // World up, except when looking straight along it — there "up" has to be
    // something else, and +Z is what every tool picks for the top view.
    let up = if z.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let x = up.cross(z).normalize();
    let y = z.cross(x);
    let target = OrbitCamera {
        orientation: Orientation::from_basis(x, y, z),
        ..app.camera
    };
    app.glide_camera_to(target);
    // Taking the framing by hand stops the camera flying its path, the same as
    // dragging in the viewport does — they'd fight over it otherwise.
    app.stop_camera_motion();
}
