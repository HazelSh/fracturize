//! Rolls: a flame drawn part by part — every part the profile has on, the
//! rest kept from a base scene.
//!
//! Randomising an IFS is easy; randomising one that *renders* is not. This
//! module only draws candidates — the maps, their variations, a symmetry, the
//! colours and the background. Whether a candidate is kept is the gate's call
//! (`super::gate`), inside the search loop both fresh rolls and mutations
//! share (`super::generate`). The draws are also what a mutation *re-rolls* a
//! part with, so they're `pub(super)` one by one rather than only as a whole.
//!
//! Colour is rolled as well as form — the colour source (all three modes),
//! the gradient a palette-mode roll needs, and the background it all sits on.
//! Every draw comes off the caller's rng, so `--random --seed N` reproduces a
//! roll exactly, colours included, given the same settings. **Keep the order
//! of draws** when changing anything here: a reordering silently changes what
//! every recorded seed rolls.

use glam::{Mat4, Quat, Vec3};
use rand::Rng;

use super::gate::rescale;
use super::settings::{RandomSettings, SymmetrySettings, XaosSettings};
use crate::path::CameraPath;
use crate::scene::{generate_colormap, resolve_color_speeds, Scene, TransformSpec, NUM_VARIATIONS};

/// A candidate before quality checking: just the parts the chaos game needs.
pub(super) struct Candidate {
    pub transforms: Vec<TransformSpec>,
    pub names: Vec<Option<String>>,
    pub colors: Vec<Vec3>,
}

/// The base scene's own transforms — its first layer — as a roll keeps them:
/// what each part that's off is taken from.
struct Base<'a> {
    transforms: Vec<&'a TransformSpec>,
    names: Vec<Option<String>>,
    colors: Vec<Vec3>,
}

impl<'a> Base<'a> {
    fn of(scene: &'a Scene) -> Self {
        let own: Vec<usize> = (0..scene.transforms.len()).filter(|&i| scene.transforms[i].layer == 0).collect();
        Self {
            transforms: own.iter().map(|&i| &scene.transforms[i]).collect(),
            names: own.iter().map(|&i| scene.transform_names.get(i).cloned().flatten()).collect(),
            colors: own.iter().map(|&i| scene.colors.get(i).copied().unwrap_or(Vec3::ONE)).collect(),
        }
    }

    fn len(&self) -> usize {
        self.transforms.len()
    }
}

