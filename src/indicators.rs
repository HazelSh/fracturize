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
//!
//! [`build_path`] is the same idea applied to a camera path: a motion you
//! otherwise have to press play and watch to know anything about, drawn as a
//! thing in the scene you can see all of at once.

use glam::{Mat4, Quat, Vec3};

use crate::gpu::LineVertex;
use crate::path::CameraPath;

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

/// Symmetry colour: violet, so a group's axis or cage reads apart from both
/// the amber offset vector and the cyan rotation arc of the map it governs —
/// all three can be on screen at once, and they say different things.
const SYM_COLOR: [f32; 4] = [0.72, 0.55, 1.0, 0.75];
/// The spokes that make the fold count countable are drawn brighter than the
/// axis: they are the part you read a number off.
const SYM_SPOKE_COLOR: [f32; 4] = [0.85, 0.72, 1.0, 0.95];

/// Draw a symmetry group into the scene.
///
/// A symmetry you can't see is a symmetry you can't edit, and until this the
/// only evidence a group existed was that the fractal had more arms than the
/// transform list explained. Two shapes, because the groups come in two kinds:
///
/// * **`C_n` / `D_n`** — the axis as a line through the attractor, with `n`
///   spokes around it. You read the fold count by counting spokes, which is the
///   one number about a cyclic group you actually need at a glance. `D_n` adds
///   its half-turn axes lying in the equator, which is exactly the difference
///   between `D` and `C` and so is exactly what should distinguish them here.
/// * **`T` / `O` / `I`** — the group's namesake solid as a wireframe cage. The
///   polyhedral groups have no single axis to draw, and a tetrahedron, cube or
///   icosahedron says "you are working in T/O/I" faster than any label can.
///
/// `radius` is the attractor's own 95th-percentile radius, so the drawing sits
/// on the form rather than at some fixed world scale.
pub fn build_symmetry(sym: &crate::symmetry::Symmetry, radius: f32) -> Vec<LineVertex> {
    use crate::symmetry::OrbitKind;

    let r = radius.max(1e-3);
    let mut verts = Vec::new();

    match sym.kind() {
        OrbitKind::Cyclic(n) | OrbitKind::Dihedral(n) => {
            let axis = sym.axis();
            let (u, v) = perpendicular_basis(axis);

            // The axis, run a little past the form so both ends are visible.
            seg(&mut verts, -axis * r * 1.35, axis * r * 1.35, SYM_COLOR);

            // n spokes in the equatorial plane: the fold count, countable.
            // Drawn from a small hub rather than from the centre so they read
            // as a wheel and not as a starburst that fills the middle of the
            // scene where the fractal is densest.
            let hub = r * 0.18;
            for k in 0..n {
                let t = std::f32::consts::TAU * k as f32 / n as f32;
                let dir = u * t.cos() + v * t.sin();
                seg(&mut verts, dir * hub, dir * r * 0.95, SYM_SPOKE_COLOR);
            }

            // A ring joining the spoke tips, so the plane of the rotation is
            // legible edge-on as well as face-on.
            let ring = 64;
            let mut prev = u * r * 0.95;
            for i in 1..=ring {
                let t = std::f32::consts::TAU * i as f32 / ring as f32;
                let next = (u * t.cos() + v * t.sin()) * r * 0.95;
                seg(&mut verts, prev, next, SYM_COLOR);
                prev = next;
            }

            // D_n's half-turn axes. They bisect the spokes rather than lying
            // along them, which is where the generator actually puts them.
            if matches!(sym.kind(), OrbitKind::Dihedral(_)) {
                for k in 0..n {
                    let t = std::f32::consts::TAU * (k as f32 + 0.5) / n as f32;
                    let dir = u * t.cos() + v * t.sin();
                    // Short struts through the ring: a two-fold axis is a line
                    // you spin about, so it is drawn as one, sticking out both
                    // sides of the ring it pierces.
                    seg(&mut verts, dir * r * 0.82, dir * r * 1.12, SYM_SPOKE_COLOR);
                }
            }
        }
        // A repeat has no cage and no spokes — it is a chain, so it is drawn as
        // one: the path the copies actually walk, with a tick at each. The
        // ghosts show where the copies are; this shows the rule that put them
        // there, which is the thing being edited.
        OrbitKind::Repeat(_) => {
            let axis = sym.axis();
            seg(&mut verts, -axis * r * 1.15, axis * r * 1.15, SYM_COLOR);

            let (u, v) = perpendicular_basis(axis);
            let nodes: Vec<Vec3> = sym
                .elements()
                .iter()
                .map(|e| e.transform_point3(Vec3::ZERO))
                .collect();
            let tick = r * 0.05;
            for (i, node) in nodes.iter().enumerate() {
                if i + 1 < nodes.len() {
                    seg(&mut verts, *node, nodes[i + 1], SYM_SPOKE_COLOR);
                }
                // A cross rather than a dot: the line renderer has no points,
                // and a cross stays visible when the chain is viewed end-on.
                seg(&mut verts, *node - u * tick, *node + u * tick, SYM_COLOR);
                seg(&mut verts, *node - v * tick, *node + v * tick, SYM_COLOR);
            }
        }
        kind => {
            for (a, b) in cage_edges(kind) {
                seg(&mut verts, a * r, b * r, SYM_COLOR);
            }
        }
    }

    verts
}

