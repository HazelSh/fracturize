//! Mutation operators: a few parts of a scene nudged or re-rolled.
//!
//! One call to [`mutate`] makes a handful of changes (the profile's
//! `mutation.changes`), each to a part the profile has on, picked by the
//! parts' odds — a map's place, turn, size, weight, variation blend or colour,
//! a map added or removed, the background, the symmetry group, an edge of the
//! xaos graph or the whole graph — and returns
//! human- and LLM-readable descriptions of what changed. A part that's off is
//! never touched, which is exactly what it means to a roll too.
//!
//! Each touched part is **nudged** (what is there perturbed, scaled by the
//! strength) or **re-rolled** (replaced using the very draw a fresh roll uses,
//! from `roll.rs`, under the same ranges). That is the point of keeping
//! mutation inside the dice: "give map 2 new variations" draws from the same
//! enabled list and weight ranges a fresh roll would, so a profile that never
//! rolls `spherical` never mutates it in either.
//!
//! This only *proposes*; whether a mutation is kept is the gate's call, in
//! the search loop `super::generate` runs.

use glam::{Mat4, Vec3};
use rand::Rng;

use super::roll;
use super::settings::RandomSettings;
use crate::scene::{Scene, NUM_VARIATIONS, VARIATION_NAMES};

/// A nudge that brings in a new variation weighs it at this share of a weight
/// the roll would draw, times the strength — a step, not a re-roll.
const NUDGE_SHARE: f32 = 0.45;

