//! Fitting an IFS to a reference image — "that, but I made it fractally".
//!
//! See LIKENESS-PLAN.md. The short version, because it inverts what the
//! literature is optimising for and code written against the literature will
//! be wrong here:
//!
//! > **Never give the solver the ability to succeed.** Choose a map budget and
//! > a map class too small to hold the subject, and spend the residual error
//! > deliberately.
//!
//! A perfect reconstruction would be a photograph and would be worthless —
//! nothing about it would say *this thing was made of copies of itself*. What
//! we want is the subject assembled out of ghosts of itself at three or four
//! scales, and that is what six affine maps and a transport loss give you
//! whether you ask nicely or not.
//!
//! The output is an ordinary `scenes/*.toml`. Not a new format and not a
//! locked artifact: the transform panel opens it, the gizmos push it around,
//! `mutate` wrecks it and `renorm` zooms into it. The solver is an authoring
//! assist and it hands off.

pub mod loss;
pub mod solve;
pub mod target;

use glam::{Vec2, Vec3};
use image::{Rgb, RgbImage};

use crate::scene::{ColorMode, Scene};

/// A point cloud with a colour per point.
///
/// Colour is carried rather than recovered afterwards, which is what lets it
/// *drive* geometry: maps get pulled toward regions that match their colour,
/// so the brass and the cyan core come out as separate structures instead of
/// one structure tinted later (LIKENESS-PLAN.md §0.7).
pub struct Cloud {
    pub pos: Vec<Vec2>,
    pub col: Vec<Vec3>,
}

// -------------------------------------------------------- normalisation

/// The centring and scaling that puts a cloud in the comparison box.
#[derive(Clone, Copy, Debug)]
pub struct Norm {
    pub center: Vec2,
    pub scale: f32,
}

/// Half-width of the box both clouds are normalised into — the paper's
/// convention, 25% padding inside the unit square.
pub(crate) const BOX: f32 = 0.75;

/// Where a cloud sits and how big it is, measured robustly.
///
/// Median and a high percentile rather than centroid and bounding box,
/// because a chaos game throws occasional far outliers and a bounding box
/// would let one of them shrink the entire attractor to a speck in the middle
/// of the frame. Target and attractor go through exactly this function, so
/// the loss compares two clouds in one convention and nothing about overall
/// placement leaks into it.
pub fn normalizer(pos: &[Vec2]) -> Norm {
    if pos.is_empty() {
        return Norm { center: Vec2::ZERO, scale: 1.0 };
    }
    let center = Vec2::new(
        median(pos.iter().map(|p| p.x)),
        median(pos.iter().map(|p| p.y)),
    );
    let half = percentile(pos.iter().map(|p| (p.x - center.x).abs()), 0.98)
        .max(percentile(pos.iter().map(|p| (p.y - center.y).abs()), 0.98));
    Norm { center, scale: BOX / half.max(1e-6) }
}

impl Norm {
    pub fn apply(&self, p: Vec2) -> Vec2 {
        (p - self.center) * self.scale
    }
}

pub fn normalize(mut cloud: Cloud) -> Cloud {
    let n = normalizer(&cloud.pos);
    for p in &mut cloud.pos {
        *p = n.apply(*p);
    }
    cloud
}

pub fn normalize_walk(mut w: solve::Walk) -> solve::Walk {
    let n = normalizer(&w.pos);
    for p in &mut w.pos {
        *p = n.apply(*p);
    }
    w
}

pub(crate) fn sorted(v: impl Iterator<Item = f32>) -> Vec<f32> {
    let mut s: Vec<f32> = v.filter(|x| x.is_finite()).collect();
    s.sort_unstable_by(f32::total_cmp);
    s
}

pub(crate) fn median(v: impl Iterator<Item = f32>) -> f32 {
    let s = sorted(v);
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}

