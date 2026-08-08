//! Random flame generator: a whole new `Scene` from nothing, for the Explore
//! window's dice button.
//!
//! Randomising an IFS is easy; randomising one that *renders* is not. Most
//! random parameter sets either collapse to a point or diverge to a haze, so
//! every candidate is run through a short CPU chaos game (the same `trace.rs`
//! walkers the GPU shader mirrors) and rejected unless it settles into a
//! bounded attractor with real extent on more than one axis. Up to
//! `MAX_ATTEMPTS` tries; the last candidate is kept if none pass, so the
//! button always produces something rather than hanging.
//!
//! Colour is rolled as well as form — the colour source (all three modes),
//! the gradient a palette-mode roll needs, and the background it all sits on.
//! Every draw comes off the caller's rng, so `--random --seed N` reproduces a
//! roll exactly, colours included.

use glam::{Mat4, Quat, Vec3};
use rand::Rng;

use crate::path::CameraPath;
use crate::scene::{
    generate_colormap, resolve_color_speeds, Scene, TransformSpec, NUM_VARIATIONS,
    VARIATION_NAMES,
};
use crate::trace::{self, AttractorStats};

/// Variations that stay bounded under repeated application in 3D. Notably
/// absent: `spherical`, whose 1/r² inversion reliably blows scenes out (see
/// AGENTS.md) — it's a fine thing to reach for by hand, a bad thing to roll.
const FRIENDLY: &[&str] = &[
    "sinusoidal",
    "swirl",
    "bubble",
    "fisheye",
    "julia",
    "horseshoe",
    "disc",
    "spiral",
    "bulb",
];

const MAX_ATTEMPTS: usize = 20;
/// Attractor radius bounds. Below the floor it's collapsed to a speck, above
/// the ceiling it's a diverging haze that no camera framing will save.
const MIN_RADIUS: f32 = 0.15;
const MAX_RADIUS: f32 = 8.0;
/// Per-axis standard deviation under which the attractor is degenerate (a
/// point or a line rather than a form). At least two axes must clear it.
const MIN_AXIS_SPREAD: f32 = 0.02;
/// Fraction of the bounding box's cells the attractor may fill before it
/// reads as a solid lump rather than a fractal. A set that fills space
/// uniformly puts a point in nearly every cell at this sample count; the
/// dusty, self-similar structure this renderer exists for does not.
const MAX_OCCUPANCY: f32 = 0.22;
/// The second-widest axis must be at least this fraction of the widest.
/// The absolute floor above only catches literal degeneracy; this catches
/// the far more common near-miss — a thin ribbon or filament that technically
/// has extent on two axes but reads as a line on screen.
const MIN_AXIS_RATIO: f32 = 0.25;

/// Build a random flame that passes the quality gate.
pub fn random_flame(rng: &mut impl Rng) -> Scene {
    let mut last = build_candidate(rng);
    for attempt in 0..MAX_ATTEMPTS {
        if let Some(stats) = measure_candidate(&last.transforms) {
            log::debug!(
                "Random flame attempt {}: r95 {:.3}, spread {:?}, occupancy {:.3} -> {}",
                attempt,
                stats.radius,
                stats.spread,
                stats.occupancy,
                if stats.acceptable() { "accept" } else { "reject" },
            );
            if stats.acceptable() {
                return finish(last, stats.center, stats.radius, rng);
            }
        }
        last = build_candidate(rng);
    }
    // Nothing passed — hand back the last roll anyway with a neutral framing
    // rather than looping forever or returning an error the UI can't act on.
    log::warn!("Random flame: no candidate passed the quality gate in {} tries", MAX_ATTEMPTS);
    finish(last, Vec3::ZERO, 1.0, rng)
}

/// A candidate before quality checking: just the parts the chaos game needs.
struct Candidate {
    transforms: Vec<TransformSpec>,
    names: Vec<Option<String>>,
    colors: Vec<Vec3>,
}