/// The wireframe of a polyhedral group's namesake solid, as unit-radius
/// vertex pairs.
///
/// The orientations here are not decorative — they are the same ones
/// `symmetry.rs` generates the groups in, and a cage in the wrong frame would
/// draw a symmetry the attractor doesn't have. `the_cage_matches_its_group`
/// checks exactly that, by applying every group element to the vertex set.
///
/// Edges are found by distance rather than listed: in each of these solids the
/// edges are precisely the vertex pairs at the minimum separation, so a short
/// search is both shorter than a table and impossible to mistype.
fn cage_edges(kind: crate::symmetry::OrbitKind) -> Vec<(Vec3, Vec3)> {
    use crate::symmetry::OrbitKind;

    let verts: Vec<Vec3> = match kind {
        // The tetrahedron on alternate cube corners — the frame in which the
        // group's 3-fold generator is the cube diagonal (1,1,1).
        OrbitKind::Tetrahedral => vec![
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
        ],
        // The cube, axis-aligned: its 4-fold axes are the coordinate axes.
        OrbitKind::Octahedral => (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { 1.0 } else { -1.0 },
                    if i & 2 == 0 { 1.0 } else { -1.0 },
                    if i & 4 == 0 { 1.0 } else { -1.0 },
                )
            })
            .collect(),
        // The icosahedron as the cyclic permutations of (0, ±1, ±φ), which is
        // the construction the 5-fold generator's axis (0, 1, φ) comes from.
        OrbitKind::Icosahedral => {
            let phi = (1.0 + 5.0f32.sqrt()) / 2.0;
            let mut v = Vec::with_capacity(12);
            for &s1 in &[1.0f32, -1.0] {
                for &s2 in &[1.0f32, -1.0] {
                    v.push(Vec3::new(0.0, s1, s2 * phi));
                    v.push(Vec3::new(s1, s2 * phi, 0.0));
                    v.push(Vec3::new(s2 * phi, 0.0, s1));
                }
            }
            v
        }
        // C/D are drawn as an axis, not a cage.
        _ => return Vec::new(),
    };

    let scale = verts[0].length();
    let unit: Vec<Vec3> = verts.iter().map(|v| *v / scale).collect();

    let mut shortest = f32::MAX;
    for (i, a) in unit.iter().enumerate() {
        for b in &unit[i + 1..] {
            shortest = shortest.min(a.distance_squared(*b));
        }
    }

    let mut edges = Vec::new();
    for (i, a) in unit.iter().enumerate() {
        for b in &unit[i + 1..] {
            if (a.distance_squared(*b) - shortest).abs() < 1e-3 {
                edges.push((*a, *b));
            }
        }
    }
    edges
}

/// Camera-path polyline colour: green, so it reads apart from the amber
/// offset vector and the cyan rotation arc a selected transform draws.
const PATH_COLOR: [f32; 4] = [0.4, 0.95, 0.55, 0.7];
/// Keypoint marker colour — brighter than the line so keys stand out on it.
const PATH_KEY_COLOR: [f32; 4] = [0.65, 1.0, 0.75, 0.95];
/// Spline samples per segment. Rebuilt only when the path changes (see
/// `App::refresh_path_lines`), so 16 is smooth at any sane path length.
const PATH_SAMPLES_PER_SEGMENT: usize = 16;

