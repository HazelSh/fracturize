//! Random scene mutations for evolutionary exploration
//!
//! One call to [`mutate`] applies 1-3 random operators to a scene — nudging
//! transforms, blending variations, shifting colors, occasionally duplicating
//! or deleting a transform — and returns human/LLM-readable descriptions of
//! what changed. Used by the in-app `U` key (with undo) and the offline
//! `--mutations` contact sheet.

use glam::{Mat4, Quat, Vec3, Vec4};
use rand::Rng;

use crate::scene::{Scene, NUM_VARIATIONS, VARIATION_NAMES};

/// Apply 1-3 random mutation operators. `strength` scales every perturbation
/// (1.0 = a noticeable but usually non-destructive step). Derived state
/// (color speeds, colormap) is refreshed before returning.
pub fn mutate(scene: &mut Scene, rng: &mut impl Rng, strength: f32) -> Vec<String> {
    let n_ops = 1
        + (rng.r#gen::<f32>() < 0.55) as usize
        + (rng.r#gen::<f32>() < 0.25) as usize;

    let mut log = Vec::new();
    for _ in 0..n_ops {
        apply_random_op(scene, rng, strength, &mut log);
    }

    crate::scene::resolve_color_speeds(
        &mut scene.transforms,
        scene.color_speed,
        scene.color_falloff,
    );
    scene.regenerate_colormap();
    log
}

/// Uniform random unit vector (rejection-sampled)
fn random_unit(rng: &mut impl Rng) -> Vec3 {
    loop {
        let v = Vec3::new(
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        );
        let len = v.length();
        if len > 1e-3 && len <= 1.0 {
            return v / len;
        }
    }
}

fn apply_random_op(scene: &mut Scene, rng: &mut impl Rng, strength: f32, log: &mut Vec<String>) {
    let n = scene.transforms.len();
    if n == 0 {
        return;
    }
    let i = rng.gen_range(0..n);
    let roll: f32 = rng.r#gen();

    if roll < 0.18 {
        // Translate jitter
        let d = random_unit(rng) * rng.gen_range(0.03..0.18) * strength;
        scene.transforms[i].matrix.w_axis += d.extend(0.0);
        log.push(format!("T{} translate ({:+.2},{:+.2},{:+.2})", i, d.x, d.y, d.z));
    } else if roll < 0.33 {
        // Rotate the linear part around a random axis through its origin
        let axis = random_unit(rng);
        let deg = rng.gen_range(5.0..30.0) * strength;
        let m = scene.transforms[i].matrix;
        let rot = Mat4::from_quat(Quat::from_axis_angle(axis, deg.to_radians()));
        let mut rotated = rot * Mat4::from_cols(m.x_axis, m.y_axis, m.z_axis, Vec4::W);
        rotated.w_axis = m.w_axis;
        scene.transforms[i].matrix = rotated;
        log.push(format!(
            "T{} rotate {:.0}° about ({:+.2},{:+.2},{:+.2})",
            i, deg, axis.x, axis.y, axis.z
        ));
    } else if roll < 0.48 {
        // Uniform scale
        let f = (rng.gen_range(-0.25..0.25) * strength).exp();
        let m = &mut scene.transforms[i].matrix;
        m.x_axis *= f;
        m.y_axis *= f;
        m.z_axis *= f;
        log.push(format!("T{} scale ×{:.2}", i, f));
    } else if roll < 0.60 {
        // Chaos-game weight
        let f = (rng.gen_range(-0.5..0.5) * strength).exp();
        let w = &mut scene.transforms[i].weight;
        *w = (*w * f).clamp(0.01, 100.0);
        log.push(format!("T{} weight ×{:.2}", i, f));
    } else if roll < 0.78 {
        // Variation blend: usually adjust an existing slot, sometimes
        // introduce a new one
        let existing: Vec<usize> = scene.transforms[i]
            .variations
            .iter()
            .enumerate()
            .filter(|&(_, &w)| w != 0.0)
            .map(|(s, _)| s)
            .collect();

        let introduce = existing.is_empty() || rng.r#gen::<f32>() < 0.4;
        if introduce {
            let slot = rng.gen_range(1..NUM_VARIATIONS); // never "introduce" linear
            let w = rng.gen_range(0.08..0.35) * strength;
            scene.transforms[i].variations[slot] += w;
            log.push(format!("T{} +{} {:.2}", i, VARIATION_NAMES[slot], w));
        } else {
            let slot = existing[rng.gen_range(0..existing.len())];
            let dw = rng.gen_range(-0.15..0.15) * strength;
            let w = &mut scene.transforms[i].variations[slot];
            *w = (*w + dw).clamp(-4.0, 4.0);
            if w.abs() < 0.03 {
                *w = 0.0;
                log.push(format!("T{} -{} (removed)", i, VARIATION_NAMES[slot]));
            } else {
                log.push(format!("T{} {} {:+.2} -> {:.2}", i, VARIATION_NAMES[slot], dw, w));
            }
        }
    } else if roll < 0.87 {
        // Hue shift of the transform's gradient color
        let deg = rng.gen_range(-90.0..90.0) * strength;
        let c = scene.colors[i];
        scene.colors[i] = rotate_hue(c, deg);
        log.push(format!("T{} hue {:+.0}°", i, deg));
    } else if roll < 0.92 {
        // Colormap index shift
        let d = rng.gen_range(-0.2..0.2) * strength;
        let cv = &mut scene.transforms[i].color_value;
        *cv = (*cv + d).rem_euclid(1.0);
        log.push(format!("T{} color_value {:+.2}", i, d));
    } else if roll < 0.97 && n < 8 {
        // Duplicate with a translate nudge
        let mut spec = scene.transforms[i].clone();
        let d = random_unit(rng) * rng.gen_range(0.05..0.2) * strength;
        spec.matrix.w_axis += d.extend(0.0);
        scene.transforms.push(spec);
        scene
            .transform_names
            .push(scene.transform_names[i].as_ref().map(|s| format!("{} mut", s)));
        scene.colors.push(scene.colors[i]);
        log.push(format!("duplicate T{} -> T{}", i, n));
    } else if n > 2 {
        scene.transforms.remove(i);
        scene.transform_names.remove(i);
        scene.colors.remove(i);
        log.push(format!("delete T{}", i));
    } else {
        // Fallback when structural ops aren't allowed: small translate
        let d = random_unit(rng) * 0.05 * strength;
        scene.transforms[i].matrix.w_axis += d.extend(0.0);
        log.push(format!("T{} translate ({:+.2},{:+.2},{:+.2})", i, d.x, d.y, d.z));
    }
}

/// Rotate a color's hue by `deg` degrees, preserving saturation/value
fn rotate_hue(c: Vec3, deg: f32) -> Vec3 {
    // Minimal RGB<->HSV roundtrip (matches app.rs helpers)
    let max = c.x.max(c.y).max(c.z);
    let min = c.x.min(c.y).min(c.z);
    let delta = max - min;
    let h = if delta < 1e-6 {
        0.0
    } else if max == c.x {
        60.0 * (((c.y - c.z) / delta).rem_euclid(6.0))
    } else if max == c.y {
        60.0 * ((c.z - c.x) / delta + 2.0)
    } else {
        60.0 * ((c.x - c.y) / delta + 4.0)
    };
    let s = if max < 1e-6 { 0.0 } else { delta / max };
    let v = max;

    let h = (h + deg).rem_euclid(360.0) / 60.0;
    let chroma = v * s;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    Vec3::new(r, g, b) + Vec3::splat(v - chroma)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256PlusPlus;

    fn test_scene(dir_name: &str) -> Scene {
        let dir = std::env::temp_dir().join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("base.toml");
        std::fs::write(
            &path,
            r#"
[meta]
name = "Mutate Base"
point_size = 0.002

[[transform]]
translation = [0.0, 0.0, 0.5]
scale = 0.5
color = [1.0, 0.2, 0.2]

[[transform]]
translation = [0.0, 0.47, -0.17]
scale = 0.5
color = [0.2, 1.0, 0.2]

[[transform]]
translation = [-0.41, -0.24, -0.17]
scale = 0.5
color = [0.2, 0.2, 1.0]
"#,
        )
        .unwrap();
        Scene::load(&path).unwrap()
    }

    #[test]
    fn mutations_stay_consistent_and_saveable() {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(42);
        let dir = std::env::temp_dir().join("fracturize_mutate_consistent");
        for round in 0..50 {
            let mut scene = test_scene("fracturize_mutate_consistent");
            let log = mutate(&mut scene, &mut rng, 1.0);
            assert!(!log.is_empty(), "round {}: no ops logged", round);
            // Parallel arrays stay in sync
            assert_eq!(scene.transforms.len(), scene.colors.len());
            assert_eq!(scene.transforms.len(), scene.transform_names.len());
            assert!(scene.transforms.len() >= 2);
            // Everything stays finite
            for t in &scene.transforms {
                assert!(t.matrix.to_cols_array().iter().all(|v| v.is_finite()));
                assert!(t.weight.is_finite() && t.weight > 0.0);
            }
            // And the result round-trips through the TOML format
            let out = dir.join("mutated.toml");
            scene.save(&out).unwrap();
            Scene::load(&out).unwrap();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn seeded_mutation_is_deterministic() {
        let mut a = test_scene("fracturize_mutate_deterministic");
        let mut b = test_scene("fracturize_mutate_deterministic");
        let log_a = mutate(&mut a, &mut Xoshiro256PlusPlus::seed_from_u64(7), 1.0);
        let log_b = mutate(&mut b, &mut Xoshiro256PlusPlus::seed_from_u64(7), 1.0);
        assert_eq!(log_a, log_b);
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_deterministic")).ok();
    }
}