fn build_candidate(rng: &mut impl Rng) -> Candidate {
    // Transform count: 3 and 4 are the sweet spot for legible structure.
    let roll: f32 = rng.r#gen();
    let n = match roll {
        r if r < 0.15 => 2,
        r if r < 0.55 => 3,
        r if r < 0.85 => 4,
        _ => 5,
    };

    let mut transforms = Vec::with_capacity(n);
    for _ in 0..n {
        transforms.push(random_transform(rng));
    }

    // An IFS only converges if it contracts on average. Rescale everything if
    // the mean linear contraction is too close to 1.
    let mean: f32 = transforms
        .iter()
        .map(|t| mean_singular_value(t.matrix))
        .sum::<f32>()
        / n as f32;
    if mean > 0.85 {
        let fix = 0.75 / mean;
        for t in &mut transforms {
            let (s, r, p) = t.matrix.to_scale_rotation_translation();
            t.matrix = Mat4::from_scale_rotation_translation(s * fix, r, p);
        }
    }

    // Variations. Purely affine IFS are a real and worthwhile family, but a
    // *random* one is almost always a dull Sierpinski variant — the good ones
    // come from deliberate symmetry, not dice. So every roll carries some
    // nonlinearity, and only rarely stays affine.
    //
    // The flame gets one characteristic variation, applied to a subset of its
    // transforms at independently-rolled strengths. Picking a single kind per
    // flame (rather than one per transform) is what makes the result read as
    // a coherent form instead of a pile of unrelated distortions; varying the
    // strength per transform is what keeps it from looking uniform.
    if rng.r#gen::<f32>() >= 0.08 {
        let name = FRIENDLY[rng.gen_range(0..FRIENDLY.len())];
        let slot = VARIATION_NAMES.iter().position(|&v| v == name).unwrap();

        // How many transforms carry it. All of them reads as a global warp;
        // one reads as an accent. Both are good, so roll it.
        let touched = rng.gen_range(1..=n);
        let mut order: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            order.swap(i, rng.gen_range(0..=i));
        }

        // Mostly-low strengths with an occasional strong one: at high blend a
        // nonlinear variation tends to swamp the affine structure, and the
        // interesting flames usually live where the two are still arguing.
        for &idx in order.iter().take(touched) {
            let w = if rng.r#gen::<f32>() < 0.25 {
                rng.gen_range(0.55..1.0)
            } else {
                rng.gen_range(0.12..0.55)
            };
            let w = if rng.r#gen::<f32>() < 0.15 { -w } else { w };
            transforms[idx].variations[slot] = w;
            // Keep the total blend near 1 so the transform's overall scale
            // stays in the range the contraction check just established.
            transforms[idx].variations[0] = 1.0 - w.abs();
        }

        // Occasionally a second kind on one transform, for flames that need
        // somewhere for the eye to catch.
        if n >= 3 && rng.r#gen::<f32>() < 0.3 {
            let other = FRIENDLY[rng.gen_range(0..FRIENDLY.len())];
            let other_slot = VARIATION_NAMES.iter().position(|&v| v == other).unwrap();
            if other_slot != slot {
                let idx = rng.gen_range(0..n);
                let w = rng.gen_range(0.15..0.5);
                transforms[idx].variations[other_slot] = w;
                transforms[idx].variations[0] = (transforms[idx].variations[0] - w).max(0.0);
            }
        }
    }

    // Palette: one base hue plus a scheme, so the colors relate rather than
    // clash. color_value spreads the transforms around the gradient.
    let base_hue: f32 = rng.gen_range(0.0..360.0);
    let scheme = rng.gen_range(0..3);
    let colors: Vec<Vec3> = (0..n)
        .map(|i| {
            let offset = match scheme {
                0 => rng.gen_range(-25.0..25.0),                  // analogous
                1 => if i % 2 == 0 { 0.0 } else { 180.0 },        // complementary
                _ => (i % 3) as f32 * 120.0,                      // triadic
            };
            hsv_to_rgb(
                base_hue + offset,
                rng.gen_range(0.6..1.0),
                rng.gen_range(0.7..1.0),
            )
        })
        .collect();

    for (i, t) in transforms.iter_mut().enumerate() {
        t.color_value = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.5 };
    }

    Candidate {
        transforms,
        names: (0..n).map(|i| Some(format!("t{}", i))).collect(),
        colors,
    }
}