/// Apply the profile's operators to `scene` at `strength` (1.0 = a noticeable
/// but usually non-destructive step). Derived state (colour speeds, colormap)
/// is refreshed before returning.
pub fn mutate(scene: &mut Scene, settings: &RandomSettings, strength: f32, rng: &mut impl Rng) -> Vec<String> {
    let n_ops = roll::rng_range_u32(rng, settings.mutation.changes);
    let mut log = Vec::new();
    // Maps an earlier operator changed, so a later one doesn't delete them
    // and throw that operator away. New maps are appended, and count.
    let mut touched = vec![false; scene.transforms.len()];
    for _ in 0..n_ops {
        let before = (!scene.tracks.is_empty()).then(|| scene.clone());
        apply_one(scene, settings, strength, rng, &mut touched, &mut log);
        touched.resize(scene.transforms.len(), true);
        // An animated value the operator changed takes its animation with it,
        // or the keys would override the change at every moment. One that
        // added or removed a map has already moved the tracks' paths.
        if let Some(before) = before.filter(|b| b.transforms.len() == scene.transforms.len()) {
            let mut tracks = std::mem::take(&mut scene.tracks);
            crate::track::rebase(&mut tracks, &before, scene);
            scene.tracks = tracks;
        }
    }
    if log.is_empty() {
        log.push("nothing changed: no part the profile has on can change on this scene".to_string());
    }

    let falloff = scene.color_falloff;
    scene.resolve_all_color_speeds(falloff);
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

/// Indices into `Parts::NAMES`.
mod part {
    pub const TRANSLATE: usize = 0;
    pub const ROTATE: usize = 1;
    pub const SCALE: usize = 2;
    pub const WEIGHT: usize = 3;
    pub const VARIATIONS: usize = 4;
    pub const COUNT: usize = 5;
    pub const SYMMETRY: usize = 6;
    pub const COLOR: usize = 7;
    pub const COLOR_VALUE: usize = 8;
    pub const BACKGROUND: usize = 9;
    pub const XAOS: usize = 10;
}

/// Scene indices of the maps in map `i`'s layer, in layer order.
fn layer_of(scene: &Scene, i: usize) -> Vec<usize> {
    let layer = scene.transforms[i].layer;
    (0..scene.transforms.len()).filter(|&j| scene.transforms[j].layer == layer).collect()
}

/// Maps in map `i`'s layer.
fn layer_len(scene: &Scene, i: usize) -> usize {
    let layer = scene.transforms[i].layer;
    scene.transforms.iter().filter(|t| t.layer == layer).count()
}

/// Whether part `k` can be changed on map `i` of this scene at all.
///
/// Adding and removing follow the same rule the gate does for bounds: the
/// profile's transform range is its say on how many maps a flame has, so a
/// mutation may not take a layer past it — but a scene already past it isn't
/// stuck, it just can't go further.
fn can_add(scene: &Scene, i: usize, settings: &RandomSettings) -> bool {
    layer_len(scene, i) < settings.transforms[1] as usize
}

fn can_remove(scene: &Scene, i: usize, settings: &RandomSettings, touched: &[bool]) -> bool {
    !touched[i] && layer_len(scene, i) > (settings.transforms[0] as usize).max(1)
}

fn applicable(k: usize, scene: &Scene, i: usize, settings: &RandomSettings, touched: &[bool]) -> bool {
    match k {
        part::VARIATIONS => {
            !settings.enabled_slots().is_empty() || scene.transforms[i].variations.iter().any(|&w| w != 0.0)
        }
        part::COUNT => can_add(scene, i, settings) || can_remove(scene, i, settings, touched),
        part::SYMMETRY => {
            settings.symmetry.any_kind() || scene.transforms.iter().any(|t| t.symmetry.is_some())
        }
        // A graph needs two maps, and something to change: xaos the profile
        // may draw, or xaos already there.
        part::XAOS => {
            let idx = layer_of(scene, i);
            idx.len() >= 2 && (settings.xaos.draws() || idx.iter().any(|&j| !scene.transforms[j].xaos.is_default()))
        }
        _ => true,
    }
}

fn apply_one(
    scene: &mut Scene,
    settings: &RandomSettings,
    strength: f32,
    rng: &mut impl Rng,
    touched: &mut Vec<bool>,
    log: &mut Vec<String>,
) {
    let n = scene.transforms.len();
    if n == 0 {
        return;
    }
    let i = rng.gen_range(0..n);
    let parts = settings.parts.all();
    let odds: Vec<f32> = parts
        .iter()
        .enumerate()
        .map(|(k, p)| if applicable(k, scene, i, settings, touched) { p.odds } else { 0.0 })
        .collect();
    let total: f32 = odds.iter().sum();
    if total <= 0.0 {
        return;
    }
    let mut r = rng.r#gen::<f32>() * total;
    let k = odds.iter().position(|&o| {
        r -= o;
        r < 0.0 && o > 0.0
    });
    // Rounding can leave `r` a hair above zero after the last part.
    let k = k.unwrap_or_else(|| odds.iter().rposition(|&o| o > 0.0).unwrap_or(0));
    let reroll = rng.r#gen::<f32>() < parts[k].reroll;

    match k {
        part::TRANSLATE => translate(scene, i, reroll, strength, rng, log),
        part::ROTATE => rotate(scene, i, reroll, strength, rng, log),
        part::SCALE => scale(scene, i, reroll, settings, strength, rng, log),
        part::WEIGHT => weight(scene, i, reroll, strength, rng, log),
        part::VARIATIONS => variations(scene, i, reroll, settings, strength, rng, log),
        part::COLOR => color(scene, i, reroll, strength, rng, log),
        part::COLOR_VALUE => color_value(scene, i, reroll, strength, rng, log),
        part::COUNT => {
            // Add or remove, whichever the range allows; both, a coin.
            let (add_ok, remove_ok) = (can_add(scene, i, settings), can_remove(scene, i, settings, touched));
            if remove_ok && !(add_ok && rng.r#gen::<bool>()) {
                scene.remove_transform(i);
                touched.remove(i);
                log.push(format!("delete T{}", i));
                return;
            }
            add(scene, i, reroll, settings, strength, rng, log);
        }
        part::BACKGROUND => background(scene, reroll, strength, rng, log),
        part::SYMMETRY => symmetry(scene, i, reroll, settings, strength, rng, log),
        part::XAOS => xaos(scene, i, reroll, settings, strength, rng, log),
        _ => unreachable!("a part with no operator"),
    }
    if k != part::BACKGROUND {
        touched[i] = true;
    }
}

fn translate(scene: &mut Scene, i: usize, reroll: bool, strength: f32, rng: &mut impl Rng, log: &mut Vec<String>) {
    let m = &mut scene.transforms[i].matrix;
    if reroll {
        let p = roll::random_translation(rng);
        m.w_axis = p.extend(1.0);
        log.push(format!("T{} moved to ({:+.2},{:+.2},{:+.2})", i, p.x, p.y, p.z));
    } else {
        let d = random_unit(rng) * rng.gen_range(0.03..0.18) * strength;
        m.w_axis += d.extend(0.0);
        log.push(format!("T{} translate ({:+.2},{:+.2},{:+.2})", i, d.x, d.y, d.z));
    }
}

fn rotate(scene: &mut Scene, i: usize, reroll: bool, strength: f32, rng: &mut impl Rng, log: &mut Vec<String>) {
    let m = scene.transforms[i].matrix;
    if reroll {
        let (s, _, p) = m.to_scale_rotation_translation();
        scene.transforms[i].matrix = Mat4::from_scale_rotation_translation(s, roll::random_quat(rng), p);
        log.push(format!("T{} turned anew", i));
    } else {
        // Rotate the linear part around a random axis through its origin
        let axis = random_unit(rng);
        let deg = rng.gen_range(5.0..30.0) * strength;
        scene.transforms[i].matrix =
            crate::rot::turn_linear_part(m, crate::rot::Turn::about(axis, deg.to_radians()));
        log.push(format!(
            "T{} rotate {:.0}° about ({:+.2},{:+.2},{:+.2})",
            i, deg, axis.x, axis.y, axis.z
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn scale(
    scene: &mut Scene,
    i: usize,
    reroll: bool,
    settings: &RandomSettings,
    strength: f32,
    rng: &mut impl Rng,
    log: &mut Vec<String>,
) {
    let m = &mut scene.transforms[i].matrix;
    if reroll {
        let (_, r, p) = m.to_scale_rotation_translation();
        let s = roll::random_scale(settings, rng);
        *m = Mat4::from_scale_rotation_translation(s, r, p);
        log.push(format!("T{} resized to ({:.2},{:.2},{:.2})", i, s.x, s.y, s.z));
    } else {
        let f = (rng.gen_range(-0.25..0.25) * strength).exp();
        m.x_axis *= f;
        m.y_axis *= f;
        m.z_axis *= f;
        log.push(format!("T{} scale ×{:.2}", i, f));
    }
}

fn weight(scene: &mut Scene, i: usize, reroll: bool, strength: f32, rng: &mut impl Rng, log: &mut Vec<String>) {
    let w = &mut scene.transforms[i].weight;
    if reroll {
        // The range `roll::random_transform` draws from.
        *w = rng.gen_range(0.5..2.0);
        log.push(format!("T{} weight {:.2}", i, w));
    } else {
        let f = (rng.gen_range(-0.5..0.5) * strength).exp();
        *w = (*w * f).clamp(0.01, 100.0);
        log.push(format!("T{} weight ×{:.2}", i, f));
    }
}

#[allow(clippy::too_many_arguments)]
fn variations(
    scene: &mut Scene,
    i: usize,
    reroll: bool,
    settings: &RandomSettings,
    strength: f32,
    rng: &mut impl Rng,
    log: &mut Vec<String>,
) {
    let pool = settings.enabled_slots();
    let t = &mut scene.transforms[i];
    if reroll && !pool.is_empty() {
        // A blend made the way a fresh roll makes one: some variations off the
        // enabled list at their ranges, linear taking up the rest.
        let m = (roll::rng_range_u32(rng, settings.per_map) as usize).clamp(1, pool.len());
        t.variations = [0.0; NUM_VARIATIONS];
        let mut names = Vec::new();
        for slot in roll::choose(rng, &pool, m) {
            t.variations[slot] = roll::roll_weight(settings, slot, rng);
            names.push(format!("{} {:.2}", VARIATION_NAMES[slot], t.variations[slot]));
        }
        let nonlinear: f32 = t.variations[1..].iter().map(|w| w.abs()).sum();
        t.variations[0] = (1.0 - nonlinear).max(0.0);
        log.push(format!("T{} variations re-rolled: {}", i, names.join(", ")));
        return;
    }
    // Usually adjust an existing slot, sometimes bring in a new one from the
    // enabled list.
    let existing: Vec<usize> =
        t.variations.iter().enumerate().filter(|&(_, &w)| w != 0.0).map(|(s, _)| s).collect();
    let introduce = !pool.is_empty() && (existing.is_empty() || rng.r#gen::<f32>() < 0.4);
    if introduce {
        let slot = pool[rng.gen_range(0..pool.len())];
        let w = roll::roll_weight(settings, slot, rng) * NUDGE_SHARE * strength;
        t.variations[slot] += w;
        log.push(format!("T{} +{} {:.2}", i, VARIATION_NAMES[slot], w));
    } else {
        // `applicable` guarantees one of the two branches has something.
        let slot = existing[rng.gen_range(0..existing.len())];
        let dw = rng.gen_range(-0.15..0.15) * strength;
        let w = &mut t.variations[slot];
        *w = (*w + dw).clamp(-4.0, 4.0);
        if w.abs() < 0.03 {
            *w = 0.0;
            log.push(format!("T{} -{} (removed)", i, VARIATION_NAMES[slot]));
        } else {
            log.push(format!("T{} {} {:+.2} -> {:.2}", i, VARIATION_NAMES[slot], dw, w));
        }
    }
}

/// The colour operator, which is a different thing in each mode. Hue-shifting
/// one transform's RGB is meaningless once a palette owns the colours —
/// nothing downstream reads it — so in palette mode the gradient is what
/// changes instead. Without this, `U` in palette mode would spend a mutation
/// doing nothing visible.
fn color(scene: &mut Scene, i: usize, reroll: bool, strength: f32, rng: &mut impl Rng, log: &mut Vec<String>) {
    let palette_mode = scene.color_mode == crate::scene::ColorMode::Palette && scene.palette.is_some();
    match (palette_mode, reroll) {
        (true, true) => {
            scene.set_palette(crate::palette::random::palette(rng));
            log.push("palette re-rolled".to_string());
        }
        (true, false) => {
            if let Some(p) = scene.palette.as_mut() {
                log.push(crate::palette::random::perturb(p, rng, strength));
            }
        }
        (false, true) => {
            scene.colors[i] = roll::hsv_to_rgb(rng.gen_range(0.0..360.0), rng.gen_range(0.6..1.0), rng.gen_range(0.7..1.0));
            log.push(format!("T{} colour re-rolled", i));
        }
        (false, false) => {
            let deg = rng.gen_range(-90.0..90.0) * strength;
            scene.colors[i] = rotate_hue(scene.colors[i], deg);
            log.push(format!("T{} hue {:+.0}°", i, deg));
        }
    }
}

fn color_value(scene: &mut Scene, i: usize, reroll: bool, strength: f32, rng: &mut impl Rng, log: &mut Vec<String>) {
    let cv = &mut scene.transforms[i].color_value;
    if reroll {
        *cv = rng.r#gen();
        log.push(format!("T{} color_value {:.2}", i, cv));
    } else {
        let d = rng.gen_range(-0.2..0.2) * strength;
        *cv = (*cv + d).rem_euclid(1.0);
        log.push(format!("T{} color_value {:+.2}", i, d));
    }
}

#[allow(clippy::too_many_arguments)]
fn add(
    scene: &mut Scene,
    i: usize,
    reroll: bool,
    settings: &RandomSettings,
    strength: f32,
    rng: &mut impl Rng,
    log: &mut Vec<String>,
) {
    let n = scene.transforms.len();
    if reroll {
        // A map made the way a fresh roll makes one, in map `i`'s layer.
        let mut spec = roll::random_transform(settings, rng);
        spec.layer = scene.transforms[i].layer;
        let pool = settings.enabled_slots();
        if !pool.is_empty() && rng.r#gen::<f32>() >= settings.affine {
            let m = (roll::rng_range_u32(rng, settings.per_map) as usize).min(pool.len());
            for slot in roll::choose(rng, &pool, m) {
                spec.variations[slot] = roll::roll_weight(settings, slot, rng);
            }
            let nonlinear: f32 = spec.variations[1..].iter().map(|w| w.abs()).sum();
            spec.variations[0] = (1.0 - nonlinear).max(0.0);
        }
        spec.color_value = rng.r#gen();
        let color = roll::hsv_to_rgb(rng.gen_range(0.0..360.0), rng.gen_range(0.6..1.0), rng.gen_range(0.7..1.0));
        scene.insert_transform(n, spec, None, Some(color), None);
        log.push(format!("new map T{}", n));
    } else {
        // Duplicate with a translate nudge
        let mut spec = scene.transforms[i].clone();
        let d = random_unit(rng) * rng.gen_range(0.05..0.2) * strength;
        spec.matrix.w_axis += d.extend(0.0);
        let name = scene.transform_names[i].as_ref().map(|s| format!("{} mut", s));
        let color = scene.colors[i];
        scene.insert_transform(n, spec, name, Some(color), Some(i));
        log.push(format!("duplicate T{} -> T{}", i, n));
    }
}

fn background(scene: &mut Scene, reroll: bool, strength: f32, rng: &mut impl Rng, log: &mut Vec<String>) {
    if reroll {
        let tint = roll::tint(scene, rng);
        scene.background = roll::random_background(rng, tint);
        log.push("background re-rolled".to_string());
    } else {
        let deg = rng.gen_range(-30.0..30.0) * strength;
        let level = (rng.gen_range(-0.3f32..0.3) * strength).exp();
        let bg = crate::palette::random::rotate_hue(scene.background, deg) * level;
        scene.background = bg.clamp(Vec3::ZERO, Vec3::ONE);
        log.push(format!("background hue {:+.0}°, level ×{:.2}", deg, level));
    }
}

/// Nudge: the group's axis tilts. Re-roll: another group off the profile's
/// symmetry settings on the same motifs — or, on a flame without one, a group
/// made the way a fresh roll makes it.
#[allow(clippy::too_many_arguments)]
fn symmetry(
    scene: &mut Scene,
    i: usize,
    reroll: bool,
    settings: &RandomSettings,
    strength: f32,
    rng: &mut impl Rng,
    log: &mut Vec<String>,
) {
    use crate::symmetry::Symmetry;
    // The group map `i` is in, else the first one in the scene.
    let group = scene.transforms[i]
        .symmetry
        .clone()
        .or_else(|| scene.transforms.iter().find_map(|t| t.symmetry.clone()));
    let can_reroll = settings.symmetry.any_kind();
    let Some(old) = group else {
        if !can_reroll {
            return;
        }
        // No group yet: give map `i`'s layer one, as a fresh roll would.
        let layer = scene.transforms[i].layer;
        let idx: Vec<usize> = (0..scene.transforms.len()).filter(|&j| scene.transforms[j].layer == layer).collect();
        let Some(sym) = roll::random_symmetry(&settings.symmetry, rng) else { return };
        let free = (roll::rng_range_u32(rng, settings.symmetry.free) as usize).min(idx.len() - 1);
        let k = roll::motif_shrink(&sym);
        for j in roll::choose(rng, &idx, idx.len() - free) {
            let (s, r, p) = scene.transforms[j].matrix.to_scale_rotation_translation();
            scene.transforms[j].matrix = Mat4::from_scale_rotation_translation(s * k, r, p);
            scene.transforms[j].symmetry = Some(sym.clone());
        }
        log.push(format!("symmetry {} added", sym.label()));
        return;
    };
    let (new, rescale, line) = if reroll && can_reroll {
        let Some(sym) = roll::random_symmetry(&settings.symmetry, rng) else { return };
        // The motifs were shrunk for the old group's size; size them for the new.
        let k = roll::motif_shrink(&sym) / roll::motif_shrink(&old);
        let line = format!("symmetry {} -> {}", old.label(), sym.label());
        (sym, k, line)
    } else {
        let tilt = (rng.gen_range(3.0f32..15.0) * strength).to_radians();
        let axis = glam::Quat::from_axis_angle(random_unit(rng), tilt) * old.axis();
        let Ok(sym) = Symmetry::new(old.kind(), axis, old.mirror(), old.color()) else { return };
        let line = format!("symmetry {} axis tilted", old.label());
        (sym, 1.0, line)
    };
    for t in scene.transforms.iter_mut().filter(|t| t.symmetry.as_ref() == Some(&old)) {
        if rescale != 1.0 {
            let (s, r, p) = t.matrix.to_scale_rotation_translation();
            t.matrix = Mat4::from_scale_rotation_translation(s * rescale, r, p);
        }
        t.symmetry = Some(new.clone());
    }
    log.push(line);
}

/// Map `i`'s layer's transition graph: re-rolled whole, the way a fresh roll
/// draws one (which may be none at all), or nudged — one edge out of map `i`
/// cut, restored, or its multiplier scaled in log space. A layer with no xaos
/// is nudged by one cut, the smallest step into it.
///
/// Never cuts a map's last exit, and never leaves an infinite-zoom scene
/// with a map that may not be followed by the zoom map, which would not load.
fn xaos(
    scene: &mut Scene,
    i: usize,
    reroll: bool,
    settings: &RandomSettings,
    strength: f32,
    rng: &mut impl Rng,
    log: &mut Vec<String>,
) {
    let idx = layer_of(scene, i);
    let n = idx.len();
    let before: Vec<_> = idx.iter().map(|&j| scene.transforms[j].xaos.clone()).collect();
    let had = before.iter().any(|x| !x.is_default());

    let line = if reroll && settings.xaos.draws() {
        match roll::random_xaos(&settings.xaos, n, rng) {
            Some(graph) => {
                for (&j, row) in idx.iter().zip(graph.rows) {
                    scene.transforms[j].xaos = row;
                }
                format!("xaos -> {}", graph.label)
            }
            None if had => {
                for &j in &idx {
                    scene.transforms[j].xaos = Default::default();
                }
                "xaos cleared".to_string()
            }
            None => return,
        }
    } else {
        if !had && !settings.xaos.draws() {
            return;
        }
        let row = scene.transforms[i].xaos.dense(n);
        let open: Vec<usize> = (0..n).filter(|&j| row[j] > 0.0).collect();
        let shut: Vec<usize> = (0..n).filter(|&j| row[j] == 0.0).collect();
        // What can be done to this row: a cut needs another exit left, and
        // a layer that has no xaos yet only takes a cut.
        let mut ops = Vec::new();
        if open.len() >= 2 {
            ops.push(0);
        }
        if had && !shut.is_empty() {
            ops.push(1);
        }
        if had && !open.is_empty() {
            ops.push(2);
        }
        let Some(&op) = ops.get(rng.gen_range(0..ops.len().max(1))) else { return };
        let xaos = &mut scene.transforms[i].xaos;
        match op {
            0 => {
                let j = open[rng.gen_range(0..open.len())];
                xaos.set(j, 0.0);
                format!("xaos T{}>T{} cut", i, idx[j])
            }
            1 => {
                let j = shut[rng.gen_range(0..shut.len())];
                xaos.set(j, 1.0);
                format!("xaos T{}>T{} restored", i, idx[j])
            }
            _ => {
                let j = open[rng.gen_range(0..open.len())];
                let step = rng.gen_range(0.3f32..1.0) * strength * if rng.r#gen::<bool>() { 1.0 } else { -1.0 };
                let [lo, hi] = super::settings::MULTIPLIER_LIMITS;
                let x = (row[j] * 3f32.powf(step)).clamp(lo, hi);
                xaos.set(j, x);
                format!("xaos T{}>T{} x{:.2} -> x{:.2}", i, idx[j], row[j], x)
            }
        }
    };

    if let Some(z) = scene.zoom.as_ref().map(|z| z.map) {
        if crate::xaos::zoom_conflict(&scene.transforms, &[], z).is_some() {
            for (&j, row) in idx.iter().zip(before) {
                scene.transforms[j].xaos = row;
            }
            return;
        }
    }
    log.push(line);
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
    roll::hsv_to_rgb(h + deg, s, max)
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

    /// In palette mode the gradient is what colour *is*, so `U` has to reach
    /// it. Before this, every colour mutation edited per-transform RGB that
    /// nothing downstream read, and mutating a palette scene never changed
    /// its colours at all.
    #[test]
    fn mutation_reaches_the_palette_in_palette_mode() {
        let settings = RandomSettings::default();
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(11);
        let mut scene = test_scene("fracturize_mutate_palette");
        scene.set_palette(crate::palette::library::get("ember").unwrap());

        let mut palette_changed = false;
        for _ in 0..60 {
            let before = scene.palette.clone().unwrap();
            let log = mutate(&mut scene, &settings, 1.0, &mut rng);
            if scene.palette.as_ref() != Some(&before) {
                palette_changed = true;
                assert!(
                    log.iter().any(|l| l.starts_with("palette")),
                    "a palette change must be reported: {log:?}"
                );
            }
            // Whatever happened, the rendered gradient must still agree with
            // the palette — a stale colormap would show the old colours.
            assert_eq!(scene.colormap, scene.effective_palette().to_colormap());
            assert_eq!(scene.color_mode, crate::scene::ColorMode::Palette);
        }
        assert!(palette_changed, "60 mutations never touched the palette");
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_palette")).ok();
    }

    /// ...and in transforms mode it must still do the old thing.
    #[test]
    fn mutation_still_shifts_transform_hues_without_a_palette() {
        let settings = RandomSettings::default();
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(11);
        let mut scene = test_scene("fracturize_mutate_hue");
        let before = scene.colors.clone();
        let mut saw_hue = false;
        for _ in 0..60 {
            let log = mutate(&mut scene, &settings, 1.0, &mut rng);
            saw_hue |= log.iter().any(|l| l.contains("hue"));
            assert!(scene.palette.is_none(), "transforms mode must not invent a palette");
        }
        assert!(saw_hue, "60 mutations never shifted a transform hue");
        assert_ne!(scene.colors, before);
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_hue")).ok();
    }

    /// Every part, nudged and re-rolled, keeps the scene consistent and
    /// saveable.
    #[test]
    fn mutations_stay_consistent_and_saveable() {
        let mut settings = RandomSettings::default();
        // Every part on, half re-rolls, so every operator runs.
        for p in settings.parts.all_mut() {
            *p = crate::dice::Part { odds: 1.0, reroll: 0.5 };
        }
        settings.symmetry.chance = 1.0;
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(42);
        let dir = std::env::temp_dir().join("fracturize_mutate_consistent");
        for round in 0..80 {
            let mut scene = test_scene("fracturize_mutate_consistent");
            let log = mutate(&mut scene, &settings, 1.0, &mut rng);
            assert!(!log.is_empty(), "round {}: no ops logged", round);
            // Parallel arrays stay in sync
            assert_eq!(scene.transforms.len(), scene.colors.len());
            assert_eq!(scene.transforms.len(), scene.transform_names.len());
            assert!(scene.transforms.len() >= 2);
            assert!(scene.transforms.len() <= 5, "round {round}: past the roll's most maps");
            // Everything stays finite
            for t in &scene.transforms {
                assert!(t.matrix.to_cols_array().iter().all(|v| v.is_finite()), "round {round}: {log:?}");
                assert!(t.weight.is_finite() && t.weight > 0.0);
            }
            assert!(scene.background.to_array().iter().all(|c| (0.0..=1.0).contains(c)));
            // And the result round-trips through the TOML format
            let out = dir.join("mutated.toml");
            scene.save(&out).unwrap();
            Scene::load(&out).unwrap_or_else(|e| panic!("round {round}: {e}\n{log:?}"));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Every part off but part `k`.
    fn only(k: usize, reroll: f32) -> crate::dice::Parts {
        let mut parts = crate::dice::Parts::default();
        for (i, p) in parts.all_mut().into_iter().enumerate() {
            *p = crate::dice::Part { odds: if i == k { 1.0 } else { 0.0 }, reroll };
        }
        parts
    }

    /// A part that's off is never touched: a profile can mask everything but
    /// colour.
    #[test]
    fn zeroed_parts_are_left_alone() {
        let mut settings = RandomSettings::default();
        settings.parts = only(part::COLOR, 0.3);
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(5);
        let base = test_scene("fracturize_mutate_mask");
        for _ in 0..30 {
            let mut scene = base.clone();
            mutate(&mut scene, &settings, 1.0, &mut rng);
            let maps = |s: &Scene| s.transforms.iter().map(|t| (t.matrix, t.weight, t.variations, t.color_value)).collect::<Vec<_>>();
            assert_eq!(maps(&scene), maps(&base), "a masked mutation moved a map");
            assert_eq!(scene.background, base.background);
        }
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_mask")).ok();
    }

    /// An operator's work is never thrown away by a later one deleting its
    /// map: a mutation that moved T1 doesn't then delete T1.
    #[test]
    fn a_changed_map_is_not_deleted() {
        let mut settings = RandomSettings::default();
        settings.parts = only(part::TRANSLATE, 0.0);
        settings.parts.count.odds = 1.0;
        settings.mutation.changes = [4, 4];
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(9);
        let base = test_scene("fracturize_mutate_touched");
        // At its most transforms already, so a count change can only remove.
        settings.transforms = [1, base.transforms.len() as u32];
        let mut deletions = 0;
        for _ in 0..60 {
            let mut scene = base.clone();
            let log = mutate(&mut scene, &settings, 1.0, &mut rng);
            // Indices in the log are as of each step, so replay them.
            let mut touched = vec![false; base.transforms.len()];
            for line in &log {
                let i: usize = line.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).unwrap().parse().unwrap();
                if line.starts_with("delete") {
                    assert!(!touched[i], "deleted a map it had changed: {log:?}");
                    touched.remove(i);
                    deletions += 1;
                } else if line.starts_with("duplicate") || line.starts_with("new map") {
                    // A count change adds once a removal has made room.
                    touched.push(true);
                } else {
                    touched[i] = true;
                }
            }
        }
        assert!(deletions > 0, "never deleted anything, so this tested nothing");
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_touched")).ok();
    }

    /// A variation brought in by a mutation comes off the profile's enabled
    /// list — the same list a fresh roll draws from.
    #[test]
    fn new_variations_come_from_the_enabled_list() {
        let mut settings = RandomSettings::default();
        for v in settings.variations.values_mut() {
            v.enabled = false;
        }
        settings.variations.get_mut("julia").unwrap().enabled = true;
        settings.parts = only(part::VARIATIONS, 0.5);
        let julia = VARIATION_NAMES.iter().position(|&v| v == "julia").unwrap();
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(9);
        for _ in 0..40 {
            let mut scene = test_scene("fracturize_mutate_pool");
            mutate(&mut scene, &settings, 1.0, &mut rng);
            for t in &scene.transforms {
                for (slot, &w) in t.variations.iter().enumerate().skip(1) {
                    assert!(slot == julia || w == 0.0, "mutated in {}", VARIATION_NAMES[slot]);
                }
            }
        }
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_pool")).ok();
    }

    #[test]
    fn seeded_mutation_is_deterministic() {
        let settings = RandomSettings::default();
        let a = test_scene("fracturize_mutate_deterministic");
        let ra = crate::dice::mutate_seeded(&settings, &a, 7).ok().unwrap();
        let rb = crate::dice::mutate_seeded(&settings, &a, 7).ok().unwrap();
        assert_eq!(ra.log, rb.log);
        let matrices = |s: &Scene| s.transforms.iter().map(|t| t.matrix).collect::<Vec<_>>();
        assert_eq!(matrices(&ra.scene), matrices(&rb.scene));
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_deterministic")).ok();
    }

    /// A mutation that passes is held to its parent: whatever came back
    /// measures no worse on shape than the parent did.
    #[test]
    fn gated_mutations_still_render() {
        let settings = RandomSettings::default();
        let parent = test_scene("fracturize_mutate_gated");
        let mut found = 0;
        for seed in 0..12 {
            if let Ok(r) = crate::dice::mutate_seeded(&settings, &parent, seed) {
                found += 1;
                let stats = crate::dice::gate::measure(&r.scene.transforms).expect("a passed child measures");
                assert!(stats.acceptable(), "seed {seed}: {:?}", r.log);
            }
        }
        assert!(found >= 10, "only {found} of 12 mutations passed");
        std::fs::remove_dir_all(std::env::temp_dir().join("fracturize_mutate_gated")).ok();
    }

    /// A xaos mutation never leaves a dead end, and in a zoom scene never
    /// cuts the zoom map off from a map that fires — that scene wouldn't load.
    #[test]
    fn xaos_mutations_keep_the_graph_walkable() {
        use crate::dice::RandomSettings;
        let mut settings = RandomSettings::default();
        settings.parts = only(super::part::XAOS, 0.3);
        settings.xaos.random = 1.0;
        settings.xaos.preset = 1.0;
        let mut changed = 0;
        for seed in 0..60 {
            let mut scene = test_scene("xaos-mutate");
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let log = mutate(&mut scene, &settings, 1.0, &mut rng);
            if log.iter().any(|l| l.starts_with("xaos")) {
                changed += 1;
            }
            let chain = crate::xaos::Chain::build(&scene.transforms, &[]).unwrap();
            assert!(chain.dead.is_empty(), "seed {seed}: a dead end after {log:?}");
        }
        assert!(changed > 40, "only {changed}/60 mutations touched the graph");

        // In a zoom scene, every map keeps the zoom map as an exit.
        let mut scene = test_scene("xaos-mutate-zoom");
        if scene.transforms.len() >= 2 {
            scene.zoom = Some(crate::renorm::ZoomSpec { map: 0, ..Default::default() });
            for seed in 0..60 {
                let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
                mutate(&mut scene, &settings, 1.0, &mut rng);
                assert!(crate::xaos::zoom_conflict(&scene.transforms, &[], 0).is_none(), "seed {seed}");
            }
        }
    }
}