/// Draw every part of a flame that the profile has on, and take the rest from
/// `base` — the one way a roll is made, whichever parts are on.
///
/// Every draw is made in the same order whatever is on, and only then is a
/// part that's off replaced by the base's; so with every part on (the
/// default), `base` is never read and a seed rolls what it always rolled. The
/// one exception is the transform count, which has to be known before the
/// transforms are drawn: off, it is the base's, and nothing is drawn for it.
pub(super) fn build_candidate(settings: &RandomSettings, base: &Scene, rng: &mut impl Rng) -> Candidate {
    let p = &settings.parts;
    let base = Base::of(base);
    let n = if p.count.on() || base.len() == 0 {
        rng.gen_range(settings.transforms[0]..=settings.transforms[1]).max(1) as usize
    } else {
        base.len()
    };
    let kept = n.min(base.len());

    let mut transforms = Vec::with_capacity(n);
    for _ in 0..n {
        transforms.push(random_transform(settings, rng));
    }
    for (t, b) in transforms.iter_mut().zip(&base.transforms) {
        let (p_on, r_on, s_on) = (p.translate.on(), p.rotate.on(), p.scale.on());
        if !(p_on || r_on || s_on) {
            // The whole affine kept, shear and post-affine included.
            t.matrix = b.matrix;
            t.post_affine = b.post_affine;
        } else if !(p_on && r_on && s_on) {
            let (s, r, pos) = t.matrix.to_scale_rotation_translation();
            let (bs, br, bp) = b.matrix.to_scale_rotation_translation();
            t.matrix = Mat4::from_scale_rotation_translation(
                if s_on { s } else { bs },
                if r_on { r } else { br },
                if p_on { pos } else { bp },
            );
        }
        if !p.weight.on() {
            t.weight = b.weight;
        }
    }

    // An IFS only converges if it contracts on average. Rescale everything if
    // the mean linear contraction is too close to 1 — a size change, so only
    // when sizes are the dice's to change.
    let mean: f32 = transforms
        .iter()
        .map(|t| mean_singular_value(t.matrix))
        .sum::<f32>()
        / n as f32;
    if mean > 0.85 && p.scale.on() {
        rescale(&mut transforms, 0.75 / mean);
    }

    if p.symmetry.on() {
        apply_symmetry(&mut transforms, &settings.symmetry, p.scale.on(), p.translate.on(), rng);
    } else {
        for (t, b) in transforms.iter_mut().zip(&base.transforms) {
            t.symmetry = b.symmetry.clone();
        }
    }

    // Which map may follow which. Kept, it is the base's rows, less any edge
    // to a map this roll doesn't have.
    if p.xaos.on() {
        if let Some(rows) = random_xaos(&settings.xaos, n, rng) {
            for (t, row) in transforms.iter_mut().zip(rows.rows) {
                t.xaos = row;
            }
        }
    } else {
        for (t, b) in transforms.iter_mut().zip(&base.transforms) {
            t.xaos = b.xaos.clone();
            t.xaos.exceptions.retain(|e| (e.0 as usize) < n);
        }
    }

    // Variations. Purely affine IFS are a real and worthwhile family, but a
    // *random* one is almost always a dull Sierpinski variant — the good ones
    // come from deliberate symmetry, not dice. So by default every roll
    // carries some nonlinearity, and only rarely stays affine.
    //
    // The flame draws a few characteristic kinds, and each map takes its
    // variations from those. Picking kinds per flame (rather than freely per
    // map) is what makes the result read as a coherent form instead of a pile
    // of unrelated distortions; varying which maps carry them, and at what
    // strength, is what keeps it from looking uniform.
    let pool = settings.enabled_slots();
    if !p.variations.on() {
        for (t, b) in transforms.iter_mut().zip(&base.transforms) {
            t.variations = b.variations;
        }
    } else if !pool.is_empty() && rng.r#gen::<f32>() >= settings.affine {
        let k = (rng.gen_range(settings.kinds[0]..=settings.kinds[1]) as usize).clamp(1, pool.len());
        let kinds = choose(rng, &pool, k);

        let mut any = false;
        for t in transforms.iter_mut() {
            let m = (rng.gen_range(settings.per_map[0]..=settings.per_map[1]) as usize).min(kinds.len());
            for slot in choose(rng, &kinds, m) {
                t.variations[slot] = roll_weight(settings, slot, rng);
                any = true;
            }
        }
        // A flame that rolled kinds but put none of them anywhere is just
        // affine by accident; one map always carries one.
        if !any {
            let idx = rng.gen_range(0..n);
            let slot = kinds[rng.gen_range(0..kinds.len())];
            transforms[idx].variations[slot] = roll_weight(settings, slot, rng);
        }
        // Keep the total blend near 1 so each map's overall scale stays in
        // the range the contraction check just established.
        for t in transforms.iter_mut() {
            let nonlinear: f32 = t.variations[1..].iter().map(|w| w.abs()).sum();
            t.variations[0] = (1.0 - nonlinear).max(0.0);
        }
    }

    // Palette: one base hue plus a scheme, so the colors relate rather than
    // clash. color_value spreads the transforms around the gradient.
    let base_hue: f32 = rng.gen_range(0.0..360.0);
    let scheme = rng.gen_range(0..3);
    let mut colors: Vec<Vec3> = (0..n)
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
    if !p.color.on() && base.len() > 0 {
        // The base's colours, and a new transform takes one of them, so the
        // set stays the base's.
        for (i, c) in colors.iter_mut().enumerate() {
            *c = base.colors[i % base.len()];
        }
    }

    for (i, t) in transforms.iter_mut().enumerate() {
        t.color_value = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.5 };
    }
    if !p.color_value.on() {
        for (t, b) in transforms.iter_mut().zip(&base.transforms) {
            t.color_value = b.color_value;
        }
    }

    let mut names: Vec<Option<String>> = (0..n).map(|i| Some(format!("t{}", i))).collect();
    if !p.count.on() {
        names[..kept].clone_from_slice(&base.names[..kept]);
    }
    Candidate { transforms, names, colors }
}

/// Roll a symmetry onto some of the maps, if the settings ask for one.
///
/// The motifs shrink as they join, by `(Σ eᵢ^D)^(-1/D)` over the elements'
/// sizes `eᵢ` (all 1 for a group, a taper for a repeat) with `D = 2.2`: what
/// keeps the effective map set's dimension near where the maps alone had it,
/// instead of sixty full-size copies filling space into a lump the shape gate
/// throws away. Reliquary's hand-sized icosahedral facet is about this size.
/// A dimension bound, if on, then steers from there as usual. The shrink and
/// the push off the centre are size and position changes, so each is made
/// only when that part is the dice's to change.
fn apply_symmetry(
    transforms: &mut [TransformSpec],
    s: &SymmetrySettings,
    resize: bool,
    move_off_centre: bool,
    rng: &mut impl Rng,
) {
    if !s.any_kind() || rng.r#gen::<f32>() >= s.chance {
        return;
    }
    let Some(sym) = random_symmetry(s, rng) else { return };

    let n = transforms.len();
    let free = (rng_range_u32(rng, s.free) as usize).min(n - 1);
    let order = choose(rng, &(0..n).collect::<Vec<_>>(), n - free);
    let k = if resize { motif_shrink(&sym) } else { 1.0 };
    for i in order {
        let t = &mut transforms[i];
        let (scale, rot, mut p) = t.matrix.to_scale_rotation_translation();
        // A motif on the centre is copied onto itself; push it out to where
        // its copies land apart.
        if p.length() < 0.4 {
            p = if p.length() > 1e-3 { p.normalize() } else { random_quat(rng) * Vec3::X } * 0.4;
        }
        if resize || move_off_centre {
            let (scale, keep_p) = (scale * k, t.matrix.w_axis.truncate());
            t.matrix = Mat4::from_scale_rotation_translation(scale, rot, if move_off_centre { p } else { keep_p });
        }
        t.symmetry = Some(sym.clone());
    }
}