fn random_transform(rng: &mut impl Rng) -> TransformSpec {
    // Translation: uniform-ish inside a ball, so transforms don't all pile up
    // near the origin the way independent per-axis ranges would.
    let dir = Vec3::new(
        rng.gen_range(-1.0..1.0),
        rng.gen_range(-1.0..1.0),
        rng.gen_range(-1.0..1.0),
    );
    let dir = if dir.length_squared() < 1e-6 { Vec3::X } else { dir.normalize() };
    let translation = dir * rng.gen_range(0.0f32..0.9).cbrt() * 0.9;

    let rotation = random_quat(rng);

    // Log-uniform so 0.35 and 0.75 are equally likely to be *interesting*,
    // rather than the top of the range dominating.
    let base = (rng.gen_range(0.35f32.ln()..0.75f32.ln())).exp();
    let scale = if rng.r#gen::<f32>() < 0.3 {
        // Squash one axis: this is what turns blobs into sheets and ribbons.
        let mut s = Vec3::splat(base);
        s[rng.gen_range(0..3)] = base * rng.gen_range(0.15..0.6);
        s
    } else {
        Vec3::splat(base)
    };

    let mut variations = [0.0f32; NUM_VARIATIONS];
    variations[0] = 1.0;

    TransformSpec {
        matrix: Mat4::from_scale_rotation_translation(scale, rotation, translation),
        post_affine: Mat4::IDENTITY,
        color_value: 0.5,
        weight: rng.gen_range(0.5..2.0),
        color_speed: 0.5,
        explicit_color_speed: None,
        symmetry: None,
        variations,
    }
}

/// Uniformly-distributed random rotation (Shoemake's subgroup method).
fn random_quat(rng: &mut impl Rng) -> Quat {
    let u1: f32 = rng.r#gen();
    let u2: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let u3: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let (s1, s2) = ((1.0 - u1).sqrt(), u1.sqrt());
    Quat::from_xyzw(s1 * u2.sin(), s1 * u2.cos(), s2 * u3.sin(), s2 * u3.cos())
}

/// Measure a candidate flame. Everything is enabled at this point — a rolled
/// flame has no disabled transforms — so this is just `trace::measure` with
/// that spelled out.
fn measure_candidate(transforms: &[TransformSpec]) -> Option<AttractorStats> {
    trace::measure(transforms, &vec![true; transforms.len()])
}

/// Geometric mean of a matrix's axis lengths — a cheap stand-in for "how much
/// does this map shrink space", good enough for the contraction check.
fn mean_singular_value(m: Mat4) -> f32 {
    let (s, _, _) = m.to_scale_rotation_translation();
    (s.x.abs() * s.y.abs() * s.z.abs()).cbrt()
}

impl AttractorStats {
    fn acceptable(&self) -> bool {
        if !(self.radius.is_finite() && (MIN_RADIUS..=MAX_RADIUS).contains(&self.radius)) {
            return false;
        }
        // A form, not a point or a line: at least two axes with real extent,
        // both in absolute terms and relative to the widest one.
        let mut axes = [self.spread.x, self.spread.y, self.spread.z];
        axes.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        if axes[1] <= MIN_AXIS_SPREAD || axes[0] <= 0.0 {
            return false;
        }
        if axes[1] / axes[0] < MIN_AXIS_RATIO {
            return false;
        }
        // Structure, not a lump.
        self.occupancy <= MAX_OCCUPANCY
    }
}


