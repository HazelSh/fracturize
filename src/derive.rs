//! `--derive <map> [--turn AXIS:DEG]... [--spin AXIS:DEG]...`: print a new
//! `[[transform]]` block built from an existing one.
//!
//! The job this does is small and it used to need scipy. The CRAFT move
//! "a defect that conforms is a motif moved off the group" means: take a
//! motif, carry it part of a group step round the symmetry axis — which moves
//! its *translation* as well as its rotation — then perhaps turn it within its
//! own plane. That is two quaternion products and a rotated vector, and it is
//! exactly the arithmetic that is silent when wrong: a sign error or the wrong
//! composition order still gives a plausible-looking map, somewhere else.
//!
//! It prints rather than edits, on purpose. A scene key that applied the turn
//! at load would have to survive every save the GUI makes — baked away, it
//! loses what the author wrote; kept, every edit has to be un-turned before
//! writing. A printed block has no such afterlife: paste it, and it is an
//! ordinary transform.
//!
//! The two operations, with the map `M = T · R · S`:
//!
//! - **`--turn`**: about a world axis through the origin, `M' = W · M`. The
//!   translation turns with it. This is the one for "the same motif, k/n of
//!   the way round its group".
//! - **`--spin`**: about the map's own axis, `R' = R · P`, translation kept.
//!   Scale is applied first, in the map's own frame, so a squashed axis spins
//!   with it — which is what turning a thin crack within its wall needs.
//!
//! Turns compose on the left and spins on the right, so the order *between* the
//! two lists doesn't matter; within each list they apply in the order given.

use glam::{Mat4, Vec3};

use crate::rot::{Orientation, Trs};
use crate::scene::Scene;

/// One `AXIS:DEG` — `x|y|z`, or three comma-separated numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisTurn {
    pub axis: Vec3,
    pub degrees: f32,
}

pub fn parse_axis_turn(s: &str) -> Result<AxisTurn, String> {
    let (axis, deg) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("'{s}': expected AXIS:DEGREES, e.g. y:11.25 or 0,1,1:30"))?;
    let degrees: f32 = deg
        .trim()
        .parse()
        .map_err(|_| format!("'{s}': '{deg}' is not a number of degrees"))?;
    let axis = match axis.trim() {
        "x" | "X" => Vec3::X,
        "y" | "Y" => Vec3::Y,
        "z" | "Z" => Vec3::Z,
        v => {
            let parts: Vec<f32> = v
                .split(',')
                .map(|p| p.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .map_err(|_| format!("'{s}': axis '{v}' is not x, y, z or X,Y,Z"))?;
            if parts.len() != 3 {
                return Err(format!("'{s}': axis '{v}' needs three components"));
            }
            Vec3::new(parts[0], parts[1], parts[2])
        }
    };
    if axis.length_squared() < 1e-12 {
        return Err(format!("'{s}': the axis has no direction"));
    }
    Ok(AxisTurn { axis: axis.normalize(), degrees })
}

/// `M` turned about world axes (`turns`, first to last) and spun about its own
/// (`spins`, first to last).
pub fn derive_matrix(m: Mat4, turns: &[AxisTurn], spins: &[AxisTurn]) -> Result<Mat4, String> {
    let mut trs = Trs::of(m);
    if !trs.is_faithful(m) {
        return Err("this map is sheared, so it has no rotation to turn; remove the \
                    shear first (Remove shear, in the Transforms window)"
            .into());
    }
    for t in turns {
        let w = Orientation::from_axis_angle(t.axis, t.degrees.to_radians());
        trs.rotation = trs.rotation.then(w);
        trs.translation = w.rotate(trs.translation);
    }
    for p in spins {
        let r = Orientation::from_axis_angle(p.axis, p.degrees.to_radians());
        // `then` reads both in world terms; a body-frame spin is the spin
        // applied first, with the map's rotation after it.
        trs.rotation = r.then(trs.rotation);
    }
    Ok(trs.matrix())
}

