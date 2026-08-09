//! Gizmo picking and drag geometry
//!
//! Pure math for grabbing transform gizmos with the mouse. A gizmo is the
//! unit right-angled tetrahedron (O, X, Y, Z) mapped through the transform's
//! affine matrix; its screen-space parts are hit-tested against the cursor:
//!
//! - the origin dot          -> translate in the view plane
//! - an axis endpoint (tip)  -> scale that axis alone
//! - an O->axis edge         -> translate along that (world-space) local axis
//! - an outer axis-axis edge -> rotate around the remaining local axis
//!
//! (Uniform scale is a modifier on any grab, handled by the caller.)

use glam::{Mat4, Vec3};

use crate::camera::world_to_screen;
use crate::rot::Angle;

/// Pick radius around the origin dot, in pixels
pub const ORIGIN_RADIUS_PX: f32 = 12.0;
/// Pick radius around an axis endpoint handle, in pixels. Smaller than the
/// origin's: three of these sit around one origin, and the origin is the part
/// you want when they crowd together.
pub const TIP_RADIUS_PX: f32 = 8.0;
/// Pick radius around gizmo edges, in pixels
pub const EDGE_RADIUS_PX: f32 = 7.0;

/// Shortest an axis may project and still offer its tip, its shaft, or the
/// outer edges that meet it.
///
/// Two reasons, and the second is the load-bearing one:
///
/// * an axis a few pixels long has no room to distinguish three parts along it;
/// * an axis pointing nearly at the camera has almost no screen gain, so
///   [`line_param_closest_to_ray`] — which every drag along it uses — swings
///   enormously for a pixel of pointer movement. Grabbing it does not fail
///   gracefully, it explodes.
///
/// The guard is in screen space, so it is a statement about this camera and not
/// about the transform: an axis that is too short to grab becomes grabbable by
/// zooming in, which is the honest fix rather than a rendering trick that
/// pretends the axis is bigger than it is.
pub const MIN_AXIS_PX: f32 = 8.0;