/// Wrap an accepted candidate up as a full `Scene`, framed for its size.
fn finish(mut c: Candidate, center: Vec3, radius: f32, rng: &mut impl Rng) -> Scene {
    let color_speed = 0.5;
    let color_falloff = if rng.r#gen::<f32>() < 0.4 { rng.gen_range(0.6..1.2) } else { 0.0 };
    resolve_color_speeds(&mut c.transforms, color_speed, color_falloff);
    let colormap = generate_colormap(&c.colors);

    // Which colour source this flame renders through, and — for palette mode —
    // the gradient itself. Rolled before the scene is built so the background
    // can be tinted from whatever will actually be on screen.
    let (color_mode, palette) = random_color_mode(rng);

    let mut scene = Scene {
        name: format!("random-{}", crate::app::unix_timestamp()),
        author: "fracturize random flame".to_string(),
        // Scale the dot size with the attractor: the same point size that
        // looks dusty on a radius-3 form is a solid mass on a radius-0.3 one.
        point_size: (radius * 0.0018).clamp(0.0008, 0.012),
        points_per_frame: 100_000,
        point_count: 500_000,
        point_count_defaulted: true,
        decay: 0.8,
        color_speed,
        color_falloff,
        color_contrast: 1.0,
        // A little haze by default. A freshly rolled flame is a form nobody has
        // seen before, and with no shading and no occlusion there is nothing
        // else telling you which arm is in front — hand-authored scenes can opt
        // in, but a random one has no author to make that call.
        haze: 0.35,
        exposure: 1.0,
        transforms: c.transforms,
        transform_names: c.names,
        colors: c.colors,
        // Filled in below: `set_palette` / `set_color_mode` install the rolled
        // mode and rebuild the colormap from whichever source it selects.
        palette: None,
        color_mode: crate::scene::ColorMode::Transforms,
        colormap,
        camera_focus: center,
        // ~2.4x the 95th-percentile radius fills the frame without clipping
        // the tail. Framing on the raw maximum instead left most flames a
        // speck in the middle of an empty view.
        camera_distance: (radius * 2.4).clamp(0.8, 12.0),
        camera_orientation: crate::rot::Orientation::from_yaw_pitch_roll(
                crate::rot::Angle::ZERO,
                crate::rot::Angle::from_radians(0.3),
                crate::rot::Angle::ZERO,
            ),
        // Replaced below, once the colours it has to sit behind are settled.
        background: crate::scene::DEFAULT_BACKGROUND,
        camera_path: None::<CameraPath>,
        // Not rolled at random: infinite zoom needs a map that contracts on
        // every axis, and the generator has no reason to guarantee one.
        // `--zoom <n>` turns it on for a roll worth keeping.
        zoom: None,
    };

    match palette {
        Some(p) => scene.set_palette(p),
        None => scene.set_color_mode(color_mode),
    }
    let tint = tint(&scene, rng);
    scene.background = random_background(rng, tint);
    scene
}

/// Odds of each colour source. `transforms` keeps the majority because it is
/// the mode the per-transform colours were rolled *for* — the ring spreads
/// them around the gradient by `color_value` — but the other two are just as
/// much a part of what this renderer does, and a dice button that can only
/// produce one third of the palette space is under-selling the instrument.
const P_MIX: f32 = 0.25;
const P_PALETTE: f32 = 0.25;

/// Roll a colour source, plus the gradient if the roll wants one.
///
/// Palette mode is the only one that needs anything built: `mix` reads the
/// per-transform RGBs the candidate already has, and `transforms` is what
/// those RGBs were made for.
fn random_color_mode(rng: &mut impl Rng) -> (crate::scene::ColorMode, Option<crate::palette::Palette>) {
    use crate::scene::ColorMode;
    let roll: f32 = rng.r#gen();
    if roll < P_PALETTE {
        (ColorMode::Palette, Some(crate::palette::random::palette(rng)))
    } else if roll < P_PALETTE + P_MIX {
        (ColorMode::Mix, None)
    } else {
        (ColorMode::Transforms, None)
    }
}

