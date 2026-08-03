//! Rolling gradients that are worth keeping.
//!
//! Random colours are easy; random *palettes* are not, and the difference is
//! two constraints that only become obvious after a hundred bad rolls:
//!
//! - **Luminance has to go somewhere.** The renderer has no lights, so the
//!   palette is the shading. A gradient that sits at one brightness renders
//!   flat however pretty its hues are.
//! - **But the colormap is cyclic**, so a monotone dark→bright ramp puts a
//!   hard seam at index 0. Luminance must rise and fall *once* across the
//!   gradient, returning to where it started.
//!
//! Both are enforced by [`score`], which every generator's candidates are run
//! through — roll a few, keep the best, so a generator is allowed to
//! occasionally produce something dull without the caller ever seeing it.
//!
//! Three generators, and they are genuinely different instruments:
//! cosine palettes are smooth and slightly alien, harmony schemes are
//! recognisable colour theory, and the library is hand-authored.

use glam::Vec3;
use rand::Rng;

use super::{library, luminance, Cosine, Interpolate, Palette, Stop};

/// How many candidates each generator rolls before keeping its best.
const CANDIDATES: usize = 12;

/// Which generator produced (or should produce) a palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generator {
    /// Iñigo Quílez cosine palettes: smooth by construction, twelve numbers.
    Cosine,
    /// Base hue plus a classical scheme (analogous, complementary, triadic,
    /// split), with a deliberate luminance envelope.
    Harmony,
    /// A hand-authored library palette.
    Library,
}

impl Generator {
    pub const ALL: [Generator; 3] = [Generator::Cosine, Generator::Harmony, Generator::Library];

    pub fn name(&self) -> &'static str {
        match self {
            Generator::Cosine => "cosine",
            Generator::Harmony => "harmony",
            Generator::Library => "library",
        }
    }

    pub fn parse(s: &str) -> Option<Generator> {
        Generator::ALL.into_iter().find(|g| g.name().eq_ignore_ascii_case(s))
    }
}

/// Roll a palette from a generator chosen at random.
///
/// The library gets a decent share because hand-authored gradients are still
/// the highest quality per unit effort, but not so much that `--random-palette`
/// becomes a slow way of typing `--palette <name>`.
pub fn palette(rng: &mut impl Rng) -> Palette {
    let roll: f32 = rng.r#gen();
    let g = if roll < 0.45 {
        Generator::Cosine
    } else if roll < 0.80 {
        Generator::Harmony
    } else {
        Generator::Library
    };
    from(g, rng)
}

/// Roll a palette from a named generator.
pub fn from(g: Generator, rng: &mut impl Rng) -> Palette {
    match g {
        Generator::Library => library::random(rng),
        Generator::Cosine => best_of(rng, cosine),
        Generator::Harmony => best_of(rng, harmony),
    }
}

/// Roll `CANDIDATES` and keep the highest-scoring one.
fn best_of<R: Rng>(rng: &mut R, mut make: impl FnMut(&mut R) -> Palette) -> Palette {
    let mut best: Option<(f32, Palette)> = None;
    for _ in 0..CANDIDATES {
        let p = make(rng);
        let s = score(&p);
        if best.as_ref().is_none_or(|(bs, _)| s > *bs) {
            best = Some((s, p));
        }
    }
    best.expect("CANDIDATES > 0").1
}

/// How usable a gradient is, higher is better. Rewards a luminance sweep and
/// chroma, penalises a seam and a washed-out midrange.
pub fn score(p: &Palette) -> f32 {
    let n = 64;
    let samples: Vec<Vec3> = (0..n).map(|i| p.sample(i as f32 / n as f32)).collect();
    let lum: Vec<f32> = samples.iter().map(|&c| luminance(c)).collect();

    let min = lum.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = lum.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    // A full sweep from near-black to near-white is worth 1; less, less.
    let sweep = ((max - min) / 0.7).min(1.0);

    // Chroma: a palette that swept luminance but had no colour would be
    // `ash`, which is fine as one library entry and dull as a random roll.
    let chroma = samples.iter().map(|&c| super::chroma(c)).sum::<f32>() / n as f32;
    let chroma = (chroma / 0.10).min(1.0);

    // The seam: how big the wrap step is relative to a typical step. One
    // rise and one fall makes this small; a monotone ramp makes it huge.
    let steps: Vec<f32> = (0..n)
        .map(|i| (samples[i] - samples[(i + 1) % n]).length())
        .collect();
    let typical = steps.iter().sum::<f32>() / n as f32;
    let seam = if typical > 1e-5 { steps[n - 1] / typical } else { 0.0 };
    let seam_penalty = ((seam - 2.0) / 6.0).clamp(0.0, 1.0);

    sweep * 1.0 + chroma * 0.6 - seam_penalty * 1.2
}

