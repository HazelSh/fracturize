//! World-space indicator geometry for the selected transform: what it does
//! relative to the identity, drawn rather than left to be inferred from
//! numbers.
//!
//! The gizmo renderer already draws a grey identity tetrahedron beside each
//! transform's coloured one, so "the base" is on screen — but the
//! *relationship* between the two was never drawn, and Euler angles are a
//! poor stand-in for it. Three interacting numbers can't answer "how far is
//! this rotated?" at a glance, and they gimbal. Rotation is natively one axis
//! plus one angle, and both of those can be drawn.
//!
//! So, for the selected transform only (drawing this for every transform
//! would be a thicket):
//!
//! * an **offset vector** from the world origin to the transform's origin,
//!   with an arrowhead — magnitude and direction of the displacement;
//! * a **rotation axis** through that origin, with an **arc** sweeping the
//!   angle — the rotation as the single thing it actually is.
//!
//! The world origin is the base because that is what an IFS transform maps
//! about, and it's where the grey identity cell already sits.

use glam::{Mat4, Quat, Vec3};

use crate::gpu::LineVertex;

/// Offset vector colour: warm, so it reads apart from the cool rotation
/// indicator and from the palette colours the fractal itself uses.
const OFFSET_COLOR: [f32; 4] = [1.0, 0.78, 0.35, 0.85];
/// Rotation axis / arc colour.
const ROT_COLOR: [f32; 4] = [0.45, 0.85, 1.0, 0.8];
/// Segments in the angle arc — enough that a full turn still reads as round.
const ARC_SEGMENTS: usize = 48;
/// Below this angle (radians) the arc is too small to mean anything; the axis
/// line alone says "barely rotated".
const MIN_ARC_ANGLE: f32 = 0.02;

/// Build the indicator line list for one transform's matrix. Returns pairs of
/// vertices (the line renderer draws a `LineList`).
pub fn build(matrix: Mat4) -> Vec<LineVertex> {
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    // Characteristic size of this transform's cell, used to scale the
    // indicators so they stay legible for both tiny and large transforms.
    let cell = ((scale.x.abs() + scale.y.abs() + scale.z.abs()) / 3.0).max(0.02);

    let mut verts = Vec::with_capacity(2 * (2 + 4 + 1 + ARC_SEGMENTS + 2));
    push_offset(&mut verts, translation, cell);
    push_rotation(&mut verts, translation, rotation, cell);
    verts
}

fn seg(verts: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    verts.push(LineVertex { position: a.to_array(), color });
    verts.push(LineVertex { position: b.to_array(), color });
}

/// Origin -> transform position, with a four-strut arrowhead. The arrowhead
/// is built from two vectors perpendicular to the shaft rather than billboard
/// geometry, so it reads as an arrow from any camera angle.
fn push_offset(verts: &mut Vec<LineVertex>, translation: Vec3, cell: f32) {
    let len = translation.length();
    if len < 1e-4 {
        return;
    }
    let dir = translation / len;
    seg(verts, Vec3::ZERO, translation, OFFSET_COLOR);

    let (u, v) = perpendicular_basis(dir);
    let head = (cell * 0.35).min(len * 0.3);
    let base = translation - dir * head;
    for offset in [u, -u, v, -v] {
        seg(verts, translation, base + offset * head * 0.45, OFFSET_COLOR);
    }
}