/// A colour the flame actually renders in, to hang the background off.
///
/// Sampled rather than averaged: the mean of a complementary or triadic
/// scheme is mud, and a background tinted from mud is just grey. One real
/// colour out of the set keeps the background in the flame's family.
fn tint(scene: &Scene, rng: &mut impl Rng) -> Vec3 {
    match scene.color_mode {
        crate::scene::ColorMode::Palette => scene
            .palette
            .as_ref()
            .map(|p| p.sample(rng.r#gen()))
            .unwrap_or(Vec3::ONE),
        _ if scene.colors.is_empty() => Vec3::ONE,
        _ => scene.colors[rng.gen_range(0..scene.colors.len())],
    }
}

/// How often the background comes out dark.
const P_DARK_BACKGROUND: f32 = 2.0 / 3.0;
/// Brightest channel a dark background may reach, linear. The old fixed
/// default sat at 0.05 on its blue channel, so this is the band that has
/// always read as "the fractal is in the dark".
const DARK_BACKGROUND_LEVEL: f32 = 0.06;
/// And the band for the other third. Stops well short of white on purpose:
/// the render composites the flame *over* the background by coverage, so
/// contrast is the gap between the two, and rolled flame colours are bright
/// (value 0.7–1.0). A paper-white ground would swallow them.
const LIGHT_BACKGROUND_LEVEL: std::ops::Range<f32> = 0.06..0.35;

/// Roll a background that belongs behind `tint`.
///
/// Hue comes from the flame rather than from nowhere, so the two agree; the
/// level and the chroma are what make it a *ground* rather than another
/// element — dark and saturated, or mid and nearly neutral.
fn random_background(rng: &mut impl Rng, tint: Vec3) -> Vec3 {
    // Mostly the flame's own hue family, which reads as one scene; sometimes
    // its opposite, which makes the form pop off the ground. Rotating in
    // Oklab keeps the shift a hue shift rather than a lightness one.
    let shift = if rng.r#gen::<f32>() < 0.3 {
        rng.gen_range(150.0..210.0)
    } else {
        rng.gen_range(-40.0f32..40.0)
    };
    let hue = crate::palette::random::rotate_hue(tint, shift).max(Vec3::ZERO);

    // Normalise to the brightest channel so `level` means what it says
    // whatever colour came in. A colour with no chroma to keep (or one the
    // rotation pushed out of gamut) falls back to neutral.
    let peak = hue.max_element();
    let hue = if peak > 1e-4 { hue / peak } else { Vec3::ONE };

    let (level, chroma) = if rng.r#gen::<f32>() < P_DARK_BACKGROUND {
        // Deep and tinted: at this level chroma costs nothing legibility-wise,
        // and a near-black that is *some* colour beats one that is grey.
        (rng.gen_range(0.004..DARK_BACKGROUND_LEVEL), rng.gen_range(0.35..1.0))
    } else {
        // Mid and mostly neutral: a saturated light ground competes with the
        // flame for the eye, and loses interestingly to nothing.
        (rng.gen_range(LIGHT_BACKGROUND_LEVEL), rng.gen_range(0.08..0.4))
    };

    (Vec3::ONE.lerp(hue, chroma) * level).clamp(Vec3::ZERO, Vec3::ONE)
}