/// A layer's transition graph, drawn: one row per map, and what it is in a
/// few words, for a mutation's log.
pub(super) struct Graph {
    pub rows: Vec<crate::xaos::Xaos>,
    pub label: String,
}

/// A transition graph for `n` maps, drawn the way the xaos settings say:
/// `None` for no xaos. Draws nothing at all when the settings can't give one
/// (`XaosSettings::draws`), which is what keeps `default`'s seeds.
pub(super) fn random_xaos(s: &XaosSettings, n: usize, rng: &mut impl Rng) -> Option<Graph> {
    use crate::xaos::Preset;
    if !s.draws() {
        return None;
    }
    let total = s.off + s.preset + s.random;
    let r = rng.r#gen::<f32>() * total;
    if r < s.off || n < 2 {
        return None;
    }
    if r < s.off + s.preset {
        // Only the presets that leave something to draw at this many maps.
        let mut presets = Vec::new();
        if s.no_self_loops && n >= 3 {
            presets.push(Preset::NoSelfLoops);
        }
        if s.blocks && n >= 4 {
            presets.push(Preset::Blocks(rng.gen_range(2..=n / 2)));
        }
        if presets.is_empty() {
            return None;
        }
        let preset = presets[rng.gen_range(0..presets.len())];
        return Some(Graph { rows: preset.rows(n), label: preset.name() });
    }

    // A random graph: a cycle through every map in random order, kept, and
    // a share of the other edges cut.
    let order = choose(rng, &(0..n).collect::<Vec<_>>(), n);
    let mut kept = vec![vec![false; n]; n];
    for k in 0..n {
        kept[order[k]][order[(k + 1) % n]] = true;
    }
    let [lo, hi] = s.cut;
    let cut = if hi > lo { rng.gen_range(lo..=hi) } else { lo };
    let no_self = n >= 3 && rng.r#gen::<f32>() < s.self_loops;
    let mut grid = vec![vec![1.0f32; n]; n];
    let mut cuts = 0;
    for i in 0..n {
        for j in 0..n {
            if !kept[i][j] && ((no_self && i == j) || rng.r#gen::<f32>() < cut) {
                grid[i][j] = 0.0;
                cuts += 1;
            }
        }
    }
    // One exit everywhere makes the walk deterministic: a handful of dots.
    // So at least one map keeps a choice.
    if grid.iter().all(|row| row.iter().filter(|&&x| x > 0.0).count() < 2) {
        let open: Vec<(usize, usize)> = (0..n)
            .flat_map(|i| (0..n).map(move |j| (i, j)))
            .filter(|&(i, j)| grid[i][j] == 0.0 && !(no_self && i == j))
            .collect();
        if let Some(&(i, j)) = open.get(rng.gen_range(0..open.len().max(1))) {
            grid[i][j] = 1.0;
            cuts -= 1;
        }
    }
    let [mlo, mhi] = s.multiplier;
    let mut weighted = 0;
    for row in grid.iter_mut() {
        for x in row.iter_mut().filter(|x| **x > 0.0) {
            if rng.r#gen::<f32>() < s.weighted {
                *x = if mhi > mlo { rng.gen_range(mlo.ln()..=mhi.ln()).exp() } else { mlo };
                weighted += 1;
            }
        }
    }
    let mut label = format!("random, {cuts} of {} edges cut", n * n);
    if weighted > 0 {
        label += &format!(", {weighted} weighted");
    }
    Some(Graph { rows: grid.iter().map(|row| row_from_dense(row)).collect(), label })
}

/// A row from every target's multiplier: the rest is whichever of 0 and 1 is
/// commoner, so a row lists as few exceptions as it can.
pub(super) fn row_from_dense(row: &[f32]) -> crate::xaos::Xaos {
    let zeros = row.iter().filter(|&&x| x == 0.0).count();
    let mut x = crate::xaos::Xaos::default();
    x.set_rest(if zeros * 2 > row.len() { 0.0 } else { 1.0 });
    for (j, &v) in row.iter().enumerate() {
        x.set(j, v);
    }
    x
}