/// The rotation axis through the transform's origin, plus an arc sweeping the
/// rotation angle in the plane perpendicular to it, with radial ticks at both
/// ends so the direction of the sweep is unambiguous.
fn push_rotation(verts: &mut Vec<LineVertex>, translation: Vec3, rotation: Quat, cell: f32) {
    let (axis, angle) = rotation.to_axis_angle();
    // `to_axis_angle` hands back an arbitrary axis for a near-identity
    // rotation; there's nothing meaningful to draw in that case.
    if !axis.is_finite() || angle.abs() < MIN_ARC_ANGLE {
        return;
    }
    let axis = axis.normalize_or_zero();
    if axis == Vec3::ZERO {
        return;
    }

    let half = cell * 1.3;
    seg(verts, translation - axis * half, translation + axis * half, ROT_COLOR);

    let (u, v) = perpendicular_basis(axis);
    let radius = cell * 0.75;
    let point_at = |t: f32| translation + (u * t.cos() + v * t.sin()) * radius;

    let steps = ((angle.abs() / std::f32::consts::TAU) * ARC_SEGMENTS as f32).ceil().max(2.0) as usize;
    let mut prev = point_at(0.0);
    for i in 1..=steps {
        let t = angle * (i as f32 / steps as f32);
        let next = point_at(t);
        seg(verts, prev, next, ROT_COLOR);
        prev = next;
    }

    // Radial ticks: where the sweep starts, and where it ends.
    seg(verts, translation, point_at(0.0), ROT_COLOR);
    seg(verts, translation, point_at(angle), ROT_COLOR);
}

/// Two unit vectors spanning the plane perpendicular to `dir`.
fn perpendicular_basis(dir: Vec3) -> (Vec3, Vec3) {
    // Cross with whichever cardinal axis `dir` is least aligned to, so the
    // cross product never degenerates.
    let seed = if dir.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = dir.cross(seed).normalize_or(Vec3::Y);
    let v = dir.cross(u).normalize_or(Vec3::Z);
    (u, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::EulerRot;

    #[test]
    fn perpendicular_basis_is_orthonormal_for_any_direction() {
        // Including the cases that naive cross-product code gets wrong: a
        // direction parallel to the seed axis.
        for dir in [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            -Vec3::X,
            Vec3::new(1.0, 1.0, 1.0).normalize(),
            Vec3::new(-0.3, 0.9, 0.2).normalize(),
        ] {
            let (u, v) = perpendicular_basis(dir);
            assert!(u.is_finite() && v.is_finite(), "dir {:?} produced NaN", dir);
            assert!((u.length() - 1.0).abs() < 1e-3, "u not unit for {:?}", dir);
            assert!((v.length() - 1.0).abs() < 1e-3, "v not unit for {:?}", dir);
            assert!(u.dot(dir).abs() < 1e-3, "u not perpendicular for {:?}", dir);
            assert!(v.dot(dir).abs() < 1e-3, "v not perpendicular for {:?}", dir);
        }
    }

    #[test]
    fn identity_transform_draws_nothing() {
        // No offset and no rotation means there is nothing to say.
        assert!(build(Mat4::IDENTITY).is_empty());
    }

    #[test]
    fn offset_only_draws_the_vector_but_no_arc() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.5),
            Quat::IDENTITY,
            Vec3::new(1.0, 0.0, 0.0),
        );
        let verts = build(m);
        assert!(!verts.is_empty());
        // Shaft plus four arrowhead struts, and nothing in the rotation colour.
        assert_eq!(verts.len(), 10);
        assert!(verts.iter().all(|v| v.color == OFFSET_COLOR));
    }

    #[test]
    fn rotation_produces_axis_and_arc() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.5),
            Quat::from_euler(EulerRot::XYZ, 0.0, 1.2, 0.0),
            Vec3::ZERO,
        );
        let verts = build(m);
        assert!(
            verts.iter().any(|v| v.color == ROT_COLOR),
            "a rotated transform must draw its axis and arc"
        );
        assert!(
            verts.iter().all(|v| v.position.iter().all(|c| c.is_finite())),
            "indicator geometry must never contain NaN"
        );
    }

    #[test]
    fn near_identity_rotation_is_not_drawn() {
        // glam hands back an arbitrary axis for a near-zero rotation; drawing
        // it would put a random line through the scene.
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.5),
            Quat::from_euler(EulerRot::XYZ, 0.0, 1e-6, 0.0),
            Vec3::ZERO,
        );
        assert!(build(m).iter().all(|v| v.color != ROT_COLOR));
    }
}