/// `q` in 0..1.
pub(crate) fn percentile(v: impl Iterator<Item = f32>, q: f32) -> f32 {
    let s = sorted(v);
    if s.is_empty() {
        return 0.0;
    }
    s[(((s.len() - 1) as f32) * q).round() as usize]
}

// ------------------------------------------------------------- pictures

/// Render a cloud the way the app would: accumulate density and colour, then
/// tone-map. Only for looking at — the fit never sees an image.
fn splat(pos: &[Vec2], col: &[Vec3], size: u32) -> RgbImage {
    let n = (size * size) as usize;
    let mut acc = vec![0.0f32; n];
    let mut rgb = vec![Vec3::ZERO; n];

    for (p, c) in pos.iter().zip(col) {
        let x = ((p.x * 0.5 + 0.5) * (size - 1) as f32).round();
        let y = ((1.0 - (p.y * 0.5 + 0.5)) * (size - 1) as f32).round();
        if !(0.0..size as f32).contains(&x) || !(0.0..size as f32).contains(&y) {
            continue;
        }
        let i = y as usize * size as usize + x as usize;
        acc[i] += 1.0;
        rgb[i] += *c;
    }

    let peak = acc
        .iter()
        .copied()
        .fold(0.0f32, f32::max)
        .max(1.0);
    let mut out = RgbImage::new(size, size);
    for (i, px) in out.pixels_mut().enumerate() {
        if acc[i] <= 0.0 {
            continue;
        }
        // Log tone curve: a chaos game's density spans orders of magnitude and
        // a linear map shows the fixed points and nothing else.
        let tone = (1.0 + acc[i] * 6.0).ln() / (1.0 + peak * 6.0).ln();
        let mean = rgb[i] / acc[i];
        let c = (mean * tone.powf(0.55) * 1.7).clamp(Vec3::ZERO, Vec3::ONE);
        *px = Rgb(crate::palette::to_srgb8(c));
    }
    out
}

/// A single-channel map as a greyscale cell.
fn grid_image(g: &target::Grid) -> RgbImage {
    let mut im = RgbImage::new(g.w as u32, g.h as u32);
    for (i, px) in im.pixels_mut().enumerate() {
        let v = (g.v[i].clamp(0.0, 1.0) * 255.0) as u8;
        *px = Rgb([v, v, v]);
    }
    im
}

fn reference_image(r: &target::Reference) -> RgbImage {
    let mut im = RgbImage::new(r.w as u32, r.h as u32);
    for (i, px) in im.pixels_mut().enumerate() {
        let c = r.srgb[i];
        *px = Rgb([
            (c.x.clamp(0.0, 1.0) * 255.0) as u8,
            (c.y.clamp(0.0, 1.0) * 255.0) as u8,
            (c.z.clamp(0.0, 1.0) * 255.0) as u8,
        ]);
    }
    im
}

/// Lay cells out on a black sheet, each fitted into its square without
/// stretching — these get compared by eye, and a stretched aspect ratio is a
/// difference you will read as a difference in the fit.
fn sheet(cells: &[RgbImage], cols: usize, cell: u32) -> RgbImage {
    let rows = cells.len().div_ceil(cols.max(1));
    let mut out = RgbImage::new(cols as u32 * cell, rows as u32 * cell);
    for (i, im) in cells.iter().enumerate() {
        let k = (cell as f32 / im.width() as f32).min(cell as f32 / im.height() as f32);
        let (w, h) = (
            ((im.width() as f32 * k) as u32).max(1),
            ((im.height() as f32 * k) as u32).max(1),
        );
        let scaled = image::imageops::resize(im, w, h, image::imageops::FilterType::Lanczos3);
        let ox = (i % cols) as u32 * cell + (cell - w) / 2;
        let oy = (i / cols) as u32 * cell + (cell - h) / 2;
        image::imageops::overlay(&mut out, &scaled, ox as i64, oy as i64);
    }
    out
}

// ---------------------------------------------------------------- emit