/// A cosine palette. Integer frequencies keep it seamless; the amplitude is
/// pushed high enough to actually swing luminance.
fn cosine<R: Rng>(rng: &mut R) -> Palette {
    // One cycle per channel is the classic look; two gives a palette that
    // visits its hues twice, which reads as banding on a fractal and is worth
    // having occasionally but not often.
    let freq = if rng.r#gen::<f32>() < 0.8 { 1.0 } else { 2.0 };
    // Channels share a frequency — mixing them makes a muddy tangle rather
    // than a gradient — and are separated by phase, which is what produces
    // hue travel.
    let base_phase: f32 = rng.r#gen();
    let spread: f32 = rng.gen_range(0.08..0.42);

    let bias: f32 = rng.gen_range(0.35..0.55);
    let amp: f32 = rng.gen_range(0.30f32..0.50).min(bias).min(1.0 - bias);

    let jitter = |rng: &mut R| Vec3::new(
        rng.gen_range(-0.08..0.08),
        rng.gen_range(-0.08..0.08),
        rng.gen_range(-0.08..0.08),
    );
    let a = Vec3::splat(bias) + jitter(rng);
    let b = Vec3::splat(amp) + jitter(rng);

    let mut p = Palette::from_cosine(Cosine {
        a: a.clamp(Vec3::splat(0.15), Vec3::splat(0.85)),
        b: b.clamp(Vec3::splat(0.15), Vec3::splat(0.6)),
        c: Vec3::splat(freq),
        d: Vec3::new(base_phase, base_phase + spread, base_phase + spread * 2.0),
    });
    p.name = Some("random cosine".to_string());
    p
}

/// A classical harmony scheme, with a deliberate luminance envelope.
fn harmony<R: Rng>(rng: &mut R) -> Palette {
    let base: f32 = rng.gen_range(0.0..360.0);
    // Offsets from the base hue. Each scheme is a different amount of
    // tension; all of them beat picking N hues at random.
    let offsets: Vec<f32> = match rng.gen_range(0..4) {
        0 => vec![0.0, 22.0, 44.0, 66.0],          // analogous
        1 => vec![0.0, 30.0, 180.0, 210.0],        // complementary
        2 => vec![0.0, 120.0, 240.0],              // triadic
        _ => vec![0.0, 150.0, 210.0],              // split complementary
    };

    let n = rng.gen_range(5..=7).max(offsets.len());
    // Rise and fall once: value follows sin(pi t), so both ends of the cyclic
    // gradient are dark and the seam is invisible. This is the whole trick.
    let floor = rng.gen_range(0.03..0.12);
    let peak = rng.gen_range(0.9..1.0);
    // Saturation runs the other way — the bright end desaturates toward a
    // highlight, which is what makes a gradient read as lit rather than
    // merely colourful.
    let sat_low = rng.gen_range(0.05..0.25);
    let sat_high = rng.gen_range(0.75..1.0);

    // Where the peak sits. Off-centre is more interesting than exactly half.
    let peak_at: f32 = rng.gen_range(0.4..0.72);

    let stops = (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            // Envelope: 0 at t=0, 1 at t=peak_at, 0 again at t=1.
            let e = if t < peak_at {
                (t / peak_at * std::f32::consts::FRAC_PI_2).sin()
            } else {
                ((1.0 - t) / (1.0 - peak_at) * std::f32::consts::FRAC_PI_2).sin()
            };
            let hue = base + offsets[i % offsets.len()] + rng.gen_range(-8.0..8.0);
            let v = floor + (peak - floor) * e;
            let s = sat_high + (sat_low - sat_high) * e.powf(2.5);
            Stop { at: t, color: hsv_to_linear(hue.rem_euclid(360.0), s, v) }
        })
        .collect();

    let mut p = Palette::from_stops(stops);
    // Harmony palettes have widely separated hues by construction, which is
    // exactly the case where linear-RGB interpolation puts a grey hole in the
    // middle. Oklab is the whole reason it's offered.
    p.interpolate = if rng.r#gen::<f32>() < 0.7 { Interpolate::Oklab } else { Interpolate::Rgb };
    p.name = Some("random harmony".to_string());
    p
}