/// How much a motif shrinks on joining `sym` (see [`apply_symmetry`]).
pub(super) fn motif_shrink(sym: &crate::symmetry::Symmetry) -> f32 {
    const D: f32 = 2.2;
    sym.element_scales().iter().map(|e| e.powf(D)).sum::<f32>().powf(-1.0 / D)
}

/// A group drawn from the enabled kinds, about a random axis. `None` when no
/// kind is enabled, or the one drawn can't be built.
pub(super) fn random_symmetry(s: &SymmetrySettings, rng: &mut impl Rng) -> Option<crate::symmetry::Symmetry> {
    use crate::symmetry::{OrbitColor, OrbitKind, Repeat, Symmetry};
    if !s.any_kind() {
        return None;
    }
    let mut kinds = Vec::new();
    let fold = |rng: &mut _| rng_range_u32(rng, s.fold);
    if s.cyclic {
        kinds.push(OrbitKind::Cyclic(fold(rng)));
    }
    if s.dihedral {
        kinds.push(OrbitKind::Dihedral(fold(rng)));
    }
    if s.tetra {
        kinds.push(OrbitKind::Tetrahedral);
    }
    if s.octa {
        kinds.push(OrbitKind::Octahedral);
    }
    if s.icosa {
        kinds.push(OrbitKind::Icosahedral);
    }
    let axis = random_quat(rng) * Vec3::Y;
    if s.repeat {
        // A helix up the axis that tapers: the turn anywhere, the climb a
        // fraction of a unit per copy, so the whole run stays near a frame.
        let count = rng_range_u32(rng, s.repeat_count);
        let climb = rng.gen_range(0.6f32..1.6) / count as f32;
        kinds.push(OrbitKind::Repeat(Repeat {
            count,
            translate: axis * climb,
            turn: rng.gen_range(20.0..160.0),
            scale: rng.gen_range(0.85..0.98),
        }));
    }
    let kind = kinds[rng.gen_range(0..kinds.len())];
    let mirror = kind.is_group() && rng.r#gen::<f32>() < s.mirror;
    let color = if rng.r#gen::<f32>() < s.orbit_color { OrbitColor::Orbit } else { OrbitColor::Shared };
    Symmetry::new(kind, axis, mirror, color).ok()
}

pub(super) fn rng_range_u32(rng: &mut impl Rng, r: [u32; 2]) -> u32 {
    rng.gen_range(r[0]..=r[1].max(r[0]))
}

/// `k` distinct items of `from`, in random order.
pub(super) fn choose(rng: &mut impl Rng, from: &[usize], k: usize) -> Vec<usize> {
    let mut v = from.to_vec();
    for i in (1..v.len()).rev() {
        v.swap(i, rng.gen_range(0..=i));
    }
    v.truncate(k);
    v
}

/// A weight for `slot`, from its range, negative at the settings' odds.
pub(super) fn roll_weight(settings: &RandomSettings, slot: usize, rng: &mut impl Rng) -> f32 {
    let [lo, hi] = settings.variation(slot).weight;
    let w = if hi > lo { rng.gen_range(lo..=hi) } else { lo };
    if rng.r#gen::<f32>() < settings.negative { -w } else { w }
}

/// A plain map — affine only, linear weight 1 — at a random place, turn and
/// size. Variations are the caller's.
pub(super) fn random_transform(settings: &RandomSettings, rng: &mut impl Rng) -> TransformSpec {
    let translation = random_translation(rng);
    let rotation = random_quat(rng);
    let scale = random_scale(settings, rng);

    let mut variations = [0.0f32; NUM_VARIATIONS];
    variations[0] = 1.0;

    TransformSpec {
        layer: 0,
        matrix: Mat4::from_scale_rotation_translation(scale, rotation, translation),
        post_affine: Mat4::IDENTITY,
        color_value: 0.5,
        weight: rng.gen_range(0.5..2.0),
        color_speed: 0.5,
        explicit_color_speed: None,
        symmetry: None,
        variations,
        xaos: Default::default(),
    }
}

/// A map's position: uniform-ish inside a ball, so transforms don't all pile
/// up near the origin the way independent per-axis ranges would.
pub(super) fn random_translation(rng: &mut impl Rng) -> Vec3 {
    let dir = Vec3::new(
        rng.gen_range(-1.0..1.0),
        rng.gen_range(-1.0..1.0),
        rng.gen_range(-1.0..1.0),
    );
    let dir = if dir.length_squared() < 1e-6 { Vec3::X } else { dir.normalize() };
    dir * rng.gen_range(0.0f32..0.9).cbrt() * 0.9
}

/// A map's per-axis size from the settings' scale range.
pub(super) fn random_scale(settings: &RandomSettings, rng: &mut impl Rng) -> Vec3 {
    // Log-uniform so 0.35 and 0.75 are equally likely to be *interesting*,
    // rather than the top of the range dominating.
    let [lo, hi] = settings.scale;
    let base = if hi > lo { rng.gen_range(lo.ln()..=hi.ln()).exp() } else { lo };
    if rng.r#gen::<f32>() < settings.squash {
        // Squash one axis: this is what turns blobs into sheets and ribbons.
        let mut s = Vec3::splat(base);
        s[rng.gen_range(0..3)] = base * rng.gen_range(0.15..0.6);
        s
    } else {
        Vec3::splat(base)
    }
}

