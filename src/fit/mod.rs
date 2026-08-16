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

/// Bake the fit's normalisation into the maps and hand back a scene.
///
/// **The framing is part of the answer, not scratch work.** The fit centres
/// and scales the attractor on every evaluation in order to compare it with
/// the target; if that transform is thrown away, opening the result shows a
/// correct fractal from an angle and distance that hide the likeness
/// entirely, which looks exactly like a failed fit.
///
/// It goes into the maps rather than only into the camera. Conjugating by the
/// similarity `S(x) = k(x − c)` sends `f(x) = Mx + t` to
/// `f'(y) = My + k(Mc + t − c)`: the linear part is untouched, so the maps
/// stay `R·diag(s)` and stay writable, and the attractor simply *is* where
/// the fit judged it to be. The camera is then a plain framing of the box.
///
/// **Only what commutes with the symmetry group may be baked in.** The walk
/// applies `g ∘ f` with `g` drawn from the group, so conjugating `f` alone
/// gives `g ∘ S f S⁻¹`, which is not `S (g ∘ f) S⁻¹` unless `S` commutes with
/// every `g`. Uniform scaling about the origin always does. Translation does
/// not — except along a cyclic group's own axis, which its rotations leave
/// fixed. So the centring is projected onto what the group allows, and
/// whatever is left over goes to the camera's focus instead, where it costs
/// nothing. Getting this wrong would emit a scene that silently differs from
/// the one that was fitted.
fn to_scene(params: &solve::Params, name: &str, walk: &solve::Walk) -> Scene {
    use crate::symmetry::OrbitKind;

    let norm = normalizer(&walk.pos);
    let center = Vec3::new(
        norm.center.x,
        norm.center.y,
        median(walk.world.iter().map(|p| p.z)),
    );
    let k = norm.scale;

    let bakeable = match params.symmetry {
        None => center,
        // A rotation about the axis fixes the axis, so sliding along it
        // commutes with the whole group.
        Some(OrbitKind::Cyclic(_)) => {
            let axis = params.axis_of();
            axis * center.dot(axis)
        }
        // Dihedral half-turns and the polyhedral groups fix only the origin.
        Some(_) => Vec3::ZERO,
    };

    let mut transforms = params.to_transforms();
    for t in &mut transforms {
        let (s, r, tr) = t.matrix.to_scale_rotation_translation();
        let linear = glam::Mat3::from_quat(r) * glam::Mat3::from_diagonal(s);
        let moved = k * (linear * bakeable + tr - bakeable);
        t.matrix = glam::Mat4::from_scale_rotation_translation(s, r, moved);
    }
    // Where the attractor ends up once the bakeable part has been applied.
    let focus = k * (center - bakeable);

    let mut scene = Scene::blank();
    scene.name = name.to_string();
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

    // Looking down −z, which is the direction the loss projected along. 45°
    // vertical FOV puts the ±0.75 box comfortably in frame at this distance.
    scene.camera_focus = focus;
    scene.camera_distance = 2.0;
    scene.camera_orientation = crate::rot::Orientation::IDENTITY;
    scene
}

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
            "seed {:>3}: loss {:.5} (best random start {:.5}, {:.0}× better)  -> {}",
            opts.solve.seed.wrapping_add(i as u64),
            outcome.loss,
            outcome.initial_loss,
            outcome.initial_loss / outcome.loss.max(1e-9),
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

    /// The emitted scene must render the attractor that was fitted.
    ///
    /// Baking the framing into the maps is a conjugation, and a conjugation
    /// that does not commute with the symmetry group changes the shape rather
    /// than moving it. That failure is invisible — the file is a valid scene
    /// and a perfectly good fractal, just not the one that was scored — so it
    /// gets a test that compares the two clouds directly rather than checking
    /// that the numbers look plausible.
    #[test]
    fn the_emitted_scene_walks_the_attractor_that_was_fitted() {
        use crate::symmetry::OrbitKind;

        for group in [None, Some(OrbitKind::Cyclic(6)), Some(OrbitKind::Dihedral(3))] {
            let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(21);
            let mut params = solve::Params::random(4, 0.0, group, solve::AxisMode::InPlane, &mut rng);
            // Off-centre, so the conjugation has real work to do.
            for m in &mut params.maps {
                m.translation += Vec3::new(1.5, -2.0, 0.5);
            }

            let fitted = solve::walk(&params, 30_000, &mut rng).unwrap();
            let scene = to_scene(&params, "t", &fitted);

            // Walk the scene's own transforms — the thing a reader opens.
            let enabled = vec![true; scene.transforms.len()];
            let mut srng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(22);
            let mut walker =
                crate::trace::Walker::new(&scene.transforms, &enabled, &mut srng).unwrap();
            for _ in 0..crate::trace::BURN_IN_STEPS {
                walker.step(&mut srng);
            }
            let emitted: Vec<Vec2> = (0..30_000)
                .map(|_| {
                    walker.step(&mut srng);
                    Vec2::new(walker.pos.x, walker.pos.y)
                })
                .collect();

            // Both normalised: the comparison is of *shape*, since where the
            // scene sits is the camera's business and is checked separately.
            let a = normalizer(&fitted.pos);
            let b = normalizer(&emitted);
            let shape = |pts: &[Vec2], n: Norm, q: f32| {
                Vec2::new(
                    percentile(pts.iter().map(|p| n.apply(*p).x), q),
                    percentile(pts.iter().map(|p| n.apply(*p).y), q),
                )
            };
            for q in [0.05, 0.25, 0.5, 0.75, 0.95] {
                let want = shape(&fitted.pos, a, q);
                let got = shape(&emitted, b, q);
                assert!(
                    (want - got).length() < 0.06,
                    "{:?}: quantile {q} moved {:?} -> {:?}; the emitted scene is not \
                     the attractor that was fitted",
                    group.map(|g| g.label()),
                    want,
                    got,
                );
            }
        }
    }

    #[test]
    fn conjugation_moves_the_attractor_into_the_box() {
        // The emitted scene must show the likeness from its own default
        // camera. That means the normalisation the fit judged through has to
        // survive into the file, and the way it survives is by being baked
        // into the maps.
        let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(9);
        let mut params = solve::Params::random(5, 1.0, None, solve::AxisMode::InPlane, &mut rng);
        // Shove the whole IFS far off the origin and shrink it, so a
        // conjugation that quietly did nothing could not pass.
        for m in &mut params.maps {
            m.translation = m.translation * 0.2 + Vec3::new(7.0, -4.0, 2.0);
        }
        let walk = solve::walk(&params, 20_000, &mut rng).unwrap();
        assert!(
            walk.pos[0].length() > 2.0,
            "the un-normalised attractor must actually be off-frame for this to test anything"
        );
        let scene = to_scene(&params, "t", &walk);

        let stats = crate::trace::measure(&scene.transforms, &vec![true; scene.transforms.len()])
            .expect("a fitted scene must walk");
        assert!(
            stats.center.length() < 0.35,
            "attractor should sit at the origin, found {:?}",
            stats.center
        );
        assert!(
            (0.3..2.0).contains(&stats.radius),
            "attractor should fill the framed box, r95 = {}",
            stats.radius
        );
    }
}