/// Frame the fitted attractor and hand back a scene.
///
/// **The framing is part of the answer, not scratch work.** The fit centres
/// and scales the attractor on every evaluation in order to compare it with
/// the target; if that transform is thrown away, opening the result shows a
/// correct fractal from an angle and distance that hide the likeness
/// entirely, which looks exactly like a failed fit.
///
/// It goes in the camera, and *only* in the camera. An earlier version baked
/// it into the maps by conjugation — `S(x) = k(x − c)` sends `f(x) = Mx + t`
/// to `f'(y) = My + k(Mc + t − c)`, which is tidy, keeps the maps writable and
/// puts the attractor honestly at the origin. It is also wrong as soon as a
/// map is anything but affine.
///
/// The walk applies `post ∘ variations ∘ affine`, so conjugating the affine
/// part alone computes `V ∘ S A S⁻¹` where the conjugate of the whole map is
/// `S ∘ V ∘ A ∘ S⁻¹`. Those differ, because a variation does not commute with
/// a similarity: `spherical` is `p/r²`, `boxfold` clamps at ±1, `bulb` works
/// on a radius — every one of them is defined about the origin at a fixed
/// scale, so moving and resizing the space underneath changes what they do.
/// The symmetry group has the same problem for the same reason, and only
/// uniform scaling and axial translation ever escaped it.
///
/// A camera has none of these constraints. It frames whatever is there,
/// exactly, whatever the maps are made of.
fn to_scene(params: &solve::Params, name: &str, walk: &solve::Walk) -> Scene {
    let norm = normalizer(&walk.pos);

    let mut scene = Scene::blank();
    scene.name = name.to_string();
    let transforms = params.to_transforms();
    scene.transform_names = (0..transforms.len()).map(|i| Some(format!("map-{i}"))).collect();
    scene.colors = params.colors();
    scene.transforms = transforms;
    scene.color_speed = params.color_speed;
    // Mix mode, because the fit measured mixed RGB: the walker carries a
    // 3-vector and distinct transform *combinations* get distinct colours,
    // which a scalar colormap index cannot represent. Emitting anything else
    // would show colours the fit never optimised.
    scene.set_color_mode(ColorMode::Mix);
    scene.regenerate_colormap();

    // Looking down −z, which is the direction the loss projected along.
    scene.camera_focus = Vec3::new(
        norm.center.x,
        norm.center.y,
        median(walk.world.iter().map(|p| p.z)),
    );
    scene.camera_distance = FRAMING / norm.scale;
    scene.camera_orientation = crate::rot::Orientation::IDENTITY;
    scene
}

/// Camera distance, in units of the fit's normalisation scale.
///
/// The fit judges everything inside `±BOX`, which in scene coordinates is a
/// half-extent of `BOX / scale`. A 45° vertical field of view covers
/// `0.414 · d`, so `BOX / 0.414 ≈ 1.81 / scale` exactly fills the frame.
/// Rounded up to 2, because a fit whose outermost band is cropped looks like a
/// fit that lost its edges — this leaves about 10% margin.
const FRAMING: f32 = 2.0;

// ----------------------------------------------------------------- run

pub struct Options {
    pub image: String,
    /// Where the scene goes. With several seeds this is a stem: `out.toml`
    /// becomes `out-000.toml`, `out-001.toml`, …
    pub out: String,
    pub target: target::Opts,
    pub solve: solve::Config,
    /// Fraction of the loss that measures colour rather than position.
    pub color_weight: f32,
    /// Fraction of the loss that compares density grids rather than
    /// one-dimensional transport. See [`loss`].
    pub grid_weight: f32,
    /// Independent fits from different starts. Every one is a legitimate
    /// output and the good one is usually not the best-scoring one, so the
    /// whole ladder gets written and the eye picks.
    pub seeds: usize,
    /// Contact strip of what the solver is aiming at: art, subject weight,
    /// target density, sampled cloud.
    pub target_sheet: Option<String>,
    /// Contact sheet of the target beside every fit.
    pub preview: Option<String>,
}

