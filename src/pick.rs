//! Gizmo picking and drag geometry
//!
//! Pure math for grabbing transform gizmos with the mouse. A gizmo is the
//! unit right-angled tetrahedron (O, X, Y, Z) mapped through the transform's
//! affine matrix; its screen-space parts are hit-tested against the cursor:
//!
//! - the origin dot          -> translate in the view plane
//! - an O->axis edge         -> translate along that (world-space) local axis
//! - an outer axis-axis edge -> rotate around the remaining local axis
//!
//! (Uniform scale is a modifier on any grab, handled by the caller.)

use glam::{Mat4, Vec3};

use crate::camera::world_to_screen;

/// Pick radius around the origin dot, in pixels
pub const ORIGIN_RADIUS_PX: f32 = 12.0;
/// Pick radius around gizmo edges, in pixels
pub const EDGE_RADIUS_PX: f32 = 7.0;

/// Which part of a gizmo was grabbed
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GizmoPart {
    /// The origin dot: translate in the view plane
    Origin,
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

/// Hit-test the cursor against every transform's gizmo. Returns the best hit,
/// with the origin dot given priority over edges when both are in range.
pub fn pick_gizmo(
    matrices: &[Mat4],
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

        // Origin dot
        let d = dist_point_segment(cursor, origin_s, origin_s);
        if d <= ORIGIN_RADIUS_PX {
            consider(d - 8.0, GizmoHit { transform: i, part: GizmoPart::Origin });
        }

        // Origin->axis edges
        for (k, end_s) in ends_s.iter().enumerate() {
            if let Some(es) = end_s {
                let d = dist_point_segment(cursor, origin_s, *es);
                if d <= EDGE_RADIUS_PX {
                    consider(d, GizmoHit { transform: i, part: GizmoPart::Axis(k) });
                }
            }
        }

        // Outer edges: (x,y) rotates around z, (y,z) around x, (x,z) around y
        const OUTER: [(usize, usize, usize); 3] = [(0, 1, 2), (1, 2, 0), (0, 2, 1)];
        for &(a, b, rot_axis) in &OUTER {
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
/// conventional (y-up) orientation
pub fn screen_angle(center: (f32, f32), cursor: (f32, f32)) -> f32 {
    (-(cursor.1 - center.1)).atan2(cursor.0 - center.0)
}

/// Smallest signed angle difference a - b, wrapped to (-pi, pi]
pub fn wrap_angle(delta: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = delta % TAU;
    if d > PI {
        d -= TAU;
    } else if d <= -PI {
        d += TAU;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{cursor_ray, OrbitCamera};

    fn test_cam() -> OrbitCamera {
        OrbitCamera {
            yaw: 0.4,
            pitch: 0.25,
            distance: 3.0,
            focus: Vec3::ZERO,
            roll: 0.0,
        }
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
        let hit = pick_gizmo(&[m], vp, (sx + 3.0, sy - 2.0), 1280.0, 720.0).unwrap();
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
        let hit = pick_gizmo(&[m], vp, (sx, sy), 1280.0, 720.0).unwrap();
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
        let hit = pick_gizmo(&[m], vp, (sx, sy), 1280.0, 720.0).unwrap();
        assert_eq!(hit.part, GizmoPart::RotEdge(2));
    }

    #[test]
    fn pick_misses_empty_space() {
        let cam = test_cam();
        let vp = cam.view_proj(16.0 / 9.0);
        let m = Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0));
        assert!(pick_gizmo(&[m], vp, (30.0, 30.0), 1280.0, 720.0).is_none());
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
    fn wrap_angle_wraps() {
        assert!((wrap_angle(3.0 * std::f32::consts::PI) - std::f32::consts::PI).abs() < 1e-5);
        assert!((wrap_angle(0.3) - 0.3).abs() < 1e-6);
        assert!((wrap_angle(-3.5) - (-3.5 + std::f32::consts::TAU)).abs() < 1e-5);
    }
}