/// Perturb an existing palette: rotate hues, nudge stops, shift the gradient.
/// Used by the mutation operators so `U` explores colour as well as form.
pub fn perturb(p: &mut Palette, rng: &mut impl Rng, strength: f32) -> String {
    match rng.gen_range(0..4) {
        0 => {
            let deg = rng.gen_range(-60.0..60.0) * strength;
            for_each_color(p, |c| rotate_hue(c, deg));
            format!("palette hue {deg:+.0}°")
        }
        1 => {
            let d = rng.gen_range(-0.25..0.25) * strength;
            p.rotate = (p.rotate + d).rem_euclid(1.0);
            format!("palette rotate {d:+.2}")
        }
        2 => {
            p.reverse = !p.reverse;
            "palette reversed".to_string()
        }
        _ => {
            // Redistribute without recolouring: the same palette, with its
            // contrast landing somewhere else on the fractal. Only `Stops`
            // has control points to move, so a procedural palette gets the
            // equivalent operation on its phase instead — an operator that
            // silently did nothing would waste a mutation slot.
            let d = rng.gen_range(0.02..0.10) * strength;
            match &mut p.body {
                super::Body::Stops(stops) => {
                    for s in stops.iter_mut() {
                        s.at = (s.at + rng.gen_range(-d..d)).clamp(0.0, 0.999);
                    }
                    p.sort_stops();
                    format!("palette stops jittered {d:.2}")
                }
                super::Body::Cosine(c) => {
                    c.d += Vec3::new(
                        rng.gen_range(-d..d),
                        rng.gen_range(-d..d),
                        rng.gen_range(-d..d),
                    );
                    format!("palette phase jittered {d:.2}")
                }
                super::Body::Entries(_) => {
                    // 256 fixed entries have nothing to redistribute; rotating
                    // is the operation that means the same thing for them.
                    p.rotate = (p.rotate + d).rem_euclid(1.0);
                    format!("palette rotate {d:+.2}")
                }
            }
        }
    }
}

fn for_each_color(p: &mut Palette, mut f: impl FnMut(Vec3) -> Vec3) {
    match &mut p.body {
        super::Body::Stops(s) => s.iter_mut().for_each(|s| s.color = f(s.color)),
        super::Body::Entries(e) => e.iter_mut().for_each(|c| *c = f(*c)),
        // A cosine palette's hue lives in its phase, so rotate that instead of
        // trying to recolour a formula.
        super::Body::Cosine(c) => {
            let shifted = f(c.a);
            let delta = luminance(shifted) - luminance(c.a);
            c.d += Vec3::splat(delta.clamp(-0.5, 0.5) * 0.25);
        }
    }
    // The colours moved, so any library provenance is no longer accurate.
    p.name = None;
}

/// HSV (hue in degrees) → **linear** RGB. The HSV cube is a display-space
/// construct, so the result is treated as sRGB and decoded, which is what
/// makes a rolled palette sit in the same space as an imported one.
pub fn hsv_to_linear(h: f32, s: f32, v: f32) -> Vec3 {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Vec3::new(
        super::srgb8_to_linear(((r + m) * 255.0).round().clamp(0.0, 255.0) as u8),
        super::srgb8_to_linear(((g + m) * 255.0).round().clamp(0.0, 255.0) as u8),
        super::srgb8_to_linear(((b + m) * 255.0).round().clamp(0.0, 255.0) as u8),
    )
}