pub fn run(opts: &Options) -> Result<(), String> {
    use rand::SeedableRng;
    let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(opts.solve.seed);

    let t0 = std::time::Instant::now();
    let target = target::build(&opts.image, &opts.target, &mut rng)?;
    println!(
        "target: {} × {}, {} points, subject weight covers {:.0}% of the frame",
        target.reference.w,
        target.reference.h,
        target.cloud.pos.len(),
        target.coverage * 100.0,
    );

    if let Some(path) = &opts.target_sheet {
        let cells = [
            reference_image(&target.reference),
            grid_image(&target.weight),
            grid_image(&target.density),
            splat(&target.cloud.pos, &target.cloud.col, 480),
        ];
        sheet(&cells, 4, 480)
            .save(path)
            .map_err(|e| format!("Failed to write {}: {}", path, e))?;
        println!("wrote {path}  (art, subject weight, target density, sampled cloud)");
    }

    let objective = loss::Target::new(
        &target.cloud.pos,
        &target.cloud.col,
        opts.solve.points,
        opts.color_weight,
        opts.grid_weight,
        &mut rng,
    );

    // Seeds are independent fits over a shared target, so they are the one
    // thing here worth threading. The target bank is the expensive shared
    // state and it is read-only once built.
    let outcomes: Vec<solve::Outcome> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..opts.seeds.max(1))
            .map(|i| {
                let objective = &objective;
                let cfg = solve::Config {
                    seed: opts.solve.seed.wrapping_add(i as u64),
                    ..opts.solve
                };
                s.spawn(move || solve::fit(objective, &cfg))
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("fit thread panicked")).collect()
    });

    let stem = std::path::Path::new(&opts.out);
    let mut cells = vec![splat(&target.cloud.pos, &target.cloud.col, 480)];

    for (i, outcome) in outcomes.iter().enumerate() {
        let mut walk_rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(0xf17);
        // Raw, not normalised. `to_scene` needs to *measure* the framing in
        // order to bake it in, and handing it a cloud already in the box would
        // make that measurement the identity and quietly emit a scene framed
        // on wherever the un-normalised attractor happened to land.
        let raw = solve::walk(&outcome.params, 400_000, &mut walk_rng)
            .ok_or_else(|| format!("seed {i}: the fitted maps do not walk"))?;

        let path = if opts.seeds > 1 { numbered(stem, i) } else { opts.out.clone() };
        let name = std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "fit".to_string());
        let scene = to_scene(&outcome.params, &name, &raw);
        std::fs::write(&path, scene.to_toml_string()?)
            .map_err(|e| format!("Failed to write {}: {}", path, e))?;

        println!(
            "seed {:>3}: loss {:.4} (from {:.4}, {:.0}× better)  dimension {}  -> {}",
            opts.solve.seed.wrapping_add(i as u64),
            outcome.loss,
            outcome.initial_loss,
            outcome.initial_loss / outcome.loss.max(1e-9),
            match outcome.dimension {
                Some(d) => format!("{d:.2}"),
                None => "--".to_string(),
            },
            path,
        );
        let shown = normalize_walk(raw);
        cells.push(splat(&shown.pos, &shown.col, 480));
    }

    if let Some(path) = &opts.preview {
        let cols = (cells.len() as f32).sqrt().ceil() as usize;
        sheet(&cells, cols.max(2), 480)
            .save(path)
            .map_err(|e| format!("Failed to write {}: {}", path, e))?;
        println!("wrote {path}  (target first, then one cell per seed)");
    }

    println!("{} fit(s) in {:.1}s", outcomes.len(), t0.elapsed().as_secs_f32());
    Ok(())
}