/// HSV (h in degrees, s/v in 0-1) to linear RGB (0-1). Mirrors the private
/// helper in app.rs; kept local so this module stays free of UI deps.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Vec3 {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Vec3::new(r, g, b) + Vec3::splat(v - c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    /// The generator's whole job is to not hand back garbage. Roll a batch
    /// across different seeds and hold every one to the same bar the gate
    /// applies — no panics, no NaN, no collapsed or exploded attractors.
    #[test]
    fn random_flames_are_renderable() {
        for seed in 0..12u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let scene = random_flame(&mut rng);

            assert!(
                (2..=5).contains(&scene.transforms.len()),
                "seed {}: transform count {} out of range",
                seed,
                scene.transforms.len()
            );
            assert_eq!(scene.colors.len(), scene.transforms.len());
            assert_eq!(scene.transform_names.len(), scene.transforms.len());
            assert!(scene.point_size > 0.0 && scene.point_size.is_finite());
            assert!(scene.camera_distance.is_finite() && scene.camera_distance > 0.0);

            for t in &scene.transforms {
                assert!(
                    t.matrix.to_cols_array().iter().all(|v| v.is_finite()),
                    "seed {}: non-finite matrix",
                    seed
                );
                assert!(t.weight > 0.0, "seed {}: non-positive weight", seed);
            }

            let stats = measure_candidate(&scene.transforms)
                .unwrap_or_else(|| panic!("seed {}: no walker could be built", seed));
            assert!(
                stats.acceptable(),
                "seed {}: attractor radius {:.3}, spread {:?} — the gate let a bad flame through",
                seed,
                stats.radius,
                stats.spread
            );
        }
    }

    /// The background is a scene parameter the renderer clears to and the
    /// haze fades toward, so a non-finite or out-of-range roll is a bad
    /// frame, not a bad colour.
    #[test]
    fn backgrounds_are_in_range_and_mostly_dark() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let mut dark = 0;
        const ROLLS: usize = 600;
        for _ in 0..ROLLS {
            let tint = hsv_to_rgb(rng.gen_range(0.0..360.0), 0.8, 0.9);
            let bg = random_background(&mut rng, tint);
            assert!(
                bg.to_array().iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)),
                "background {:?} out of range",
                bg
            );
            if bg.max_element() <= DARK_BACKGROUND_LEVEL {
                dark += 1;
            }
        }
        // 2/3, with room for the binomial spread at this sample count.
        let ratio = dark as f32 / ROLLS as f32;
        assert!((0.60..0.73).contains(&ratio), "dark background ratio {:.3}", ratio);
    }

    /// All three colour sources should show up on the dice, and a palette-mode
    /// flame must actually carry a palette — the mode without one renders
    /// through a fallback, which is a bug the generator shouldn't ship.
    #[test]
    fn all_color_modes_are_rolled_and_consistent() {
        use crate::scene::ColorMode;
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let mut seen = [0usize; 3];
        for _ in 0..300 {
            let (mode, palette) = random_color_mode(&mut rng);
            match mode {
                ColorMode::Transforms => seen[0] += 1,
                ColorMode::Palette => seen[1] += 1,
                ColorMode::Mix => seen[2] += 1,
            }
            assert_eq!(
                palette.is_some(),
                mode == ColorMode::Palette,
                "{:?} rolled the wrong palette state",
                mode
            );
        }
        assert!(seen.iter().all(|&n| n > 0), "not every colour mode was rolled: {:?}", seen);
    }

    /// Whatever mode a rolled flame lands in, it has to be renderable in it.
    #[test]
    fn rolled_flames_carry_their_color_mode() {
        for seed in 0..8u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let scene = random_flame(&mut rng);
            if scene.color_mode == crate::scene::ColorMode::Palette {
                assert!(scene.palette.is_some(), "seed {}: palette mode with no palette", seed);
            }
            assert!(
                scene.colormap.iter().flatten().all(|c| c.is_finite()),
                "seed {}: non-finite colormap",
                seed
            );
            assert!(
                scene.background.to_array().iter().all(|c| (0.0..=1.0).contains(c)),
                "seed {}: background {:?} out of range",
                seed,
                scene.background
            );
        }
    }

    /// `--random --seed n` promises the same flame twice. The colour rolls
    /// have to come off the seeded rng like everything else for that to hold.
    #[test]
    fn the_same_seed_gives_the_same_colours() {
        for seed in 0..4u64 {
            let a = random_flame(&mut rand::rngs::StdRng::seed_from_u64(seed));
            let b = random_flame(&mut rand::rngs::StdRng::seed_from_u64(seed));
            assert_eq!(a.color_mode, b.color_mode, "seed {}: mode not reproducible", seed);
            assert_eq!(a.background, b.background, "seed {}: background not reproducible", seed);
            assert_eq!(a.color_falloff, b.color_falloff, "seed {}: falloff not reproducible", seed);
        }
    }

    #[test]
    fn spherical_is_never_rolled() {
        // Guard the AGENTS.md rule: spherical's 1/r^2 inversion blows scenes
        // out, so it must never appear in a rolled flame.
        let slot = VARIATION_NAMES.iter().position(|&v| v == "spherical").unwrap();
        for seed in 0..24u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let scene = random_flame(&mut rng);
            for t in &scene.transforms {
                assert_eq!(t.variations[slot], 0.0, "seed {}: rolled spherical", seed);
            }
        }
    }
}