/// Which part of a gizmo was grabbed
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GizmoPart {
    /// The origin dot: translate in the view plane
    Origin,
    /// An axis endpoint (0=x, 1=y, 2=z): scale that axis on its own, and pass
    /// through zero to mirror it
    Tip(usize),
    /// An origin->axis edge (0=x, 1=y, 2=z): translate along that axis
    Axis(usize),
    /// An outer edge: rotate around the given local axis index
    /// (edge x-y rotates around z, y-z around x, x-z around y)
    RotEdge(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct GizmoHit {
    pub transform: usize,
    pub part: GizmoPart,
}

fn dist_point_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = (p.0 - a.0, p.1 - a.1);
    let (ex, ey) = (b.0 - a.0, b.1 - a.1);
    let len_sq = ex * ex + ey * ey;
    let t = if len_sq > 0.0 {
        ((px * ex + py * ey) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (px - t * ex, py - t * ey);
    (dx * dx + dy * dy).sqrt()
}

/// Hit-test the cursor against every transform's gizmo. Returns the best hit.
///
/// Overlapping parts are resolved by score (lower wins), and the biases are the
/// whole design: an axis endpoint sits *on* the end of its own shaft and at the
/// meeting point of two outer edges, so without a bias the tip would be a
/// coin-toss against three other parts. The order is
/// origin (−8) < tip (−4) < edges (0), which reads as: when parts crowd
/// together, prefer the one whose drag is least destructive. The origin only
/// moves the map; a tip reshapes it.
///
/// That ordering also handles the degenerate case for free — when a transform
/// is so small on screen that its tips land on its origin, every tip is at
/// distance ~0 and so is the origin, and the origin's larger bonus wins.
///
/// `selected` scopes what is on offer. **An unselected transform offers only
/// its origin dot**; tips, shafts and rotate edges belong to the transform you
/// have already chosen.
///
/// This is the rule that closes the invisible-click hole. Every part of every
/// gizmo used to be pickable whether or not you could see it, so a gizmo buried
/// in the attractor still ate clicks and scrolls aimed at something else. The
/// x-ray pass makes the *selected* gizmo and every origin dot visible; this
/// makes the pickable set exactly the visible set. It also shrinks the contest
/// from ~140 candidates at twenty transforms to ~9.
///
/// The cost is one extra click to work on a different transform, which is the
/// order people work in anyway: pick the thing, then change it.
pub fn pick_gizmo(
    matrices: &[Mat4],
    selected: Option<usize>,
    view_proj: Mat4,
    cursor: (f32, f32),
    w: f32,
    h: f32,
) -> Option<GizmoHit> {
    // Scored candidates: lower is better. Origin dots get a bonus so a dot
    // sitting on top of an edge wins the grab.
    let mut best: Option<(f32, GizmoHit)> = None;
    let mut consider = |score: f32, hit: GizmoHit| {
        if best.map_or(true, |(s, _)| score < s) {
            best = Some((score, hit));
        }
    };

    for (i, m) in matrices.iter().enumerate() {
        let origin = m.w_axis.truncate();
        let ends = [
            m.transform_point3(Vec3::X),
            m.transform_point3(Vec3::Y),
            m.transform_point3(Vec3::Z),
        ];

        let Some(origin_s) = world_to_screen(origin, view_proj, w, h) else {
            continue;
        };
        let ends_s: Vec<Option<(f32, f32)>> = ends
            .iter()
            .map(|&e| world_to_screen(e, view_proj, w, h))
            .collect();

        // Origin dot — always on offer, for every transform. On an unselected
        // one it is the whole of what a click can do (select it); on the
        // selected one it translates, as it always has.
        let d = dist_point_segment(cursor, origin_s, origin_s);
        if d <= ORIGIN_RADIUS_PX {
            consider(d - 8.0, GizmoHit { transform: i, part: GizmoPart::Origin });
        }

        if selected != Some(i) {
            continue;
        }

        // An axis has to project far enough to be worth offering at all; see
        // `MIN_AXIS_PX`. Computed once here because the tip, the shaft and the
        // two outer edges that meet this axis all depend on the same answer.
        let long_enough: [bool; 3] = std::array::from_fn(|k| {
            ends_s[k].is_some_and(|es| {
                let (dx, dy) = (es.0 - origin_s.0, es.1 - origin_s.1);
                (dx * dx + dy * dy).sqrt() >= MIN_AXIS_PX
            })
        });

        // Axis endpoints
        for (k, end_s) in ends_s.iter().enumerate() {
            if let (Some(es), true) = (end_s, long_enough[k]) {
                let d = dist_point_segment(cursor, *es, *es);
                if d <= TIP_RADIUS_PX {
                    consider(d - 4.0, GizmoHit { transform: i, part: GizmoPart::Tip(k) });
                }
            }
        }

        // Origin->axis edges
        for (k, end_s) in ends_s.iter().enumerate() {
            if let (Some(es), true) = (end_s, long_enough[k]) {
                let d = dist_point_segment(cursor, origin_s, *es);
                if d <= EDGE_RADIUS_PX {
                    consider(d, GizmoHit { transform: i, part: GizmoPart::Axis(k) });
                }
            }
        }

        // Outer edges: (x,y) rotates around z, (y,z) around x, (x,z) around y
        const OUTER: [(usize, usize, usize); 3] = [(0, 1, 2), (1, 2, 0), (0, 2, 1)];
        for &(a, b, rot_axis) in &OUTER {
            if !(long_enough[a] && long_enough[b]) {
                continue;
            }
            if let (Some(asr), Some(bs)) = (ends_s[a], ends_s[b]) {
                let d = dist_point_segment(cursor, asr, bs);
                if d <= EDGE_RADIUS_PX {
                    consider(d, GizmoHit { transform: i, part: GizmoPart::RotEdge(rot_axis) });
                }
            }
        }
    }

    best.map(|(_, hit)| hit)
}

/// Intersect a ray with a plane. Returns None when the ray is parallel to it.
pub fn ray_plane(ray_o: Vec3, ray_d: Vec3, plane_pt: Vec3, normal: Vec3) -> Option<Vec3> {
    let denom = ray_d.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_pt - ray_o).dot(normal) / denom;
    Some(ray_o + ray_d * t)
}

/// Parameter s of the point on the line `line_pt + s * line_dir` closest to
/// the ray (Ericson, Real-Time Collision Detection §5.1.8). `line_dir` need
/// not be normalized; s is in units of `line_dir`.
pub fn line_param_closest_to_ray(line_pt: Vec3, line_dir: Vec3, ray_o: Vec3, ray_d: Vec3) -> f32 {
    let r = line_pt - ray_o;
    let a = line_dir.dot(line_dir);
    let e = ray_d.dot(ray_d);
    let b = line_dir.dot(ray_d);
    let c = line_dir.dot(r);
    let f = ray_d.dot(r);
    let denom = a * e - b * b;
    if denom.abs() < 1e-9 {
        // Line and ray are parallel: any point is equally close
        return 0.0;
    }
    (b * f - c * e) / denom
}

/// Screen-space angle of the cursor around a center point, CCW-positive in
/// conventional (y-up) orientation.
///
/// An [`Angle`] rather than an `f32` on purpose: two of these have no
/// difference until a caller says which way round the pointer went. Use
/// `start.shortest_to(now)`, which is right for a drag — nobody swings the
/// mouse more than half a turn between two frames.
pub fn screen_angle(center: (f32, f32), cursor: (f32, f32)) -> Angle {
    Angle::from_radians((-(cursor.1 - center.1)).atan2(cursor.0 - center.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{cursor_ray, OrbitCamera};

    fn test_cam() -> OrbitCamera {
        OrbitCamera::from_chart(0.4, 0.25, 0.0, 3.0, Vec3::ZERO)
    }

    #[test]
    fn ray_plane_hits_expected_point() {
        let hit = ray_plane(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::ZERO,
            Vec3::Y,
        )
        .unwrap();
        assert!(hit.length() < 1e-6);
    }

    #[test]
    fn line_ray_closest_param() {
        // Line along X through origin; ray shooting straight down at x=2
        let s = line_param_closest_to_ray(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(2.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
        );
        assert!((s - 2.0).abs() < 1e-5);
    }

    #[test]
    fn pick_origin_dot() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.5),
            glam::Quat::IDENTITY,
            Vec3::new(0.2, 0.1, -0.3),
        );
        let (sx, sy) = world_to_screen(m.w_axis.truncate(), vp, 1280.0, 720.0).unwrap();
        let hit = pick_gizmo(&[m], Some(0), vp, (sx + 3.0, sy - 2.0), 1280.0, 720.0).unwrap();
        assert_eq!(hit.transform, 0);
        assert_eq!(hit.part, GizmoPart::Origin);
    }

    #[test]
    fn pick_axis_edge_midpoint() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.6),
            glam::Quat::IDENTITY,
            Vec3::new(-0.3, 0.2, 0.1),
        );
        // Midpoint of the origin->Y edge in world space
        let mid = m.transform_point3(Vec3::new(0.0, 0.5, 0.0));
        let (sx, sy) = world_to_screen(mid, vp, 1280.0, 720.0).unwrap();
        let hit = pick_gizmo(&[m], Some(0), vp, (sx, sy), 1280.0, 720.0).unwrap();
        assert_eq!(hit.part, GizmoPart::Axis(1));
    }

    #[test]
    fn pick_outer_edge_rotation_axis() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.6),
            glam::Quat::IDENTITY,
            Vec3::new(0.0, 0.4, 0.0),
        );
        // Midpoint of the X-Y outer edge: rotation around local z
        let mid = m.transform_point3(Vec3::new(0.5, 0.5, 0.0));
        let (sx, sy) = world_to_screen(mid, vp, 1280.0, 720.0).unwrap();
        let hit = pick_gizmo(&[m], Some(0), vp, (sx, sy), 1280.0, 720.0).unwrap();
        assert_eq!(hit.part, GizmoPart::RotEdge(2));
    }

    /// The tip sits exactly where its own shaft ends and two outer edges meet.
    /// Without the bias this is a four-way tie decided by float noise, so this
    /// is the test that says the handle is reachable at all.
    #[test]
    fn tip_beats_the_parts_it_touches() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.6),
            glam::Quat::IDENTITY,
            Vec3::new(-0.2, 0.1, 0.0),
        );
        for k in 0..3 {
            let tip = m.transform_point3(match k {
                0 => Vec3::X,
                1 => Vec3::Y,
                _ => Vec3::Z,
            });
            let (sx, sy) = world_to_screen(tip, vp, 1280.0, 720.0).unwrap();
            let hit = pick_gizmo(&[m], Some(0), vp, (sx, sy), 1280.0, 720.0).unwrap();
            assert_eq!(hit.part, GizmoPart::Tip(k), "axis {k}");
        }
    }

    /// ...but not at the cost of the origin. When a transform is tiny on screen
    /// its tips pile onto its own origin dot, and the least destructive grab
    /// has to win: moving a map is recoverable at a glance, reshaping it is not.
    #[test]
    fn the_origin_still_wins_when_the_gizmo_is_tiny() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        // Small enough that all three tips project within a pixel or two of
        // the origin at this camera distance.
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.002),
            glam::Quat::IDENTITY,
            Vec3::new(0.1, 0.0, 0.0),
        );
        let (sx, sy) = world_to_screen(m.w_axis.truncate(), vp, 1280.0, 720.0).unwrap();
        let hit = pick_gizmo(&[m], Some(0), vp, (sx, sy), 1280.0, 720.0).unwrap();
        assert_eq!(hit.part, GizmoPart::Origin);
    }

    /// An axis too short to read is also too short to drag: `line_param_
    /// closest_to_ray` has almost no gain along it, so a pixel of pointer
    /// movement would swing the scale wildly. Offer nothing rather than
    /// something that can't be controlled.
    #[test]
    fn a_barely_projected_axis_offers_nothing() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.002),
            glam::Quat::IDENTITY,
            Vec3::new(0.1, 0.0, 0.0),
        );
        // Just outside the origin dot, where a tip or shaft would otherwise be
        // the only candidate.
        let (sx, sy) = world_to_screen(m.w_axis.truncate(), vp, 1280.0, 720.0).unwrap();
        let hit = pick_gizmo(&[m], Some(0), vp, (sx + ORIGIN_RADIUS_PX + 1.0, sy), 1280.0, 720.0);
        assert!(hit.is_none(), "got {hit:?} from an axis under the projection floor");
    }

    /// An unselected transform offers its origin dot and nothing else.
    ///
    /// The rule exists because picking has no depth awareness: before this, a
    /// gizmo buried in the attractor still took clicks aimed past it. The
    /// origin dot is exempt because the x-ray pass draws every one of them
    /// through the fractal — what you can grab is what you can see.
    #[test]
    fn an_unselected_transform_offers_only_its_origin() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.6),
            glam::Quat::IDENTITY,
            Vec3::new(-0.2, 0.1, 0.0),
        );

        // Every part that isn't the origin: the three tips, and the midpoints
        // of a shaft and an outer edge.
        let probes = [
            Vec3::X, Vec3::Y, Vec3::Z,
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.0),
        ];
        for probe in probes {
            let at = m.transform_point3(probe);
            let (sx, sy) = world_to_screen(at, vp, 1280.0, 720.0).unwrap();

            let selected = pick_gizmo(&[m], Some(0), vp, (sx, sy), 1280.0, 720.0);
            assert!(
                selected.is_some(),
                "the selected transform must still offer {probe:?}"
            );

            match pick_gizmo(&[m], None, vp, (sx, sy), 1280.0, 720.0) {
                None => {}
                // Close to the origin the dot legitimately wins; anything else
                // means an unselected transform handed out a manipulator.
                Some(hit) => assert_eq!(
                    hit.part, GizmoPart::Origin,
                    "unselected transform offered {:?} at {probe:?}", hit.part
                ),
            }
        }
    }

    /// ...but its origin dot stays grabbable, which is how it gets selected.
    #[test]
    fn an_unselected_transform_still_offers_its_origin() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.5),
            glam::Quat::IDENTITY,
            Vec3::new(0.2, 0.1, -0.3),
        );
        let (sx, sy) = world_to_screen(m.w_axis.truncate(), vp, 1280.0, 720.0).unwrap();
        let hit = pick_gizmo(&[m], None, vp, (sx + 2.0, sy), 1280.0, 720.0).unwrap();
        assert_eq!(hit.transform, 0);
        assert_eq!(hit.part, GizmoPart::Origin);
    }

    #[test]
    fn pick_misses_empty_space() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0));
        assert!(pick_gizmo(&[m], Some(0), vp, (30.0, 30.0), 1280.0, 720.0).is_none());
    }

    /// Full drag simulation: grab the origin dot, move the cursor, verify the
    /// view-plane translation follows it on screen
    #[test]
    fn view_plane_translate_follows_cursor() {
        let cam = test_cam();
        let (w, h) = (1280.0f32, 720.0f32);
        let vp = cam.view_proj(w / h);
        let inv = vp.inverse();
        let origin = Vec3::new(0.2, 0.1, -0.3);

        let (sx, sy) = world_to_screen(origin, vp, w, h).unwrap();
        let normal = cam.forward();

        // Grab exactly on the dot, drag 40px right and 25px up
        let (ro, rd) = cursor_ray(inv, sx, sy, w, h);
        let grab = ray_plane(ro, rd, origin, normal).unwrap();
        let grab_offset = origin - grab;

        let (cx, cy) = (sx + 40.0, sy - 25.0);
        let (ro2, rd2) = cursor_ray(inv, cx, cy, w, h);
        let hit = ray_plane(ro2, rd2, origin, normal).unwrap();
        let new_origin = hit + grab_offset;

        let (nx, ny) = world_to_screen(new_origin, vp, w, h).unwrap();
        assert!((nx - cx).abs() < 0.5, "x: {} vs {}", nx, cx);
        assert!((ny - cy).abs() < 0.5, "y: {} vs {}", ny, cy);
    }

    #[test]
    fn a_drag_across_the_seam_is_a_short_move() {
        // The pointer crossing the -x axis takes the angle from just under +pi
        // to just over -pi. That is a small move, and the gizmo must rotate by
        // a small amount — not by very nearly a full turn the other way.
        let center = (100.0, 100.0);
        let just_above = screen_angle(center, (0.0, 99.9));
        let just_below = screen_angle(center, (0.0, 100.1));
        let delta = just_above.shortest_to(just_below);
        assert!(delta.abs() < 0.01, "seam crossing swung {} rad", delta.radians());
        // The literal difference of the two representatives is the trap: it is
        // very nearly a whole turn, and using it directly is the bug.
        let naive = just_below.radians() - just_above.radians();
        assert!(naive.abs() > 6.2, "fixture must actually straddle the seam ({})", naive);
    }
}