fn numbered(stem: &std::path::Path, i: usize) -> String {
    let ext = stem.extension().map(|e| e.to_string_lossy().into_owned());
    let base = stem.with_extension("");
    match ext {
        Some(e) => format!("{}-{:03}.{}", base.display(), i, e),
        None => format!("{}-{:03}", base.display(), i),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};

    #[test]
    fn normalisation_is_the_same_question_for_both_clouds() {
        // The loss only means anything if target and attractor are put in the
        // box by identical code — a systematic scale difference between them
        // would be read as shape error the fit can never remove.
        let mut rng = rand::rngs::StdRng::seed_from_u64(4);
        let pos: Vec<Vec2> = (0..5000)
            .map(|_| Vec2::new(rng.gen_range(-3.0..7.0), rng.gen_range(2.0..4.0)))
            .collect();
        let n = normalizer(&pos);
        let moved: Vec<Vec2> = pos.iter().map(|p| n.apply(*p)).collect();
        let again = normalizer(&moved);
        assert!(again.center.length() < 1e-3, "already centred: {:?}", again.center);
        assert!((again.scale - 1.0).abs() < 1e-2, "already scaled: {}", again.scale);
    }

    /// The emitted scene must *show* the attractor that was fitted.
    ///
    /// The framing is the half of the answer that is easiest to get silently
    /// wrong: the file is a valid scene and a perfectly good fractal either
    /// way, just pointed somewhere that hides the likeness. So this measures
    /// the scene the way a reader meets it — walk its own transforms, then
    /// check its own camera is aimed at what came out.
    ///
    /// Run across the awkward cases on purpose. Nonlinear maps and symmetry
    /// groups are exactly what broke the previous approach, which baked the
    /// framing into the affine part and so computed `V ∘ S A S⁻¹` instead of
    /// `S ∘ V ∘ A ∘ S⁻¹`.
    #[test]
    fn the_emitted_scene_frames_the_attractor_that_was_fitted() {
        use crate::symmetry::OrbitKind;

        let cases = [
            (None, 0.0, "plain affine"),
            (None, 0.6, "nonlinear"),
            (Some(OrbitKind::Cyclic(6)), 0.6, "cyclic + nonlinear"),
            (Some(OrbitKind::Dihedral(3)), 0.4, "dihedral + nonlinear"),
        ];

        for (group, nonlinear, what) in cases {
            let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(21);
            let mut params = solve::Params::random(
                4,
                0.0,
                group,
                solve::AxisMode::InPlane,
                nonlinear,
                &mut rng,
            );
            // Off-centre, so the framing has real work to do.
            for m in &mut params.maps {
                m.translation += Vec3::new(1.5, -2.0, 0.5);
            }

            let fitted = solve::walk(&params, 30_000, &mut rng).unwrap();
            let scene = to_scene(&params, "t", &fitted);

            // Walk the scene's own transforms — the thing a reader opens —
            // rather than trusting that they are the ones we handed over.
            let enabled = vec![true; scene.transforms.len()];
            let mut srng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(22);
            let mut walker =
                crate::trace::Walker::new(&scene.transforms, &enabled, &mut srng).unwrap();
            for _ in 0..crate::trace::BURN_IN_STEPS {
                walker.step(&mut srng);
            }
            let shown: Vec<Vec2> = (0..30_000)
                .map(|_| {
                    walker.step(&mut srng);
                    Vec2::new(walker.pos.x, walker.pos.y)
                })
                .collect();

            let actual = normalizer(&shown);
            let aimed = Vec2::new(scene.camera_focus.x, scene.camera_focus.y);
            let half = BOX / actual.scale;

            // The camera looks where the material is, to within a tenth of the
            // attractor's own size.
            assert!(
                (aimed - actual.center).length() < half * 0.1,
                "{what}: camera aims at {aimed:?} but the scene walks to {:?}",
                actual.center,
            );
            // And from far enough to see all of it, without it being a speck.
            let visible = scene.camera_distance * (crate::camera::FOV_Y_RADIANS * 0.5).tan();
            assert!(
                visible > half && visible < half * 1.6,
                "{what}: half-extent {half:.3} framed at half-height {visible:.3}",
            );
        }
    }

}