/// The whole command: resolve the map, derive it, and return the block to
/// print — a comment saying how it was made, then ordinary TOML.
pub fn run(
    scene: &Scene,
    reference: &str,
    turns: &[AxisTurn],
    spins: &[AxisTurn],
) -> Result<String, String> {
    let i = crate::scene::resolve_transform_ref(reference, &scene.transform_names)
        .map_err(|e| format!("--derive: {e}"))?;
    let mut s = scene.clone();
    s.transforms[i].matrix = derive_matrix(s.transforms[i].matrix, turns, spins)
        .map_err(|e| format!("--derive {reference}: {e}"))?;
    let base = scene.transform_names[i].clone().unwrap_or_else(|| format!("transform {i}"));
    if !turns.is_empty() || !spins.is_empty() {
        s.transform_names[i] = Some(format!("{base}-derived"));
    }
    let block = s.transform_toml(i)?;

    let describe = |label: &str, list: &[AxisTurn]| -> Option<String> {
        (!list.is_empty()).then(|| {
            let parts: Vec<String> = list
                .iter()
                .map(|t| {
                    format!(
                        "{}° about ({:.3}, {:.3}, {:.3})",
                        t.degrees, t.axis.x, t.axis.y, t.axis.z
                    )
                })
                .collect();
            format!("{label} {}", parts.join(", then "))
        })
    };
    let how: Vec<String> = [describe("turned", turns), describe("spun", spins)]
        .into_iter()
        .flatten()
        .collect();
    let mut out = String::new();
    if how.is_empty() {
        out.push_str(&format!("# {base}, as the scene has it\n"));
    } else {
        out.push_str(&format!("# {base}, {}\n", how.join("; ")));
        out.push_str("# a new map outside any [[symmetry]] block; rename it before pasting\n");
    }
    out.push_str(&block);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trs(t: [f32; 3], s: [f32; 3], r: Orientation) -> Mat4 {
        Trs { scale: Vec3::from(s), rotation: r, translation: Vec3::from(t) }.matrix()
    }

    #[test]
    fn parses_named_and_vector_axes() {
        assert_eq!(parse_axis_turn("y:11.25").unwrap(), AxisTurn { axis: Vec3::Y, degrees: 11.25 });
        let t = parse_axis_turn("0,0,2:-40").unwrap();
        assert_eq!((t.axis, t.degrees), (Vec3::Z, -40.0));
        assert!(parse_axis_turn("y").is_err());
        assert!(parse_axis_turn("0,0,0:5").is_err());
        assert!(parse_axis_turn("w:5").is_err());
    }

    /// Kintsugi's crack, which was worked out by hand with scipy: `rim`
    /// turned half a Cyc8 step about Y, then spun 40° about its own z.
    #[test]
    fn reproduces_the_kintsugi_crack() {
        let rim = trs(
            [0.0, 0.55, 0.95],
            [0.36, 0.30, 0.36],
            Orientation::from_xyz_degrees([-28.0, 0.0, 0.0]),
        );
        let m = derive_matrix(
            rim,
            &[AxisTurn { axis: Vec3::Y, degrees: 11.25 }],
            &[AxisTurn { axis: Vec3::Z, degrees: 40.0 }],
        )
        .unwrap();
        let out = Trs::of(m);
        assert!((out.translation - Vec3::new(0.1853, 0.55, 0.9317)).length() < 1e-3);
        // scipy: Ry(11.25°) · Rx(-28°) · Rz(40°), as a rotation vector
        let want = crate::rot::Turn::from_rotation_vector(Vec3::new(-0.401, 0.3554, 0.7298)).exp();
        assert!(out.rotation.angle_to(want) < 1e-3, "{:?}", out.rotation);
        assert!((out.scale - Vec3::new(0.36, 0.30, 0.36)).length() < 1e-4);
    }

    #[test]
    fn a_full_group_step_lands_on_another_copy() {
        // A Cyc8 motif turned by a whole step is the next element of its orbit.
        let m = trs([0.0, 0.2, 1.0], [0.3; 3], Orientation::from_xyz_degrees([-20.0, 0.0, 0.0]));
        let g = Mat4::from_rotation_y(45f32.to_radians());
        let d = derive_matrix(m, &[AxisTurn { axis: Vec3::Y, degrees: 45.0 }], &[]).unwrap();
        assert!(d.abs_diff_eq(g * m, 1e-5));
    }

    #[test]
    fn a_spin_keeps_the_translation() {
        let m = trs([0.3, 0.1, -0.2], [0.02, 0.4, 0.4], Orientation::IDENTITY);
        let d = derive_matrix(m, &[], &[AxisTurn { axis: Vec3::Z, degrees: 90.0 }]).unwrap();
        assert!((Trs::of(d).translation - Vec3::new(0.3, 0.1, -0.2)).length() < 1e-6);
        // the thin local x axis now points along world y
        let x = d.transform_vector3(Vec3::X);
        assert!((x - Vec3::new(0.0, 0.02, 0.0)).length() < 1e-5, "{x:?}");
    }
}
