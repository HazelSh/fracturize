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

/// Distance from a pane's bottom-right corner when the viewport is split. The
/// whole-window cross keeps clear of the status bar; a pane's corner already
/// is clear of it.
const PANE_MARGIN: egui::Vec2 = egui::vec2(-8.0, -8.0);

pub fn draw(ctx: &egui::Context, app: &mut App) {
    // It is a gizmo — a handle drawn over the picture that you grab to move
    // the camera — so it goes away with the rest of them. Somewhere to look
    // at the fractal with nothing on top of it is the whole point of the
    // toggle, and one cross left in the corner spoils that.
    if !app.show_gizmos {
        return;
    }

    // The zoom carries the world with it; see `App::zoom_frame`. Identity
    // unless this scene has an infinite zoom and the camera has folded.
    let frame = app.zoom_frame();
    let carried = frame != crate::rot::Orientation::IDENTITY;
    let windows: Vec<egui::Rect> = ctx.memory(|m| {
        m.areas().visible_layer_ids()
            .into_iter()
            .filter(|l| l.order == egui::Order::Middle)
            .filter_map(|l| m.area_rect(l.id))
            .collect()
    });

    // One cross per pane, each drawn from that pane's camera, so every view
    // says which way it is looking (`multiview`). In the perspective pane a
    // click turns the camera to look down the axis; in an axis pane it makes
    // that pane look down it instead.
    let panes = app.panes();
    let ppp = ctx.pixels_per_point();
    let mut go: Option<(usize, Vec3)> = None;
    for (i, pane) in panes.iter().enumerate() {
        let (screen, margin) = if panes.len() == 1 {
            (ctx.content_rect(), MARGIN)
        } else {
            let r = pane.rect;
            let rect = egui::Rect::from_min_size(
                egui::pos2(r.x as f32 / ppp, r.y as f32 / ppp),
                egui::vec2(r.w as f32 / ppp, r.h as f32 / ppp),
            );
            (rect, PANE_MARGIN)
        };
        let twist = if carried {
            "\n\nUnder infinite zoom the cross keeps turning as you zoom, following \
             the zoom's own twist."
        } else {
            ""
        };
        let (hint, status) = match pane.slot {
            crate::multiview::Slot::Perspective => (
                format!(
                    "Which way the world's axes point. Click a ball to look along \
                     that axis. Solid balls are the positive ends, hollow ones the \
                     negative.{twist}"
                ),
                "click an axis: look down it",
            ),
            crate::multiview::Slot::Axis(k) => {
                let axis = app.view_axis(k);
                (
                    format!(
                        "This view looks down {} at the scene ({}), from far enough \
                         away that depth doesn't change sizes. Click another ball to \
                         look down that axis here instead. Solid balls are the \
                         positive ends, hollow ones the negative.{twist}",
                        axis.signed(),
                        axis.name().to_lowercase(),
                    ),
                    "click an axis: this view looks down it",
                )
            }
        };
        let id = egui::Id::new(("fracturize_axis_widget", i));
        let to_cam = pane.camera.orientation.inverse();
        if let Some(world) = cross(ctx, app, id, screen, margin, &windows, to_cam, frame, &hint, status) {
            go = Some((i, world));
        }
    }

    match go.map(|(i, world)| (panes[i].slot, world)) {
        Some((crate::multiview::Slot::Perspective, world)) => look_down(app, world),
        Some((crate::multiview::Slot::Axis(k), world)) => {
            // Back out of the zoom's carried frame to the world axis it shows.
            app.set_view_axis(k, crate::multiview::Axis::nearest(frame.inverse().rotate(world)));
        }
        None => {}
    }
}

/// Draw one cross in the bottom-right corner of `screen`, as seen through a
/// camera whose world-to-camera rotation is `to_cam`, and return the world
/// direction of a ball that was clicked.
#[allow(clippy::too_many_arguments)]
fn cross(
    ctx: &egui::Context,
    app: &mut App,
    id: egui::Id,
    screen: egui::Rect,
    margin: egui::Vec2,
    windows: &[egui::Rect],
    to_cam: Orientation,
    frame: Orientation,
    hint: &str,
    status: &str,
) -> Option<Vec3> {
    let mut go: Option<Vec3> = None;
    // Out of the way of the windows. The Timeline is wide and low and often
    // dragged right across the bottom of the screen, and the Transforms
    // window often sits down the right edge; a cross drawn over either hides
    // what is under it and takes its clicks. So the cross climbs above any
    // window it would land on (last frame's rectangles), or, when there's no
    // room above, steps to its left.
    let size = egui::Vec2::splat(RADIUS * 2.0 + HIT);
    let home = egui::Rect::from_min_size(screen.right_bottom() + margin - size, size);
    let mut at = home;
    for _ in 0..8 {
        let Some(w) = windows.iter().find(|w| at.expand(4.0).intersects(**w)) else { break };
        let above = at.translate(egui::vec2(0.0, w.top() - 8.0 - at.bottom()));
        at = if above.top() > screen.top() + 40.0 {
            above
        } else {
            at.translate(egui::vec2(w.left() - 8.0 - at.right(), 0.0))
        };
    }
    egui::Area::new(id)
        .fixed_pos(at.min)
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let centre = rect.center();
            let painter = ui.painter();

            // World → camera. The camera looks down its own −Z, so a ball with
            // camera-space z > 0 is on the near side and gets drawn last.

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
            let resp = ui.interact(rect, id.with("hit"), egui::Sense::click());
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
            super::hints::hinted(resp, &mut app.ui_state, hint, status);
        });
    go
}

/// Put the camera on `dir`, looking back at the focus — Blender's numpad
/// 1/3/7, generalised to all six.
///
/// Keeps the distance and the focus: this is a change of *angle*, and moving
/// the eye as well would lose the thing you were looking at. Glides rather than
/// cuts, like every other discrete framing change.
fn look_down(app: &mut App, dir: Vec3) {
    // The framing an axis pane looking down `dir` has, so the perspective view
    // turned to an axis and that axis's own pane show the same picture. World
    // up is screen up except looking along y, where +x stays on the right, as
    // a drawing's top view sits above its front view.
    let target = OrbitCamera {
        orientation: crate::multiview::axis_orientation(dir),
        ..app.camera
    };
    app.glide_camera_to(target);
    // Taking the framing by hand stops the camera flying its path, the same as
    // dragging in the viewport does — they'd fight over it otherwise.
    app.stop_camera_motion();
}