/// Build the line list for a camera path: the eye's route as a polyline, with
/// a cross at every keypoint.
///
/// Drawn at the eye positions rather than the focus positions: the path is a
/// route through space, and that's the thing you're positioning against when
/// this is on screen. There's deliberately no playhead marker — the path is
/// only ever drawn while it *isn't* playing (see `App::visible_path`), because
/// during playback the camera is standing on the line and drawing it just
/// smears the shot.
pub fn build_path(path: &CameraPath) -> Vec<LineVertex> {
    if !path.playable() {
        return Vec::new();
    }
    let samples = path.segments() * PATH_SAMPLES_PER_SEGMENT;
    let mut verts = Vec::with_capacity(2 * (samples + path.keys.len() * 3));

    // Marker size scales with the path's own radius so it stays legible for
    // both a close-in fly-through and a wide orbit.
    let mean_distance =
        path.keys.iter().map(|k| k.distance).sum::<f32>() / path.keys.len() as f32;
    let tick = (mean_distance * 0.04).max(1e-3);

    let mut prev = path.sample(0.0).eye();
    for i in 1..=samples {
        let p = path.sample(i as f32 / samples as f32).eye();
        seg(&mut verts, prev, p, PATH_COLOR);
        prev = p;
    }
    // An open path's polyline stops at the last key; a closed one has already
    // come back round, because sample(1.0) wraps to the start.

    for key in &path.keys {
        push_cross(&mut verts, key.to_camera().eye(), tick, PATH_KEY_COLOR);
    }

    verts
}

/// Three axis-aligned struts through a point — a marker that reads as a
/// position from any camera angle without needing to billboard.
fn push_cross(verts: &mut Vec<LineVertex>, at: Vec3, size: f32, color: [f32; 4]) {
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        seg(verts, at - axis * size, at + axis * size, color);
    }
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

    /// The cage has to be drawn in the same frame the group is generated in.
    ///
    /// This is the check that makes hardcoded vertex lists safe: every element
    /// of the group must permute the cage's vertices among themselves. A cage
    /// in the wrong orientation looks perfectly plausible on screen — a
    /// tetrahedron is a tetrahedron — while telling you the symmetry is
    /// somewhere it isn't, which is the one thing a symmetry gizmo must never
    /// do.
    #[test]
    fn the_cage_matches_its_group() {
        use crate::symmetry::{OrbitKind, OrbitColor, Symmetry};

        for (kind, corners, edges) in [
            (OrbitKind::Tetrahedral, 4, 6),
            (OrbitKind::Octahedral, 8, 12),
            (OrbitKind::Icosahedral, 12, 30),
        ] {
            let cage = super::cage_edges(kind);
            assert_eq!(cage.len(), edges, "{:?}: wrong edge count", kind);

            let mut verts: Vec<Vec3> = Vec::new();
            for (a, b) in &cage {
                for v in [a, b] {
                    if !verts.iter().any(|u| u.distance(*v) < 1e-3) {
                        verts.push(*v);
                    }
                }
            }
            assert_eq!(verts.len(), corners, "{:?}: wrong vertex count", kind);

            let group = Symmetry::new(kind, Vec3::Y, false, OrbitColor::Shared).unwrap();
            for g in group.elements() {
                for v in &verts {
                    let moved = g.transform_point3(*v);
                    assert!(
                        verts.iter().any(|u| u.distance(moved) < 1e-3),
                        "{:?}: a group element moves the cage off itself — the \
                         gizmo and the group are in different frames",
                        kind
                    );
                }
            }
        }
    }

    /// A cyclic group's drawing has to have as many spokes as the group has
    /// folds, because counting them is how the fold count is read.
    #[test]
    fn a_cyclic_axis_draws_one_spoke_per_fold() {
        use crate::symmetry::{OrbitKind, OrbitColor, Symmetry};
        for n in [3u32, 5, 12] {
            let sym =
                Symmetry::new(OrbitKind::Cyclic(n), Vec3::Y, false, OrbitColor::Shared).unwrap();
            let verts = super::build_symmetry(&sym, 1.0);
            let spokes = verts.iter().filter(|v| v.color == super::SYM_SPOKE_COLOR).count() / 2;
            assert_eq!(spokes, n as usize, "C{n} should draw {n} spokes");
            assert!(
                verts.iter().all(|v| v.position.iter().all(|c| c.is_finite())),
                "symmetry geometry must never contain NaN"
            );
        }
    }

    /// The axis is the group's, not the world's — swinging it has to swing the
    /// drawing.
    #[test]
    fn the_drawn_axis_follows_the_group() {
        use crate::symmetry::{OrbitKind, OrbitColor, Symmetry};
        let axis = Vec3::new(0.3, 0.6, -0.74).normalize();
        let sym = Symmetry::new(OrbitKind::Cyclic(4), axis, false, OrbitColor::Shared).unwrap();
        let verts = super::build_symmetry(&sym, 1.0);
        // The axis line is the first segment drawn, from -axis to +axis.
        let end = Vec3::from(verts[1].position).normalize();
        assert!((end - axis).length() < 1e-3, "the axis line should lie along {axis:?}");
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