/// Uniformly-distributed random rotation (Shoemake's subgroup method).
pub(super) fn random_quat(rng: &mut impl Rng) -> Quat {
    let u1: f32 = rng.r#gen();
    let u2: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let u3: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let (s1, s2) = ((1.0 - u1).sqrt(), u1.sqrt());
    Quat::from_xyzw(s1 * u2.sin(), s1 * u2.cos(), s2 * u3.sin(), s2 * u3.cos())
}

/// Geometric mean of a matrix's axis lengths — a cheap stand-in for "how much
/// does this map shrink space", good enough for the contraction check.
fn mean_singular_value(m: Mat4) -> f32 {
    let (s, _, _) = m.to_scale_rotation_translation();
    (s.x.abs() * s.y.abs() * s.z.abs()).cbrt()
}

/// Wrap an accepted candidate up as a full `Scene`, framed for its size.
///
/// Like [`build_candidate`], every draw is made and then a part that's off is
/// taken from `base`: the colour source and gradient, the background. A roll
/// with no part of the form on has the base's form, so it keeps the base's
/// camera and look as well — it's the same flame, recoloured.
pub(super) fn finish(
    mut c: Candidate,
    center: Vec3,
    radius: f32,
    settings: &RandomSettings,
    base: &Scene,
    rng: &mut impl Rng,
) -> Scene {
    let p = &settings.parts;
    let color_speed = 0.5;
    let color_falloff = if rng.r#gen::<f32>() < 0.4 { rng.gen_range(0.6..1.2) } else { 0.0 };
    resolve_color_speeds(&mut c.transforms, color_speed, color_falloff);
    let colormap = generate_colormap(&c.colors);

    // Which colour source this flame renders through, and — for palette mode —
    // the gradient itself. Rolled before the scene is built so the background
    // can be tinted from whatever will actually be on screen.
    let (color_mode, palette) = random_color_mode(rng);

    let mut scene = Scene {
        subscenes: Vec::new(),
        root_share: 1.0,
        root_visible: true,
        tracks: Vec::new(),
        xaos_matrix_layers: Vec::new(),
            colours_left_out: Vec::new(),
        name: format!("random-{}", crate::app::unix_timestamp()),
        author: "fracturize random flame".to_string(),
        // Scale the dot size with the attractor: the same point size that
        // looks dusty on a radius-3 form is a solid mass on a radius-0.3 one.
        point_size: (radius * 0.0018).clamp(0.0008, 0.012),
        points_per_frame: 100_000,
        point_count: 500_000,
        point_count_defaulted: true,
        // A random flame states no lens, so whoever opens it renders it
        // through their own — same as a scene file with no `fov` key.
        camera_fov: crate::camera::Fov::default(),
        camera_fov_authored: false,
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

    if !p.color.on() {
        scene.color_falloff = base.color_falloff;
        scene.color_speed = base.color_speed;
        scene.color_contrast = base.color_contrast;
        scene.palette = base.palette.clone();
        scene.set_color_mode(base.color_mode);
        let falloff = scene.color_falloff;
        scene.resolve_all_color_speeds(falloff);
    }
    if !p.background.on() {
        scene.background = base.background;
    }
    // A roll draws the scene's own maps and nothing else, so a composite's
    // subscenes come through it whole, maps, placement, colour and all —
    // and with them the author's framing of the composition, which a camera
    // fitted to the scene's own maps alone would crop.
    let composite = !base.subscenes.is_empty();
    if composite {
        scene.subscenes = base.subscenes.clone();
        scene.root_share = base.root_share;
        scene.root_visible = base.root_visible;
        scene.xaos_matrix_layers.extend(base.xaos_matrix_layers.iter().filter(|&&l| l != 0));
        for (i, t) in base.transforms.iter().enumerate().filter(|(_, t)| t.layer != 0) {
            scene.transforms.push(t.clone());
            scene.transform_names.push(base.transform_names.get(i).cloned().flatten());
            scene.colors.push(base.colors.get(i).copied().unwrap_or(Vec3::ONE));
        }
    }
    if p.no_form() || composite {
        scene.camera_focus = base.camera_focus;
        scene.camera_distance = base.camera_distance;
        scene.camera_orientation = base.camera_orientation;
    }
    if p.no_form() {
        // The same flame, recoloured: it keeps its name, its author, and
        // which colours it left to their positions (re-checked against the
        // values, so a colour the roll did change is written down).
        scene.name = base.name.clone();
        scene.author = base.author.clone();
        scene.colours_left_out = base.colours_left_out.clone();
        scene.decay = base.decay;
        scene.points_per_frame = base.points_per_frame;
        scene.camera_focus = base.camera_focus;
        scene.camera_distance = base.camera_distance;
        scene.camera_orientation = base.camera_orientation;
        scene.camera_fov = base.camera_fov;
        scene.camera_fov_authored = base.camera_fov_authored;
        scene.point_size = base.point_size;
        scene.point_count = base.point_count;
        scene.point_count_defaulted = base.point_count_defaulted;
        scene.haze = base.haze;
        scene.exposure = base.exposure;
    }
    // Infinite zoom stays wherever its map still qualifies on what was drawn:
    // always, when the form is kept; on a new form, if the same map is still
    // plain, contracting, and allowed after every map that fires.
    scene.zoom = base.zoom.clone().filter(|z| {
        crate::renorm::Renorm::build(z, &scene.transforms, scene.camera_distance).is_ok()
            && crate::xaos::zoom_conflict(&scene.transforms, &scene.transform_names, z.map).is_none()
    });
    // A roll changes values; it doesn't throw the animation away. Tracks
    // follow their maps onto the roll and move with the values drawn, and the
    // camera's flight moves with the framing (unchanged when the form is
    // kept, since the framing is then the base's).
    scene.tracks = crate::track::carry_onto_roll(base, &scene);
    scene.camera_path = base.camera_path.clone().map(|mut path| {
        let (shift, ratio) = (scene.camera_focus - base.camera_focus, scene.camera_distance / base.camera_distance);
        for key in &mut path.keys {
            key.focus += shift;
            key.distance *= ratio;
        }
        // A zoom loop closes under the zoom map's symmetry; without a zoom the
        // file wouldn't load, so it loops back to its first key instead.
        if scene.zoom.is_none() && matches!(path.loops, crate::path::Loop::Zoom(_)) {
            path.loops = crate::path::Loop::Closed;
        }
        path
    });
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
pub(super) fn random_color_mode(rng: &mut impl Rng) -> (crate::scene::ColorMode, Option<crate::palette::Palette>) {
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
pub(super) fn tint(scene: &Scene, rng: &mut impl Rng) -> Vec3 {
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
///
/// Linear, so read it through the sRGB curve: 0.06 shows as 69/255 and 0.2 as
/// 124/255. It used to reach 0.35 (160/255, a 63% grey), which was judged too
/// bright at the top — a judgement that could only be made once thumbnails
/// stopped drawing every background darker than it opens (2a75672).
const LIGHT_BACKGROUND_LEVEL: std::ops::Range<f32> = 0.06..0.2;

/// Roll a background that belongs behind `tint`.
///
/// Hue comes from the flame rather than from nowhere, so the two agree; the
/// level and the chroma are what make it a *ground* rather than another
/// element — dark and saturated, or mid and nearly neutral.
pub(super) fn random_background(rng: &mut impl Rng, tint: Vec3) -> Vec3 {
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
pub(super) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Vec3 {
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
    use crate::dice::gate::measure as measure_candidate;
    use crate::dice::settings::{Bound, SymmetrySettings, VariationRange};
    use crate::dice::{random_flame, roll};
    use crate::scene::{similarity_dimension, VARIATION_NAMES};
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

    /// Only enabled variations are rolled, and only at their weights.
    #[test]
    fn rolls_keep_to_the_variation_pool_and_ranges() {
        let mut settings = RandomSettings::default();
        for v in settings.variations.values_mut() {
            v.enabled = false;
        }
        settings.variations.insert("julia".into(), VariationRange { enabled: true, weight: [0.3, 0.4] });
        settings.variations.insert("absfold".into(), VariationRange { enabled: true, weight: [0.2, 0.25] });
        settings.affine = 0.0;
        settings.negative = 0.0;
        settings.transforms = [3, 4];
        settings.per_map = [1, 2];
        let settings = settings.normalized();
        let julia = VARIATION_NAMES.iter().position(|&v| v == "julia").unwrap();
        let absfold = VARIATION_NAMES.iter().position(|&v| v == "absfold").unwrap();
        let mut rolled = 0;
        for seed in 0..10u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let Ok(r) = roll(&settings, &mut rng) else { continue };
            rolled += 1;
            assert!((3..=4).contains(&r.scene.transforms.len()), "seed {seed}: count");
            for t in &r.scene.transforms {
                for (slot, &w) in t.variations.iter().enumerate().skip(1) {
                    match slot {
                        s if s == julia => assert!(w == 0.0 || (0.3..=0.4).contains(&w)),
                        s if s == absfold => assert!(w == 0.0 || (0.2..=0.25).contains(&w)),
                        _ => assert_eq!(w, 0.0, "seed {seed}: rolled {}", VARIATION_NAMES[slot]),
                    }
                }
                // Each map takes at least one variation.
                assert!(t.variations[1..].iter().any(|&w| w != 0.0), "seed {seed}: bare map");
            }
        }
        assert!(rolled >= 5, "only {rolled} of 10 rolls passed");
    }

    /// Dimension is steered rather than rejected, so even a narrow bound is
    /// met nearly every time.
    #[test]
    fn dimension_bounds_are_met() {
        for range in [[1.2, 1.4], [2.2, 2.4], [2.8, 3.0]] {
            let settings = RandomSettings {
                dimension: Bound { on: true, range },
                ..Default::default()
            };
            let mut met = 0;
            for seed in 0..12u64 {
                let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
                if let Ok(r) = roll(&settings, &mut rng) {
                    let d = similarity_dimension(&r.scene.transforms).unwrap();
                    assert!(
                        d >= range[0] - 2e-3 && d <= range[1] + 2e-3,
                        "seed {seed}: d {d} outside {range:?}"
                    );
                    met += 1;
                }
            }
            assert!(met >= 10, "{range:?}: only {met} of 12 met the bound");
        }
    }

    /// Lacunarity is rejection-sampled: whatever comes back is inside, and a
    /// bound nothing can meet fails with the reason counted.
    #[test]
    fn lacunarity_bounds_are_met_or_reported() {
        let settings = RandomSettings {
            lacunarity: Bound { on: true, range: [3.0, 30.0] },
            ..Default::default()
        };
        for seed in 0..6u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            if let Ok(r) = roll(&settings, &mut rng) {
                let l = r.lacunarity.unwrap();
                assert!((3.0..=30.0).contains(&l), "seed {seed}: lacunarity {l}");
            }
        }
        let impossible = RandomSettings {
            lacunarity: Bound { on: true, range: [900.0, 1000.0] },
            attempts: 8,
            ..Default::default()
        };
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let Err(f) = roll(&impossible, &mut rng) else { panic!("met an impossible bound") };
        assert_eq!(f.attempts, 8);
        assert_eq!(f.rejected.total(), 8);
        assert!(f.rejected.lacunarity > 0, "{:?}", f.rejected);
    }

    /// Not a correctness test: how dimension and lacunarity fall across
    /// default rolls, which is what says whether a bound is improbable.
    /// `cargo test --release random_flame_study -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement; prints a distribution"]
    fn random_flame_study() {
        let settings = RandomSettings::default();
        let mut ds = Vec::new();
        let mut ls = Vec::new();
        let mut tries = 0;
        let t = std::time::Instant::now();
        for seed in 0..200u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            if let Ok(r) = roll(&settings, &mut rng) {
                tries += r.attempts;
                ds.extend(r.dimension);
                ls.extend(r.lacunarity);
            }
        }
        let pct = |v: &mut Vec<f32>, p: f32| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[((v.len() - 1) as f32 * p) as usize]
        };
        println!("{} rolls, {} candidates, {:.1} ms/roll", ls.len(), tries, t.elapsed().as_secs_f64() * 1e3 / 200.0);
        for p in [0.05, 0.25, 0.5, 0.75, 0.95] {
            println!("p{:02}: d {:.2}  lacunarity {:.2}", (p * 100.0) as u32, pct(&mut ds, p), pct(&mut ls, p));
        }
    }


    fn symmetric() -> RandomSettings {
        RandomSettings {
            symmetry: SymmetrySettings { chance: 1.0, repeat: true, ..Default::default() },
            ..Default::default()
        }
    }

    /// At chance 1 every roll that passes is symmetric, keeps a map outside
    /// the group, and every motif shares one group.
    #[test]
    fn symmetric_rolls_have_a_group_and_a_defect() {
        let settings = symmetric();
        let mut rolled = 0;
        for seed in 0..24u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let Ok(r) = roll(&settings, &mut rng) else { continue };
            rolled += 1;
            let ts = &r.scene.transforms;
            let motifs: Vec<_> = ts.iter().filter_map(|t| t.symmetry.as_ref()).collect();
            assert!(!motifs.is_empty(), "seed {seed}: no group");
            assert_eq!(ts.len() - motifs.len(), 1, "seed {seed}: one map stays outside");
            assert!(motifs.iter().all(|g| *g == motifs[0]), "seed {seed}: two groups");
            // And it survives a save: the group is regrouped into a block and
            // read back onto the same motifs.
            let text = r.scene.to_toml_string().unwrap();
            let path = std::env::temp_dir().join(format!("fracturize-symroll-{}-{seed}.toml", std::process::id()));
            std::fs::write(&path, &text).unwrap();
            let back = Scene::load(&path).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{text}"));
            std::fs::remove_file(&path).ok();
            assert_eq!(
                back.transforms.iter().filter(|t| t.symmetry.is_some()).count(),
                motifs.len(),
                "seed {seed}: symmetry lost in a save"
            );
        }
        assert!(rolled >= 16, "only {rolled} of 24 symmetric rolls passed");
    }

    /// Steering has to count the group's copies, or it lands somewhere else.
    #[test]
    fn dimension_steering_counts_the_group() {
        let mut settings = symmetric();
        settings.dimension = Bound { on: true, range: [2.0, 2.1] };
        let mut met = 0;
        for seed in 0..12u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            if let Ok(r) = roll(&settings, &mut rng) {
                let d = similarity_dimension(&r.scene.transforms).unwrap();
                assert!((1.998..=2.102).contains(&d), "seed {seed}: d {d}");
                met += 1;
            }
        }
        assert!(met >= 8, "only {met} of 12");
    }

    /// How often symmetric rolls pass, and what they cost.
    /// `cargo test --release symmetry_study -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement; prints a distribution"]
    fn symmetry_study() {
        let settings = symmetric();
        let (mut ok, mut tries, mut ds, mut ls) = (0, 0, Vec::new(), Vec::new());
        let t = std::time::Instant::now();
        for seed in 0..200u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            match roll(&settings, &mut rng) {
                Ok(r) => {
                    ok += 1;
                    tries += r.attempts;
                    ds.extend(r.dimension);
                    ls.extend(r.lacunarity);
                }
                Err(f) => println!("seed {seed}: {}", f.message()),
            }
        }
        ds.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ls.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{ok}/200 rolled, {:.2} tries each, {:.1} ms/roll; d median {:.2}, lacunarity median {:.1}",
            tries as f32 / ok as f32,
            t.elapsed().as_secs_f64() * 1e3 / 200.0,
            ds[ds.len() / 2],
            ls[ls.len() / 2],
        );
    }


    /// Every random xaos graph keeps its cycle, so every map gets a real
    /// share of the walk and no map is a dead end; every preset drawn fits
    /// the map count; and the default draws none, and no die for it.
    #[test]
    fn xaos_graphs_are_connected_and_fit() {
        use crate::dice::XaosSettings;
        let fun = XaosSettings { off: 0.0, preset: 1.0, random: 2.0, weighted: 0.3, ..Default::default() };
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let mut kinds = std::collections::BTreeSet::new();
        for k in 0..400 {
            let n = 2 + k % 7;
            let Some(g) = random_xaos(&fun, n, &mut rng) else { continue };
            kinds.insert(g.label.split([',', ':']).next().unwrap().to_string());
            assert_eq!(g.rows.len(), n);
            let mut maps: Vec<TransformSpec> = (0..n).map(|_| random_transform(&RandomSettings::default(), &mut rng)).collect();
            for (t, row) in maps.iter_mut().zip(&g.rows) {
                t.xaos = row.clone();
            }
            let chain = crate::xaos::Chain::build(&maps, &[]).unwrap();
            assert!(chain.dead.is_empty(), "{}: a dead end", g.label);
            assert!(
                (0..n).any(|i| (0..n).filter(|&j| chain.allowed(i, j)).count() >= 2),
                "{}: every map has one exit, so the walk is a fixed sequence",
                g.label
            );
            if g.label.starts_with("random") {
                assert!(chain.shares().iter().all(|&s| s > 1e-6), "{}: a map shut out", g.label);
            }
            if g.label == "no-self-loops" {
                assert!(n >= 3);
            }
            if let Some(b) = g.label.strip_prefix("blocks:") {
                assert!(b.parse::<usize>().unwrap() * 2 <= n, "{} over {n} maps leaves a block of one", g.label);
            }
        }
        assert_eq!(kinds.into_iter().collect::<Vec<_>>(), ["blocks", "no-self-loops", "random"]);

        // The default draws nothing: the rng is where it was.
        let mut a = rand::rngs::StdRng::seed_from_u64(5);
        let b = a.clone();
        assert!(random_xaos(&XaosSettings::default(), 5, &mut a).is_none());
        assert_eq!(a, b, "the default spent a die on xaos");
    }

    /// Off, a roll keeps the base's rows — less any edge to a map it doesn't
    /// have — and on with odds only of none, it clears them.
    #[test]
    fn a_roll_keeps_or_clears_the_bases_xaos() {
        let mut base = Scene::blank();
        let extra = base.transforms[0].clone();
        base.insert_transform(2, extra, None, None, None);
        base.transforms[0].xaos.set(1, 0.0);
        base.transforms[1].xaos.set(2, 0.0);
        let mut keep = RandomSettings::default();
        keep.parts.xaos.set_on(false);
        keep.transforms = [2, 2];
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let c = build_candidate(&keep, &base, &mut rng);
        assert_eq!(c.transforms[0].xaos.get(1), 0.0, "kept");
        assert!(c.transforms[1].xaos.is_default(), "the edge to a map the roll lacks is dropped");
        let c = build_candidate(&RandomSettings::default(), &base, &mut rng);
        assert!(c.transforms.iter().all(|t| t.xaos.is_default()));
    }
}