/// Rotate a linear-RGB colour's hue, preserving luminance-ish. Same idea as
/// `mutate::rotate_hue` but in Oklab, where a hue rotation is a rotation.
pub fn rotate_hue(c: Vec3, degrees: f32) -> Vec3 {
    let lab = super::linear_to_oklab(c);
    let (sin, cos) = degrees.to_radians().sin_cos();
    let rotated = Vec3::new(lab.x, lab.y * cos - lab.z * sin, lab.y * sin + lab.z * cos);
    super::oklab_to_linear(rotated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng(seed: u64) -> rand::rngs::StdRng {
        rand::rngs::StdRng::seed_from_u64(seed)
    }

    /// The point of the whole module: rolls beat the constraints, not just
    /// on average but on every seed.
    #[test]
    fn every_roll_sweeps_luminance_and_has_no_seam() {
        for seed in 0..48 {
            let mut r = rng(seed);
            let p = palette(&mut r);
            let name = p.describe();

            let (_, swing) = p.luminance_profile();
            assert!(swing > 0.12, "seed {seed} ({name}) is flat: swing {swing:.3}");

            let steps: Vec<f32> = (0..64)
                .map(|i| (p.sample(i as f32 / 64.0) - p.sample((i + 1) as f32 / 64.0)).length())
                .collect();
            let typical = steps.iter().sum::<f32>() / 64.0;
            assert!(
                steps[63] < typical * 6.0 + 1e-4,
                "seed {seed} ({name}) has a seam: {:.3} vs {typical:.3} typical",
                steps[63]
            );
        }
    }

    #[test]
    fn rolls_stay_in_gamut() {
        for seed in 0..24 {
            let p = palette(&mut rng(seed));
            for i in 0..256 {
                let c = p.sample(i as f32 / 256.0);
                assert!(
                    c.min_element() >= -1e-5 && c.max_element() <= 1.0 + 1e-5,
                    "seed {seed} entry {i} out of gamut: {c}"
                );
            }
        }
    }

    #[test]
    fn a_seed_reproduces_a_palette() {
        assert_eq!(palette(&mut rng(7)), palette(&mut rng(7)));
        assert_ne!(palette(&mut rng(7)), palette(&mut rng(8)));
    }

    #[test]
    fn each_generator_produces_its_own_kind() {
        assert!(matches!(from(Generator::Cosine, &mut rng(1)).body, super::super::Body::Cosine(_)));
        assert!(matches!(from(Generator::Harmony, &mut rng(1)).body, super::super::Body::Stops(_)));
        let lib = from(Generator::Library, &mut rng(1));
        assert!(library::names().contains(&lib.name.as_deref().unwrap()));
        for g in Generator::ALL {
            assert_eq!(Generator::parse(g.name()), Some(g));
        }
        assert_eq!(Generator::parse("nope"), None);
    }

    #[test]
    fn score_prefers_a_sweep_over_a_flat_gradient() {
        let flat = Palette::from_stops(vec![
            Stop { at: 0.0, color: Vec3::splat(0.5) },
            Stop { at: 0.5, color: Vec3::splat(0.52) },
        ]);
        let good = library::get("ember").unwrap();
        assert!(score(&good) > score(&flat));

        // ...and penalises a monotone ramp, whose seam is the whole gradient
        let ramp = Palette::from_stops(vec![
            Stop { at: 0.0, color: Vec3::ZERO },
            Stop { at: 0.999, color: Vec3::ONE },
        ]);
        assert!(score(&good) > score(&ramp), "a dark→bright ramp should lose on its seam");
    }

    #[test]
    fn perturb_changes_something_and_stays_valid() {
        for seed in 0..16 {
            let mut r = rng(seed);
            let mut p = palette(&mut r);
            let before = p.clone();
            let log = perturb(&mut p, &mut r, 1.0);
            assert!(!log.is_empty());
            assert_ne!(p, before, "seed {seed}: '{log}' changed nothing");
            for i in 0..256 {
                let c = p.sample(i as f32 / 256.0);
                assert!(c.min_element() >= -1e-5 && c.max_element() <= 1.0 + 1e-5);
            }
        }
    }

    #[test]
    fn hue_rotation_is_a_full_circle() {
        let c = Vec3::new(0.6, 0.15, 0.05);
        // Not exact: the halfway colour is outside the sRGB gamut and
        // `oklab_to_linear` clamps it back in, which loses a little chroma.
        // Staying in gamut matters more than reversibility here — a palette
        // entry the GPU can't represent is worse than one that drifted.
        let back = rotate_hue(rotate_hue(c, 180.0), 180.0);
        assert!((back - c).length() < 0.05, "{c} -> {back}");
        // 180° from a warm red is not a warm red
        assert!((rotate_hue(c, 180.0) - c).length() > 0.05);
        // Hue rotation is not supposed to move lightness
        let lum = |v: Vec3| super::super::luminance(v);
        assert!((lum(rotate_hue(c, 90.0)) - lum(c)).abs() < 0.12);
    }

    #[test]
    fn hsv_primaries_land_where_they_should() {
        assert_eq!(super::super::to_srgb8(hsv_to_linear(0.0, 1.0, 1.0)), [255, 0, 0]);
        assert_eq!(super::super::to_srgb8(hsv_to_linear(120.0, 1.0, 1.0)), [0, 255, 0]);
        assert_eq!(super::super::to_srgb8(hsv_to_linear(240.0, 1.0, 1.0)), [0, 0, 255]);
        assert_eq!(super::super::to_srgb8(hsv_to_linear(0.0, 0.0, 1.0)), [255, 255, 255]);
    }
}
