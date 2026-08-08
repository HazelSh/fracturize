use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::palette::spec::PaletteDef;
use crate::palette::Palette;
use crate::path::{CameraPath, PathKey};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// 256-color gradient for Apophysis-style rendering. Lives in
/// `crate::palette` now that filling it is a swappable stage; re-exported
/// here because everything that reads a scene expects to find it here.
pub use crate::palette::Colormap;

/// Which source fills the 256-entry colormap.
///
/// Both modes share stage 1 (the walker's colour accumulation), so
/// `color_speed`, `color_falloff` and `color_contrast` mean the same thing in
/// each — the seam is only at the 1-D → RGB map itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// The ring built from per-transform RGB. What the renderer always did,
    /// and the lane for using colour to read IFS *structure* — each transform
    /// keeps its own identity.
    #[default]
    Transforms,
    /// An independent gradient that doesn't depend on the transform count.
    /// The Apophysis model: structure and styling become separable jobs.
    Palette,
    /// Per-transform RGB carried through the walk as a **3-vector**, mixed
    /// rather than indexed, and written straight into the point.
    ///
    /// The other two modes both reduce the walker's history to one scalar and
    /// look it up. That reduction, not the gradient, is where the information
    /// goes: a walker's history is a word over N symbols and a scalar cannot
    /// distinguish most words. Mixing three channels can — a walker that came
    /// via a red map and then a blue one is purple, and distinguishable from
    /// one that came via two magenta maps. Distinct transform *combinations*
    /// get distinct colours.
    ///
    /// Costs nothing: the walker's spare padding holds the vector and the
    /// point's spare 24 bits hold the result. `color_contrast` does not apply
    /// (it stretches a 1-D index, and there isn't one).
    Mix,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [ColorMode::Transforms, ColorMode::Palette, ColorMode::Mix];

    pub fn name(&self) -> &'static str {
        match self {
            ColorMode::Transforms => "transforms",
            ColorMode::Palette => "palette",
            ColorMode::Mix => "mix",
        }
    }

    pub fn parse(s: &str) -> Option<ColorMode> {
        ColorMode::ALL.into_iter().find(|m| m.name().eq_ignore_ascii_case(s))
    }

    /// Whether point colour is packed RGB rather than a colormap index.
    pub fn packs_rgb(&self) -> bool {
        matches!(self, ColorMode::Mix)
    }
}

/// Number of variation slots per transform (must match chaos.wgsl)
pub const NUM_VARIATIONS: usize = 20;

/// Variation names, in GPU slot order (must match apply_variations in chaos.wgsl)
pub const VARIATION_NAMES: [&str; NUM_VARIATIONS] = [
    "linear",      // 0: identity
    "sinusoidal",  // 1: sin() per component
    "spherical",   // 2: p / r^2 (inversion)
    "swirl",       // 3: rotate xy by r^2
    "horseshoe",   // 4: complex square-ish fold
    "polar",       // 5: (theta/pi, r-1)
    "disc",        // 6: (theta/pi)*(sin(pi r), cos(pi r))
    "spiral",      // 7: (cos+sin r, sin-cos r)/r
    "hyperbolic",  // 8: (sin(theta)/r, r cos(theta))
    "diamond",     // 9: (sin t cos r, cos t sin r)
    "julia",       // 10: sqrt-r half-angle with random branch
    "bent",        // 11: piecewise fold of negative x/y
    "fisheye",     // 12: 2p/(r+1) (eyefish)
    "bubble",      // 13: 4p/(r^2+4)
    "cylinder",    // 14: (sin x, y, z)
    "tangent",     // 15: (sin x / cos y, tan y, z)
    "absfold",     // 16: abs(p) — KIFS kaleidoscope fold (pair with rotations)
    "boxfold",     // 17: 2*clamp(p,±1)-p — Mandelbox box fold
    "spherefold",  // 18: Mandelbox sphere fold (minR2 0.25, fixR2 1)
    "bulb",        // 19: power-8 mandelbulb angle map, radius-preserving
];

/// A single IFS transform, fully resolved for use by the app and GPU
#[derive(Clone)]
pub struct TransformSpec {
    /// Affine part (applied before variations)
    pub matrix: Mat4,
    /// Post-affine part (applied after variations). Identity by default.
    /// This is the slot that enables symmetry: `g ∘ f` where g is drawn from
    /// a group and f is the pre-affine + variations.
    pub post_affine: Mat4,
    /// Colormap index (0.0-1.0)
    pub color_value: f32,
    /// Selection weight
    pub weight: f32,
    /// Effective color blending speed (0.0-1.0), resolved by resolve_color_speeds
    pub color_speed: f32,
    /// Explicit per-transform color_speed from the scene file, if any.
    /// Always wins over global color_speed and color_falloff.
    pub explicit_color_speed: Option<f32>,
    /// Variation blend weights, by slot (see VARIATION_NAMES)
    pub variations: [f32; NUM_VARIATIONS],
    /// The finite symmetry group this map is a motif of, if any.
    ///
    /// `None` is an ordinary transform. `Some(g)` means the chaos game draws an
    /// element of `g` uniformly *after* this map's post-affine slot and applies
    /// it, so what the attractor actually contains is the `|G|` maps
    /// `{g ∘ f : g ∈ G}` — see `crate::symmetry`. The orbit is never expanded
    /// into the transform list: this stays one transform, in the file, in the
    /// panel, in `--info` and on the GPU.
    ///
    /// A property of the transform rather than a scene-level table because
    /// that is what it is — two motifs can sit under different groups, and
    /// enrolling a map is then an edit to that map and nothing else. The
    /// `[[symmetry]]` block in a scene file is sugar over this: it names one
    /// group and the motifs that carry it, so a 120-map scene is still eight
    /// lines to read and to write.
    pub symmetry: Option<crate::symmetry::Symmetry>,
}

/// Resolve each transform's effective color_speed.
///
/// With color_falloff = 0, transforms use their explicit color_speed or the
/// global one (classic fixed-rate EMA). With color_falloff > 0, the EMA
/// retain-factor per step is tied to the transform's spatial contraction:
///
///     retained = contraction^falloff    (speed = 1 - retained)
///
/// so the color weight of the transform applied k steps ago equals the
/// spatial scale that step controls, raised to `falloff`. Color variation
/// amplitude then follows a pure power law of feature scale — detail at
/// every scale with no resonant size. Lower falloff = flatter spectrum
/// (more fine detail, but colors compress toward the mean; compensate with
/// color_contrast at render time).
pub fn resolve_color_speeds(transforms: &mut [TransformSpec], global_speed: f32, falloff: f32) {
    for t in transforms {
        t.color_speed = match t.explicit_color_speed {
            Some(s) => s,
            None if falloff > 0.0 => 1.0 - t.contraction().powf(falloff),
            None => global_speed,
        };
    }
}

/// Similarity dimension of an IFS: solve Σsᵢᵈ = 1 for d.
///
/// Each sᵢ is a contraction factor (< 1). The sum of sᵢᵈ equals 1 at exactly
/// one value of d, which is the similarity dimension — the fractal dimension
/// of the attractor when transforms overlap minimally.
///
/// Returns `None` if no transforms have weight, or if all contractions are ≥ 1
/// (the IFS expands rather than contracts).
pub fn similarity_dimension(transforms: &[TransformSpec]) -> Option<f32> {
    similarity_dimension_weighted(transforms, None)
}

/// Similarity dimension with optional per-transform weighting.
///
/// When `weights` is `Some`, uses those weights instead of the transforms'
/// own weights. This allows computing dimension for a subset of enabled maps.
pub fn similarity_dimension_weighted(
    transforms: &[TransformSpec],
    weights: Option<&[f32]>,
) -> Option<f32> {
    // One entry per map in the *effective* set. A motif under a symmetry group
    // contributes `|G|` maps, not one: the group's elements are orthogonal, so
    // every copy has the motif's own contraction, and `Σsᵢᵈ = 1` becomes
    // `Σ|Gᵢ|·sᵢᵈ = 1`.
    //
    // This is not a refinement — it is the difference between a number and
    // nonsense. A single map under `C5` reads as `d = 0` ("dust") if the group
    // is ignored, because one contraction can never sum to 1; counted properly
    // it is `ln 5 / ln(1/s)`, which for a half-scale motif is 2.3 and is the
    // dimension the picture actually has.
    //
    // A *repeat* is where the `|G|·sᵈ` shorthand stops working: its k-th copy
    // carries `scaleᵏ` on top of the motif's own contraction, so the copies are
    // not all the same size and the sum is `Σₖ (s·scaleᵏ)ᵈ`. Hence
    // `element_scales`, which is all ones for a group and a geometric run for a
    // repeat — without it a tapering helix reads as `count` copies of its
    // widest turn, which overstates the dimension badly.
    let contractions: Vec<f32> = transforms
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let w = weights.map_or(t.weight, |ws| ws.get(i).copied().unwrap_or(0.0));
            if w > 0.0 {
                let s = t.linear_determinant().abs().powf(1.0 / 3.0);
                if s > 0.0 && s < 1.0 {
                    let scales = t
                        .symmetry
                        .as_ref()
                        .map_or_else(|| vec![1.0], |g| g.element_scales());
                    Some(
                        scales
                            .into_iter()
                            .map(move |e| s * e)
                            .filter(|c| *c > 0.0 && *c < 1.0)
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                }
            } else {
                None
            }
        })
        .flatten()
        .collect();

    if contractions.is_empty() {
        return None;
    }

    // Bisection: find d where Σsᵢᵈ = 1
    // At d=0, sum = N > 1 (assuming N > 1 contracting maps)
    // As d → ∞, sum → 0 (each sᵢᵈ → 0 for sᵢ < 1)
    let sum_at = |d: f32| -> f32 { contractions.iter().map(|&s| s.powf(d)).sum() };

    let mut lo = 0.0f32;
    let mut hi = 10.0f32;

    // If even at d=0 the sum is < 1, there's only one transform
    if sum_at(lo) < 1.0 {
        return Some(0.0);
    }
    // Expand upper bound if needed
    while sum_at(hi) > 1.0 && hi < 100.0 {
        hi *= 2.0;
    }

    // Bisect for ~200 steps gives ~6 decimal places
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if sum_at(mid) > 1.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    Some((lo + hi) / 2.0)
}

/// The factor every *other* map's contraction must be multiplied by to hold
/// the similarity dimension `d` fixed while one map moves `old_s -> new_s`.
///
/// From `Σsᵢᵈ = 1`: the edited map contributes `sₖᵈ`, so the rest contribute
/// `1 - sₖᵈ`. Requiring the same identity after the edit, with the rest scaled
/// by a common `f`:
///
/// ```text
///     sₖ'ᵈ + fᵈ·(1 - sₖᵈ) = 1   =>   f = ((1 - sₖ'ᵈ) / (1 - sₖᵈ))^(1/d)
/// ```
///
/// `None` when the balance has no solution: the edited map was already the
/// whole sum (nothing left to take up the slack), or the new scale would need
/// the others to vanish. Callers leave the scene alone in that case rather
/// than collapsing it.
pub fn dimension_lock_factor(d: f32, old_s: f32, new_s: f32) -> Option<f32> {
    if !(d > 1e-4) {
        return None;
    }
    let others = 1.0 - old_s.powf(d);
    let needed = 1.0 - new_s.powf(d);
    if others < 1e-6 || needed < 1e-6 {
        return None;
    }
    let f = (needed / others).powf(1.0 / d);
    f.is_finite().then_some(f)
}

impl TransformSpec {
    /// Determinant of the map's whole linear part — the pre-affine matrix and
    /// the post-affine slot together, since `det(BA) = det(B)·det(A)`.
    ///
    /// Every contraction question has to ask this rather than `matrix` alone.
    /// A KIFS-style map carries its scale in the post-affine slot and leaves
    /// the pre-affine at identity; read only `matrix` and it looks like a map
    /// that does not contract at all, which silently takes it out of the
    /// similarity dimension, out of the dimension lock, and out of
    /// `color_falloff`'s speed resolution.
    ///
    /// Signed, because a negative determinant is a map that turns space inside
    /// out and that is worth being able to see. Note this is exact only for
    /// affine maps: a nonlinear variation sits between the two matrices and
    /// has a Jacobian of its own that no single number captures.
    pub fn linear_determinant(&self) -> f32 {
        self.matrix.determinant() * self.post_affine.determinant()
    }

    /// Spatial contraction factor of the linear part (cube root of
    /// [`Self::linear_determinant`]), clamped away from 0 and 1 so
    /// falloff-derived speeds stay sane for degenerate or expanding transforms.
    pub fn contraction(&self) -> f32 {
        self.linear_determinant().abs().powf(1.0 / 3.0).clamp(0.05, 0.95)
    }

    /// Weights for a pure-linear (classic affine) transform
    pub fn linear_variations() -> [f32; NUM_VARIATIONS] {
        let mut v = [0.0; NUM_VARIATIONS];
        v[0] = 1.0;
        v
    }

    /// Short summary of variation weights, e.g. "spherical 0.70 + linear 0.30"
    pub fn variation_summary(&self) -> String {
        let mut parts: Vec<(usize, f32)> = self
            .variations
            .iter()
            .enumerate()
            .filter(|&(_, &w)| w != 0.0)
            .map(|(i, &w)| (i, w))
            .collect();
        if parts.is_empty() {
            return "none".to_string();
        }
        if parts.len() == 1 && parts[0].0 == 0 {
            return "linear".to_string();
        }
        parts.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap());
        parts
            .iter()
            .map(|(i, w)| format!("{} {:.2}", VARIATION_NAMES[*i], w))
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// Resolve the `[[symmetry]]` blocks onto the motifs they name.
///
/// After this, nothing downstream knows blocks exist: a symmetry is a property
/// of a transform (`TransformSpec::symmetry`), which is what lets the panel edit
/// it per map, the GPU carry it per map, and a motif be withdrawn from its group
/// without touching anything else.
///
/// Every failure here is a load error rather than a warning. A scene that
/// quietly rendered without the symmetry it asked for would be a scene whose
/// picture silently disagreed with its file — and the failure modes are all
/// typos (a misspelled motif name, a group that isn't a group), which are much
/// cheaper to fix when something says so.
fn apply_symmetries(
    defs: &[SymmetryDef],
    transforms: &mut [TransformSpec],
    names: &[Option<String>],
) -> Result<(), String> {
    if defs.is_empty() {
        return Ok(());
    }

    let mut total_elements = 0usize;
    let mut claimed: Vec<Option<usize>> = vec![None; transforms.len()];

    for (block, def) in defs.iter().enumerate() {
        let kind = match (&def.group, def.repeat) {
            (Some(_), Some(_)) => {
                return Err(format!(
                    "[[symmetry]] #{} names both `group` and `repeat`. A block is \
                     one or the other — a group's copies are its elements, a \
                     repeat's are its steps",
                    block + 1
                ));
            }
            (None, None) => {
                return Err(format!(
                    "[[symmetry]] #{} names neither `group` nor `repeat`. Use \
                     `group = \"Icosa\"` for a symmetry group, or `repeat = 8` \
                     with `step`/`turn`/`shrink` for a progression",
                    block + 1
                ));
            }
            (Some(g), None) => crate::symmetry::OrbitKind::parse(g)
                .map_err(|e| format!("[[symmetry]] #{}: {}", block + 1, e))?,
            (None, Some(count)) => {
                let d = crate::symmetry::Repeat::default();
                crate::symmetry::OrbitKind::Repeat(crate::symmetry::Repeat {
                    count,
                    translate: def
                        .step
                        .map(|v| Vec3::from(v.map(|c| c as f32)))
                        .unwrap_or(d.translate),
                    turn: def.turn.map(|t| t as f32).unwrap_or(d.turn),
                    scale: def.shrink.map(|t| t as f32).unwrap_or(d.scale),
                })
            }
        };
        let axis = def
            .axis
            .map(|a| Vec3::from(a.map(|v| v as f32)))
            .unwrap_or(Vec3::Y);
        let color = def
            .color
            .as_deref()
            .map(crate::symmetry::OrbitColor::parse)
            .transpose()
            .map_err(|e| format!("[[symmetry]] #{}: {}", block + 1, e))?
            .unwrap_or_default();

        let group =
            crate::symmetry::Symmetry::new(kind, axis, def.mirror.unwrap_or(false), color)
                .map_err(|e| format!("[[symmetry]] #{}: {}", block + 1, e))?;

        if def.applies_to.is_empty() {
            return Err(format!(
                "[[symmetry]] #{} ({}) governs no transforms. Name them in \
                 `applies_to = [\"...\"]`",
                block + 1,
                group.label()
            ));
        }

        total_elements += group.order();
        if total_elements > crate::gpu::points::compute::MAX_GROUP_ELEMENTS {
            return Err(format!(
                "this scene's symmetry groups need {} elements, more than the \
                 {} the renderer holds. Use fewer distinct groups, or smaller ones",
                total_elements,
                crate::gpu::points::compute::MAX_GROUP_ELEMENTS
            ));
        }

        for reference in &def.applies_to {
            let idx = resolve_transform_ref(reference, names).map_err(|e| {
                format!("[[symmetry]] #{} ({}): {}", block + 1, group.label(), e)
            })?;
            if let Some(first) = claimed[idx] {
                return Err(format!(
                    "transform '{}' is claimed by two symmetry blocks (#{} and #{}). \
                     A map belongs to one group; to compose two symmetries, use the \
                     larger group that contains both",
                    reference,
                    first + 1,
                    block + 1
                ));
            }
            claimed[idx] = Some(block);
            transforms[idx].symmetry = Some(group.clone());
        }
    }

    Ok(())
}

/// Group the transforms' symmetries back into `[[symmetry]]` blocks for saving.
///
/// The inverse of [`apply_symmetries`], and the reason a symmetry survives a
/// round trip through the panel: motifs sharing a group come back out as one
/// block naming all of them, in the order they appear in the transform list.
pub fn symmetry_blocks(
    transforms: &[TransformSpec],
    names: &[Option<String>],
) -> Vec<SymmetryDef> {
    let mut blocks: Vec<(&crate::symmetry::Symmetry, Vec<String>)> = Vec::new();

    for (i, t) in transforms.iter().enumerate() {
        let Some(sym) = t.symmetry.as_ref() else { continue };
        // A motif with no name is referenced by index, which `applies_to`
        // accepts on the same terms as `[zoom].map` — so an unnamed transform
        // can still carry a symmetry through a save.
        let reference = names
            .get(i)
            .and_then(|n| n.clone())
            .unwrap_or_else(|| i.to_string());
        match blocks.iter_mut().find(|(s, _)| *s == sym) {
            Some((_, members)) => members.push(reference),
            None => blocks.push((sym, vec![reference])),
        }
    }

    blocks
        .into_iter()
        .map(|(sym, applies_to)| {
            let step = sym.kind().repeat();
            SymmetryDef {
                // A repeat is spelled `repeat = n`, never `group = "..."`, so
                // the two keys stay mutually exclusive on the way out as well
                // as on the way in.
                group: step.is_none().then(|| sym.kind().label()),
                repeat: step.map(|r| r.count),
                // Written even at their defaults: a repeat with `repeat = 8`
                // and nothing else is a file that doesn't say what it does, and
                // the step is the whole content of the block.
                step: step.map(|r| r.translate.to_array().map(|v| v as f64)),
                turn: step.map(|r| r.turn as f64),
                shrink: step.map(|r| r.scale as f64),
                // Only where it means something: writing an axis beside
                // `group = "Icosa"` would say the polyhedral group is aimed,
                // and it isn't.
                axis: sym
                    .kind()
                    .uses_axis()
                    .then(|| sym.axis().to_array().map(|v| v as f64)),
                mirror: sym.mirror().then_some(true),
                applies_to,
                color: match sym.color() {
                    crate::symmetry::OrbitColor::Shared => None,
                    other => Some(other.name().to_string()),
                },
            }
        })
        .collect()
}

/// Resolve a transform reference — a name, or an index written as a string —
/// against the scene's transform names. Used by `[zoom].map` and `--zoom`.
pub fn resolve_transform_ref(
    reference: &str,
    names: &[Option<String>],
) -> Result<usize, String> {
    let reference = reference.trim();
    if let Some(i) = names.iter().position(|n| n.as_deref() == Some(reference)) {
        return Ok(i);
    }
    if let Ok(i) = reference.parse::<usize>() {
        if i < names.len() {
            return Ok(i);
        }
        return Err(format!("transform index {} is out of range (0..{})", i, names.len()));
    }
    let known: Vec<&str> = names.iter().filter_map(|n| n.as_deref()).collect();
    Err(format!(
        "no transform named '{}'. Named transforms: {}",
        reference,
        if known.is_empty() { "(none)".to_string() } else { known.join(", ") }
    ))
}

/// Parse a TOML `variations` table into slot weights
fn parse_variations(table: &BTreeMap<String, f64>) -> Result<[f32; NUM_VARIATIONS], String> {
    let mut weights = [0.0f32; NUM_VARIATIONS];
    for (name, &weight) in table {
        let slot = VARIATION_NAMES
            .iter()
            .position(|&n| n == name)
            .ok_or_else(|| {
                format!(
                    "Unknown variation '{}'. Available: {}",
                    name,
                    VARIATION_NAMES.join(", ")
                )
            })?;
        weights[slot] = weight as f32;
    }
    Ok(weights)
}

/// Scene metadata from TOML
#[derive(Deserialize, Serialize)]
pub struct SceneMeta {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default = "default_point_size")]
    pub point_size: f64,
    /// Points generated per frame by the chaos game (legacy, unused by point renderer)
    #[serde(alias = "iters", default)] // backwards compat
    pub points_per_frame: usize,
    /// Temporal decay factor (0.0-1.0). Lower = sharper, higher = more accumulation
    #[serde(default = "default_decay")]
    pub decay: f64,
    #[serde(default = "default_color_speed")]
    pub color_speed: f64,
    /// Scale-aware color accumulation exponent (see resolve_color_speeds).
    /// 0 = classic fixed-rate EMA using color_speed. > 0 ties each EMA step's
    /// retain-factor to the transform's contraction^falloff, giving color
    /// detail at every spatial scale (power-law, no resonant size).
    /// ~1.0 is neutral; lower = more fine detail (raise color_contrast too).
    #[serde(default)]
    pub color_falloff: f64,
    /// Render-time contrast stretch of the colormap index around its center,
    /// wrapping cyclically. Compensates the wash-out from low color_falloff.
    #[serde(default = "default_color_contrast")]
    pub color_contrast: f64,
    /// Colour source, when the `[palette]` presence rule can't express it.
    ///
    /// That rule — palette table present means palette mode — covers two of
    /// the three modes and stays the mechanism for them, so existing files are
    /// untouched and there is no redundant key to fall out of sync. `mix` is
    /// the case it can't say, so this exists; it wins over the rule whenever
    /// it is present, and is only *written* when the rule would get it wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mode: Option<String>,
    /// Background colour behind the fractal, in **linear** 0-1 RGB — the
    /// value handed straight to `LoadOp::Clear`, which for an sRGB target is
    /// linear. Defaults to the dark blue-black every scene rendered on before
    /// this was configurable, so nothing moves. Note for hand-authoring:
    /// `[0.02, 0.02, 0.05]` is that default and looks like sRGB `#282a45` —
    /// linear values run much darker than the number suggests, and the Render
    /// window's picker does the conversion for you.
    #[serde(default = "default_background")]
    pub background: [f64; 3],
    /// Atmospheric-haze strength, 0 (off) to 1. A single amount rather than the
    /// four raw shader knobs: the near/far band auto-ranges off the camera
    /// distance (see `App::haze_range`) and the transmittance/saturation
    /// falloffs are derived from this (see `App::haze_falloff`), so there is
    /// exactly one number worth keeping with the artwork. Defaults to 0, which
    /// leaves every scene authored before this existed looking as it did.
    ///
    /// Reads `fog` too: that's what this was called when it darkened distant
    /// material instead of thinning it (see `src/haze.rs` for why it changed),
    /// and scenes were saved with it.
    #[serde(default, alias = "fog")]
    pub haze: f64,
    /// Splat-renderer exposure multiplier. Scene data, because it is the splat
    /// renderer's primary brightness control and a picture tuned without it is
    /// simply wrong.
    ///
    /// It used to live only on `App`, fed by `--exposure`: tune it, Ctrl+S,
    /// reopen, and the scene came back at 1.0 looking nothing like what you
    /// saved. The shipped scenes worked around that by recording the flags in a
    /// *comment* — `# fracturize --scene … --splat --exposure 1.8` at the top
    /// of `ammonite.toml`. When a format's workaround for a missing field is a
    /// comment telling you what to type on the command line, the field wants to
    /// exist.
    ///
    /// Defaults to 1.0, which is what every scene authored before this was
    /// getting anyway, so nothing moves.
    #[serde(default = "default_exposure")]
    pub exposure: f64,
    /// Total point buffer size for the simple point renderer.
    /// If unset, defaults to 500k.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point_count: Option<usize>,
}

fn default_point_size() -> f64 {
    0.012
}

fn default_exposure() -> f64 {
    1.0
}

fn default_color_speed() -> f64 {
    0.5
}

fn default_color_contrast() -> f64 {
    1.0
}

/// A background colour as a wgpu clear value. Linear RGB — which is what
/// `LoadOp::Clear` wants for an sRGB target, and what `Scene::background`
/// stores, so this is a repack rather than a conversion.
///
/// `alpha` is 0 for a transparent render and 1 otherwise. It only ever
/// reaches the swapchain as 1: the window has no alpha channel to composite
/// through, so transparency is a property of what gets written to a file, not
/// a preview mode.
pub fn clear_color(background: Vec3, alpha: f64) -> wgpu::Color {
    wgpu::Color {
        r: background.x as f64,
        g: background.y as f64,
        b: background.z as f64,
        a: alpha,
    }
}

/// The dark blue-black every scene rendered on before the background was
/// configurable. Linear RGB (see `SceneMeta::background`).
pub const DEFAULT_BACKGROUND: Vec3 = Vec3::new(0.02, 0.02, 0.05);

fn default_background() -> [f64; 3] {
    [
        DEFAULT_BACKGROUND.x as f64,
        DEFAULT_BACKGROUND.y as f64,
        DEFAULT_BACKGROUND.z as f64,
    ]
}

fn default_decay() -> f64 {
    0.8 // ~10 frame persistence
}

/// Transform definition in TOML (human-readable format)
#[derive(Deserialize, Serialize)]
pub struct TransformDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub translation: [f64; 3],
    #[serde(default = "default_scale")]
    pub scale: ScaleDef,
    #[serde(default)]
    pub rotation: [f64; 3], // Euler angles in degrees, extrinsic XYZ
    /// The exact rotation: a rotation vector in radians. Wins over `rotation`.
    ///
    /// XYZ euler is a chart, and it has poles: a rotation near a quarter turn
    /// about Y comes back as something like `[179.2, 87.6, -179.3]`, which is
    /// correct, unreadable, and one rounding error away from being wrong. Only
    /// written where the degrees can't reproduce the matrix, so ordinary
    /// scenes — and `tools/lsystem_to_ifs.py` — carry on unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotvec: Option<[f64; 3]>,
    pub color: [f64; 3],
    #[serde(default = "default_weight")]
    pub weight: f64,
    /// Color value for Apophysis-style colormap indexing (0.0-1.0)
    /// If not specified, auto-assigned based on transform index
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_value: Option<f64>,
    /// Per-transform color blending speed (0.0-1.0)
    /// Overrides global color_speed and color_falloff if set
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_speed: Option<f64>,
    /// Variation blend weights by name, e.g. { spherical = 0.7, linear = 0.3 }
    /// Defaults to { linear = 1.0 } (classic affine IFS)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variations: Option<BTreeMap<String, f64>>,
    /// Post-affine translation (applied after variations). Default [0,0,0].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_translation: Option<[f64; 3]>,
    /// Post-affine scale (applied after variations). Default 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_scale: Option<ScaleDef>,
    /// Post-affine rotation in degrees (applied after variations). Default [0,0,0].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_rotation: Option<[f64; 3]>,
}

fn default_scale() -> ScaleDef {
    ScaleDef::Uniform(1.0)
}

/// Uniform (`scale = 0.5`) or per-axis (`scale = [0.05, 0.6, 0.05]`) scale.
/// Per-axis scale is what makes L-system-style maps (squash onto a trunk
/// segment, long thin branches) expressible in the TOML format.
#[derive(Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(untagged)]
pub enum ScaleDef {
    Uniform(f64),
    PerAxis([f64; 3]),
}

impl ScaleDef {
    pub fn to_vec3(self) -> Vec3 {
        match self {
            ScaleDef::Uniform(s) => Vec3::splat(s as f32),
            ScaleDef::PerAxis(a) => Vec3::from(a.map(|v| v as f32)),
        }
    }
}

fn default_weight() -> f64 {
    1.0
}

/// Camera configuration from TOML
#[derive(Deserialize, Serialize, Default)]
pub struct CameraDef {
    /// Orbit center / look-at point
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<[f64; 3]>,
    /// Legacy: eye displacement off the orbit sphere. Folded into
    /// yaw/pitch/distance at load; never written back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<[f64; 3]>,
    /// Orbit radius
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    /// Orbit angle around Y in radians
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
    /// Orbit elevation in radians (positive = above the focus)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    /// Roll about the view axis in radians (0 = horizon level)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
    /// The exact framing: a rotation vector in radians. Wins over
    /// yaw/pitch/roll when present.
    ///
    /// yaw/pitch/roll are a chart, and a chart has poles: looking straight up
    /// or down they collapse into one control and stop naming a framing
    /// individually. Those framings are reachable now, so they need a way to
    /// be written down. Only used where the chart can't do the job, so
    /// ordinary scenes stay readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotvec: Option<[f64; 3]>,
    /// Camera path: how playback gets from the last frame back to the first —
    /// `"once"`, `"pingpong"`, `"closed"` or `"zoom"`. Omitted means `once`.
    /// See `path::Loop`; this is the single key that names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_loop: Option<String>,
    /// Camera path: zoom periods descended per loop, with `path_loop =
    /// "zoom"`. Omitted means one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_zoom_periods: Option<u32>,
    /// Camera path: playback/render duration in seconds (default 3s per
    /// segment traversed — which a ping-pong does twice over)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_seconds: Option<f64>,
    /// Camera path: ease in/out (default: the loop's own — see `path::Loop`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_ease: Option<bool>,
    /// Legacy: the loop used to be two independent keys, `path_closed = true`
    /// and `path_zoom_loop = <periods>`, which between them could say
    /// "returns to the first key *and* descends a zoom period" — two
    /// different loops at once. Both still load, below `path_loop`; neither is
    /// ever written back, the same treatment camera `offset` gets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_closed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_zoom_loop: Option<u32>,
    /// Camera path spline keypoints ([[camera.path]]). Omitted fields
    /// default to the base camera's values. Must be last: TOML requires
    /// scalar keys before sub-tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<PathKeyDef>>,
}

/// How far a `route` may miss the key it runs to before the scene is refused,
/// in degrees.
///
/// Not zero, because a route is written out rounded and read back in `f32`,
/// and a hand-written `6.28` for a full turn is a route someone means. Small
/// enough that anything wrong enough to see is refused: the spline hits its
/// keys by composing these displacements, so whatever slack is allowed here is
/// how far off a key the curve may pass, and half a degree is under three
/// pixels of a 1080-tall frame.
const ROUTE_MISS_LIMIT_DEG: f32 = 0.5;

/// One [[camera.path]] spline keypoint. Every field is optional and falls
/// back to the base [camera] framing, so a key only states what changes.
#[derive(Deserialize, Serialize, Clone, Copy, Default)]
pub struct PathKeyDef {
    /// The exact framing: a rotation vector in radians. Wins over
    /// yaw/pitch/roll. See [`CameraDef::rotvec`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotvec: Option<[f64; 3]>,
    /// Extra whole turns taken on the segment *leaving* this key, beyond the
    /// short way round. Only written where the yaw column can't carry the
    /// route on its own — so a hand-edited `yaw` can never contradict it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<i32>,
    /// The exact journey out of this key, as a rotation vector in radians:
    /// the displacement the segment leaving this key travels, instead of the
    /// short way plus `turns`. Wins over `turns` where both are given.
    ///
    /// This is the spelling for a route the endpoints cannot imply, and the
    /// only one: a loop *in place* about a chosen axis (equal framings leave
    /// the axis free, so a winding has nothing to be a winding of), or a
    /// corkscrew about an axis that isn't the short way's. Everything else
    /// should stay in `turns`, which cannot contradict its keys.
    ///
    /// Checked as the scene loads — `exp(route)` must land on the next key, or
    /// the load fails naming this key and the miss. See [`crate::path::Route`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<[f64; 3]>,
    /// Orbit angle in radians. Unbounded: successive keys spanning more than
    /// a full turn author spirals; nothing is wrapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<[f64; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
}

/// Infinite-zoom settings ([zoom] in TOML). See `src/renorm.rs` for what the
/// construction actually is; the short version is that one of the scene's own
/// transforms is nominated as the scale symmetry, and the attractor is then
/// rendered as the unbounded set invariant under it.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ZoomDef {
    /// Which transform renormalizes: its name, or its index as a string
    /// ("2"). It must be pure affine and contract on all three axes.
    pub map: String,
    /// Outer radius of the rendered band, as a multiple of the camera
    /// distance. Not a look control: below `renorm::MIN_RADIUS` (2.42) the
    /// band's edge enters the frustum and material blinks out at every wrap.
    #[serde(default = "default_zoom_radius")]
    pub radius: f64,
    /// Octaves of scale rendered below `radius`. Needs to cover at least the
    /// depth the camera can see into, or zooming reaches an empty core.
    #[serde(default = "default_zoom_levels")]
    pub levels: f64,
    /// How steeply the point budget falls off toward the fixed point, as a
    /// power of the contraction ratio. Leave at 0 for anything that will be
    /// flown: a falloff makes neighbouring octaves hold different numbers of
    /// points, and a wrap then changes the density on screen. Useful for a
    /// still, where it evens out density and nothing ever wraps.
    #[serde(default = "default_zoom_octave_falloff")]
    pub octave_falloff: f64,
    /// Octaves over which the picture's *outer* edge is faded to nothing, at
    /// render time, in multiples of the current eye distance. The band's edge
    /// is a cliff otherwise, and a wrap walks the picture off it — a whole
    /// octave of structure disappears between two frames.
    ///
    /// Leave it alone. The default (1 octave) fades the edge out exactly where
    /// full haze would have hidden it anyway, and costs nothing at a wrap: the
    /// ramp is measured against the camera, so the wrap's similarity leaves it
    /// unchanged. Setting 0 restores the hard edge, which is a tool for
    /// measuring the artifact (`scenes/octave-edge-visual.toml`) rather than a
    /// look. A wider one is clamped to the room the band has.
    ///
    /// See `renorm::DEFAULT_EDGE_GUARD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_guard: Option<f64>,
    /// Legacy: what this was called when it was a taper on the *point deal* —
    /// a design that could not work, because a static density profile delivers
    /// all of its change at the wrap instant. Still read, below `edge_guard`,
    /// and never written back; saving over a file that has it drops it, the
    /// same treatment `meta.fog` gets.
    ///
    /// Two fields rather than a serde alias, and for a reason worth keeping: an
    /// alias makes a file holding *both* keys a hard parse error, and `--set
    /// zoom.edge_guard=…` on a scene written against the old name produces
    /// exactly that file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub octave_fade: Option<f64>,
}

fn default_zoom_radius() -> f64 {
    crate::renorm::ZoomSpec::default().radius as f64
}

fn default_zoom_levels() -> f64 {
    crate::renorm::ZoomSpec::default().levels as f64
}

fn default_zoom_octave_falloff() -> f64 {
    crate::renorm::ZoomSpec::default().octave_falloff as f64
}

fn default_zoom_edge_guard() -> f64 {
    crate::renorm::ZoomSpec::default().edge_guard as f64
}


/// Full scene file structure
#[derive(Deserialize, Serialize)]
pub struct SceneFile {
    pub meta: SceneMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: Option<ZoomDef>,
    /// An independent gradient. **Its presence selects palette mode** — one
    /// mechanism, rather than a `color_mode` key that can fall out of sync
    /// with it. (`enabled = false` inside the table is the one exception, and
    /// exists so the A/B toggle survives a save.) Per-transform `color` stays
    /// in the file either way: it is still what `transforms` mode renders, so
    /// a scene can carry both and be flipped between them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palette: Option<PaletteDef>,
    /// Symmetry groups, each naming the motifs it governs.
    ///
    /// The large-scene lever: two motifs under `group = "I"` are an effective
    /// map set of 120, and the file, the panel and `--info` all still see two
    /// transforms plus a group. Deliberately a top-level array rather than a
    /// key on each transform, because that is how it reads — one group, these
    /// motifs — and because it keeps a scene's whole symmetry structure in one
    /// place you can take in at a glance. It resolves onto the transforms named
    /// in `applies_to` (see `TransformSpec::symmetry`), so *internally* it is a
    /// per-transform property and nothing downstream has to know about blocks.
    #[serde(default, rename = "symmetry", skip_serializing_if = "Vec::is_empty")]
    pub symmetries: Vec<SymmetryDef>,
    #[serde(rename = "transform")]
    pub transforms: Vec<TransformDef>,
}

/// A `[[symmetry]]` block.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SymmetryDef {
    /// `Cyc<n>`, `Dih<n>`, `Tetra`, `Octa` or `Icosa` — the bare `C<n>`, `D<n>`,
    /// `T`, `O`, `I` are accepted too. See `symmetry::OrbitKind::parse`.
    ///
    /// Exactly one of `group` and `repeat` names what the block is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// How many copies, for a repeat block. Mutually exclusive with `group`.
    ///
    /// A repeat is spelled as its own key rather than as `group = "Repeat"`
    /// because it is not a group, and because the count belongs in the data
    /// rather than inside a name that would then have to be parsed back out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// Slide per copy, for a repeat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<[f64; 3]>,
    /// Degrees about `axis` per copy, for a repeat. 137.5 is the golden angle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<f64>,
    /// Size multiplier per copy, for a repeat. `0 < shrink <= 1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shrink: Option<f64>,
    /// The rotation axis for `C_n` / `D_n`, or the axis a repeat turns about.
    /// Ignored by the polyhedral groups, which have no single axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<[f64; 3]>,
    /// Extend the group by the central inversion, doubling its order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<bool>,
    /// The transforms this group governs, by name (or by index written as a
    /// string, on the same terms as `[zoom].map`).
    pub applies_to: Vec<String>,
    /// `"shared"` (default) or `"orbit"` — see `symmetry::OrbitColor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Default point buffer size for the simple point renderer
const DEFAULT_POINT_COUNT: usize = 500_000;

/// Loaded scene ready for use
#[derive(Clone)]
pub struct Scene {
    pub name: String,
    pub author: String,
    pub point_size: f32,
    /// Points generated per frame by the density renderer's chaos game
    #[allow(dead_code)]
    pub points_per_frame: usize,
    /// Total point buffer size for the simple point renderer
    pub point_count: usize,
    /// Whether `point_count` is [`DEFAULT_POINT_COUNT`] because the file said
    /// nothing, rather than because the file said this.
    ///
    /// Point count is a render property rather than part of the scene, so a
    /// file that omits it is the normal case — but `--info` reporting 500 000
    /// with no hint of where it came from sends people looking for a line that
    /// isn't there. Derived, never serialized.
    pub point_count_defaulted: bool,
    /// Temporal decay factor (0.0-1.0)
    #[allow(dead_code)]
    pub decay: f32,
    pub color_speed: f32,
    /// Scale-aware color accumulation exponent (0 = classic fixed-rate EMA)
    pub color_falloff: f32,
    /// Render-time cyclic contrast stretch of the colormap index
    pub color_contrast: f32,
    /// Atmospheric-haze strength, 0 (off) to 1 — see `SceneMeta::haze`
    pub haze: f32,
    /// Splat-renderer exposure multiplier — see `SceneMeta::exposure`
    pub exposure: f32,
    /// Background colour, linear RGB — see `SceneMeta::background`
    pub background: Vec3,
    /// IFS transforms (affine matrix + variation blend weights)
    pub transforms: Vec<TransformSpec>,
    /// Human-readable name per transform (from scene file)
    pub transform_names: Vec<Option<String>>,
    /// Per-transform gradient color (source data for `ColorMode::Transforms`)
    pub colors: Vec<Vec3>,
    /// The scene's own gradient, if it has one. Kept even when
    /// `color_mode` is `Transforms`, so switching back and forth — and
    /// saving in between — doesn't throw the palette away.
    pub palette: Option<Palette>,
    /// Which of the two fills `colormap`
    pub color_mode: ColorMode,
    /// 256-color gradient for point coloring — the resolved output of
    /// `color_mode`, and the only thing the GPU ever sees. Rebuild it with
    /// [`Scene::regenerate_colormap`] after touching either source.
    pub colormap: Colormap,
    /// Camera orbit center / look-at point
    pub camera_focus: Vec3,
    /// Camera orbit radius
    pub camera_distance: f32,
    /// Camera framing.
    ///
    /// A quaternion rather than the yaw/pitch/roll the file is written in:
    /// those are a chart, they degenerate at the poles, and holding them here
    /// meant every framing in the app had to be one the chart could name. The
    /// conversion happens once, at the file boundary.
    pub camera_orientation: crate::rot::Orientation,
    /// Optional spline camera path ([[camera.path]] keypoints)
    pub camera_path: Option<CameraPath>,
    /// Infinite-zoom renormalization ([zoom]), resolved to a transform index
    pub zoom: Option<crate::renorm::ZoomSpec>,
}

impl Scene {
    /// The scene's base framing, as a camera.
    pub fn camera(&self) -> crate::camera::OrbitCamera {
        crate::camera::OrbitCamera {
            orientation: self.camera_orientation,
            distance: self.camera_distance,
            focus: self.camera_focus,
        }
    }

    /// Adopt a camera as the scene's base framing.
    pub fn set_camera(&mut self, cam: &crate::camera::OrbitCamera) {
        self.camera_orientation = cam.orientation;
        self.camera_distance = cam.distance;
        self.camera_focus = cam.focus;
    }

    /// An empty canvas: the smallest scene that still renders something, for
    /// building an IFS up from nothing rather than mutating one that exists.
    ///
    /// Two transforms, not one. A single contracting map has a single fixed
    /// point, so the chaos game converges to a dot and there's nothing on
    /// screen to work against; two opposed half-scale maps give a visible line
    /// of dust through the origin. Both are clean axis-aligned half-scales at
    /// ±(0.5, 0.5, 0) — no rotation, no variations, nothing to reverse-engineer
    /// out of the inspector before you start adding your own.
    pub fn blank() -> Self {
        let colors = vec![Vec3::new(0.9, 0.5, 0.25), Vec3::new(0.25, 0.6, 0.9)];
        // `generate_colormap` spreads N colours evenly around the cyclic
        // gradient, so `i / N` is where transform i's own colour sits — each
        // one pulls the running colour index toward itself and the two halves
        // of the attractor read apart. Both at the same index (the default for
        // a hand-added transform) would come out one flat colour.
        let map = |i: usize, offset: Vec3| TransformSpec {
            matrix: Mat4::from_scale_rotation_translation(
                Vec3::splat(0.5),
                glam::Quat::IDENTITY,
                offset,
            ),
            post_affine: Mat4::IDENTITY,
            color_value: i as f32 / colors.len() as f32,
            weight: 1.0,
            color_speed: default_color_speed() as f32,
            explicit_color_speed: None,
            symmetry: None,
            variations: TransformSpec::linear_variations(),
        };
        let colormap = generate_colormap(&colors);
        Self {
            name: "untitled".to_string(),
            author: String::new(),
            point_size: default_point_size() as f32,
            points_per_frame: 100_000,
            point_count: DEFAULT_POINT_COUNT,
            point_count_defaulted: true,
            decay: default_decay() as f32,
            color_speed: default_color_speed() as f32,
            color_falloff: 0.0,
            color_contrast: default_color_contrast() as f32,
            haze: 0.0,
            exposure: default_exposure() as f32,
            transforms: vec![
                map(0, Vec3::new(0.5, 0.5, 0.0)),
                map(1, Vec3::new(-0.5, -0.5, 0.0)),
            ],
            transform_names: vec![None, None],
            colors,
            palette: None,
            color_mode: ColorMode::Transforms,
            colormap,
            camera_focus: Vec3::ZERO,
            camera_distance: 3.0,
            camera_orientation: crate::rot::Orientation::from_yaw_pitch_roll(
                crate::rot::Angle::ZERO,
                crate::rot::Angle::from_radians(0.3),
                crate::rot::Angle::ZERO,
            ),
            background: DEFAULT_BACKGROUND,
            camera_path: None,
            zoom: None,
        }
    }

    /// Load a scene from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        Self::load_with(path, &[])
    }

    /// Load a scene, applying `--set path=value` overrides to its TOML first
    /// (see `src/set.rs`). The overrides are textual and land before parsing,
    /// so everything downstream sees an ordinary scene.
    pub fn load_with<P: AsRef<Path>>(path: P, overrides: &[String]) -> Result<Self, String> {
        Self::load_reporting(path, overrides).map(|(scene, _)| scene)
    }

    /// [`Self::load_with`], plus what each `--set` displaced.
    ///
    /// Only `--info` wants the second half: everything that renders wants the
    /// scene and nothing else, and the report is the one place that has to
    /// answer "did my edit land, and what did it move?".
    pub fn load_reporting<P: AsRef<Path>>(
        path: P,
        overrides: &[String],
    ) -> Result<(Self, Vec<crate::set::Applied>), String> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read scene file: {}", e))?;

        let (content, applied) = crate::set::apply_recording(&content, overrides)?;

        let scene_file: SceneFile = toml::from_str(&content)
            .map_err(|e| format!("Failed to parse scene file: {}", e))?;

        let num_transforms = scene_file.transforms.len();

        // Collect transform colors for colormap generation
        let transform_colors: Vec<Vec3> = scene_file
            .transforms
            .iter()
            .map(|t| Vec3::from(t.color.map(|v| v as f32)))
            .collect();

        let global_speed = scene_file.meta.color_speed as f32;

        let transform_names: Vec<Option<String>> = scene_file
            .transforms
            .iter()
            .map(|t| t.name.clone())
            .collect();

        let transforms: Vec<TransformSpec> = scene_file
            .transforms
            .iter()
            .enumerate()
            .map(|(i, t)| {
                // The transform chart is extrinsic XYZ degrees, and stays
                // that way: redefining it would silently reinterpret every
                // `rotation = [...]` on disk and in tools/lsystem_to_ifs.py.
                // An exact `rotvec` wins where a matrix won't survive it.
                let rotation = match t.rotvec {
                    Some(v) => crate::rot::Turn::from_rotation_vector(Vec3::from(
                        v.map(|x| x as f32),
                    ))
                    .exp(),
                    None => crate::rot::Orientation::from_xyz_degrees(
                        t.rotation.map(|v| v as f32),
                    ),
                };

                let matrix = crate::rot::Trs {
                    scale: t.scale.to_vec3(),
                    rotation,
                    translation: Vec3::from(t.translation.map(|v| v as f32)),
                }
                .matrix();

                // Color value: use explicit if provided, otherwise distribute evenly
                let color_value = t.color_value.map(|v| v as f32).unwrap_or_else(|| {
                    if num_transforms == 1 {
                        0.5
                    } else {
                        i as f32 / (num_transforms - 1) as f32
                    }
                });

                // Placeholder; resolve_color_speeds computes the effective value
                let speed = t.color_speed.map(|v| v as f32).unwrap_or(global_speed);

                let variations = match &t.variations {
                    Some(table) => parse_variations(table)
                        .map_err(|e| format!("Transform {}: {}", i, e))?,
                    None => TransformSpec::linear_variations(),
                };

                // Build post-affine matrix from optional fields (default to identity)
                let post_affine = if t.post_translation.is_some()
                    || t.post_scale.is_some()
                    || t.post_rotation.is_some()
                {
                    let post_trans = t
                        .post_translation
                        .map(|v| Vec3::from(v.map(|x| x as f32)))
                        .unwrap_or(Vec3::ZERO);
                    let post_scale = t.post_scale.unwrap_or(ScaleDef::Uniform(1.0)).to_vec3();
                    let post_rot = t
                        .post_rotation
                        .map(|v| {
                            crate::rot::Orientation::from_xyz_degrees(v.map(|x| x as f32))
                        })
                        .unwrap_or(crate::rot::Orientation::IDENTITY);
                    crate::rot::Trs {
                        scale: post_scale,
                        rotation: post_rot,
                        translation: post_trans,
                    }
                    .matrix()
                } else {
                    Mat4::IDENTITY
                };

                Ok(TransformSpec {
                    matrix,
                    post_affine,
                    color_value,
                    weight: t.weight as f32,
                    color_speed: speed,
                    explicit_color_speed: t.color_speed.map(|v| v as f32),
                    symmetry: None,
                    variations,
                })
            })
            .collect::<Result<_, String>>()?;

        let mut transforms = transforms;
        apply_symmetries(&scene_file.symmetries, &mut transforms, &transform_names)?;
        resolve_color_speeds(&mut transforms, global_speed, scene_file.meta.color_falloff as f32);

        // A `[palette]` table selects palette mode by its presence alone;
        // `enabled = false` inside it keeps the gradient on file while
        // rendering the transform ring (the A/B toggle). A malformed palette
        // is an error rather than a silent fall-back to the other mode — a
        // scene that quietly rendered the wrong colours would be worse than
        // one that refused to load.
        let palette = scene_file
            .palette
            .as_ref()
            .map(|d| d.resolve())
            .transpose()?;
        // An explicit `meta.color_mode` wins; otherwise the palette-presence
        // rule decides. Naming a mode that doesn't exist is an error rather
        // than a silent fall-back to the default — a typo that quietly
        // rendered the wrong colours would be worse than one that refused.
        let color_mode = match &scene_file.meta.color_mode {
            Some(name) => ColorMode::parse(name).ok_or_else(|| {
                format!(
                    "Unknown color_mode '{}'. Available: {}",
                    name,
                    ColorMode::ALL.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ")
                )
            })?,
            None => match (&palette, scene_file.palette.as_ref().and_then(|d| d.enabled)) {
                (Some(_), Some(false)) => ColorMode::Transforms,
                (Some(_), _) => ColorMode::Palette,
                (None, _) => ColorMode::Transforms,
            },
        };
        let colormap = match (color_mode, &palette) {
            (ColorMode::Palette, Some(p)) => p.to_colormap(),
            _ => generate_colormap(&transform_colors),
        };

        let cam = scene_file.camera.unwrap_or_default();
        let camera_focus = cam.focus.map(|f| Vec3::from(f.map(|v| v as f32))).unwrap_or(Vec3::ZERO);
        // Fully-legacy camera blocks (no yaw/pitch) default to the historical
        // slightly-elevated eye; files that specify yaw/pitch get no offset
        let default_offset = if cam.yaw.is_none() && cam.pitch.is_none() {
            Vec3::new(0.0, 1.0, 0.0)
        } else {
            Vec3::ZERO
        };
        let camera_offset = cam.offset.map(|f| Vec3::from(f.map(|v| v as f32))).unwrap_or(default_offset);
        let camera_distance = cam.distance.unwrap_or(3.0) as f32;
        // Fold the legacy eye offset into an on-sphere framing and distance
        let mut folded = crate::camera::OrbitCamera::from_legacy(
            camera_focus,
            camera_offset,
            camera_distance,
            cam.yaw.unwrap_or(0.0) as f32,
            cam.pitch.unwrap_or(0.0) as f32,
            cam.roll.unwrap_or(0.0) as f32,
        );
        // An exact framing wins over the chart: it is what gets written for
        // anything the chart can't name, so it has to be what gets read.
        if let Some(v) = cam.rotvec {
            folded.orientation =
                crate::rot::Turn::from_rotation_vector(Vec3::from(v.map(|x| x as f32))).exp();
        }
        let base_chart = folded.chart();

        // Which of the four loops this path is on. `path_loop` names it
        // outright; without it, fall back to reading the pair of legacy keys
        // that used to. A zoom loop wins over a closed one there, matching
        // what the old loader did when a file claimed both.
        let loop_kind = match &cam.path_loop {
            Some(s) => crate::path::LoopKind::parse(s).ok_or_else(|| {
                format!(
                    "camera.path_loop: unknown loop {:?} — expected \
                     \"once\", \"pingpong\", \"closed\" or \"zoom\"",
                    s
                )
            })?,
            None if cam.path_zoom_loop.is_some() => crate::path::LoopKind::Zoom,
            None if cam.path_closed == Some(true) => crate::path::LoopKind::Closed,
            None => crate::path::LoopKind::Once,
        };
        let zoom_periods = cam
            .path_zoom_periods
            .or(cam.path_zoom_loop)
            .unwrap_or(1)
            .clamp(1, 64);

        // Resolve [[camera.path]] keypoints; omitted fields inherit the base
        // (folded) camera framing
        let camera_path = match &cam.path {
            Some(defs) if !defs.is_empty() => {
                // A zoom loop is the one path that works with a single key:
                // its closing segment runs to that key's own image under the
                // symmetry, which is a real segment through real geometry.
                if defs.len() < 2 && loop_kind != crate::path::LoopKind::Zoom {
                    return Err("camera.path needs at least 2 keypoints \
                                (or one, with path_loop = \"zoom\")"
                        .to_string());
                }
                // Each key's chart, with omitted fields inherited from the
                // base framing. Kept alongside the resolved orientations
                // because the *route* lives here and nowhere else: a key at
                // yaw 6.28 is the same framing as one at 0, and only the
                // number the author wrote says a whole turn happened.
                let charts: Vec<crate::rot::YawPitchRoll> = defs
                    .iter()
                    .map(|k| crate::rot::YawPitchRoll {
                        yaw: k
                            .yaw
                            .map_or(base_chart.yaw, |v| crate::rot::Angle::from_radians(v as f32)),
                        pitch: k
                            .pitch
                            .map_or(base_chart.pitch, |v| crate::rot::Angle::from_radians(v as f32)),
                        roll: k
                            .roll
                            .map_or(base_chart.roll, |v| crate::rot::Angle::from_radians(v as f32)),
                    })
                    .collect();

                let keys: Vec<PathKey> = defs
                    .iter()
                    .zip(&charts)
                    .map(|(k, c)| PathKey {
                        orientation: match k.rotvec {
                            Some(v) => crate::rot::Turn::from_rotation_vector(Vec3::from(
                                v.map(|x| x as f32),
                            ))
                            .exp(),
                            None => crate::rot::Orientation::from_yaw_pitch_roll(
                                c.yaw, c.pitch, c.roll,
                            ),
                        },
                        distance: k.distance.map(|v| v as f32).unwrap_or(folded.distance),
                        focus: k
                            .focus
                            .map(|f| Vec3::from(f.map(|v| v as f32)))
                            .unwrap_or(camera_focus),
                    })
                    .collect();

                let closed = matches!(
                    loop_kind,
                    crate::path::LoopKind::Closed | crate::path::LoopKind::Zoom
                );
                let n = keys.len();
                let segments = match (n, closed) {
                    (0, _) => 0,
                    (1, _) => 0,
                    (n, true) => n,
                    (n, false) => n - 1,
                };
                // An explicit `route` is the journey itself and wins over
                // everything; then an explicit `turns`; otherwise read the
                // route off the chart the author drew. A key written as an
                // exact rotvec has no chart to read, so it takes the short way
                // unless told.
                let routes: Vec<crate::path::Route> = (0..segments)
                    .map(|i| {
                        let j = (i + 1) % n;
                        if let Some(v) = defs[i].route {
                            if defs[i].turns.is_some() {
                                log::warn!(
                                    "camera.path key {}: both `route` and `turns` given; \
                                     the route is the exact journey, so `turns` is ignored",
                                    i + 1
                                );
                            }
                            return crate::path::Route::Exact(
                                crate::rot::Turn::from_rotation_vector(Vec3::from(
                                    v.map(|x| x as f32),
                                )),
                            );
                        }
                        crate::path::Route::Turns(match defs[i].turns {
                            Some(t) => t,
                            // A closed path's last segment is the one nobody
                            // wrote: it runs back to key 0, and the documented
                            // convention is that it takes the shortest way
                            // there. Reading the literal jump from the last
                            // key's yaw to the first would send a corkscrew
                            // all the way back round the way it came.
                            None if j < i => 0,
                            None if defs[i].rotvec.is_some() || defs[j].rotvec.is_some() => 0,
                            None => crate::rot::winding_from_chart(charts[i], charts[j]),
                        })
                    })
                    .collect();

                // A stored displacement can disagree with the keys it connects
                // — the one thing a winding integer could never do — so it is
                // checked here, once, before anything flies it.
                for (i, r) in routes.iter().enumerate() {
                    if let crate::path::Route::Exact(t) = r {
                        let j = (i + 1) % n;
                        let miss = keys[i]
                            .orientation
                            .then_body(*t)
                            .angle_to(keys[j].orientation)
                            .to_degrees();
                        if miss > ROUTE_MISS_LIMIT_DEG {
                            let v = defs[i].route.unwrap_or_default();
                            return Err(format!(
                                "camera.path key {}: route = [{}, {}, {}] does not arrive at \
                                 key {} — it misses by {:.2}°. A route is the exact journey \
                                 between two framings, so either the route or the framing it \
                                 runs to is wrong.",
                                i + 1,
                                v[0],
                                v[1],
                                v[2],
                                j + 1,
                                miss
                            ));
                        }
                    }
                }

                Some(CameraPath {
                    keys,
                    routes,
                    loops: match loop_kind {
                        crate::path::LoopKind::Once => crate::path::Loop::Once,
                        crate::path::LoopKind::PingPong => crate::path::Loop::PingPong,
                        crate::path::LoopKind::Closed => crate::path::Loop::Closed,
                        // A zoom loop's similarity comes from [zoom], which
                        // hasn't been resolved yet. Fixed up below, and the
                        // block that does it errors rather than leaving this
                        // standing if the scene has no map to close under.
                        crate::path::LoopKind::Zoom => crate::path::Loop::Once,
                    },
                    ease: cam.path_ease,
                    seconds: cam.path_seconds.map(|v| v as f32),
                })
            }
            _ => None,
        };

        let mut camera_path = camera_path;

        // Resolve [zoom].map, which may name a transform or index one
        let zoom = match &scene_file.zoom {
            Some(z) => Some(crate::renorm::ZoomSpec {
                map: resolve_transform_ref(&z.map, &transform_names)?,
                radius: z.radius as f32,
                levels: z.levels as f32,
                octave_falloff: z.octave_falloff as f32,
                edge_guard: z
                    .edge_guard
                    .or(z.octave_fade)
                    .unwrap_or_else(default_zoom_edge_guard) as f32,
            }),
            None => None,
        };

        // A zoom-looping path closes under the renormalizing map, so it can
        // only be resolved once that map is known.
        if loop_kind == crate::path::LoopKind::Zoom {
            let spec = zoom.as_ref().ok_or_else(|| {
                "camera.path_loop = \"zoom\" needs a [zoom] map: the loop closes under \
                 the scene's scale symmetry, and without one there is no symmetry \
                 to close under"
                    .to_string()
            })?;
            let renorm = crate::renorm::Renorm::build(spec, &transforms, folded.distance)
                .map_err(|e| format!("camera.path_loop: {}", e))?;
            if let Some(path) = camera_path.as_mut() {
                path.loops = crate::path::Loop::Zoom(renorm.loop_similarity(zoom_periods));
            }
        }

        Ok((Scene {
            name: scene_file.meta.name,
            author: scene_file.meta.author.unwrap_or_else(|| "Unknown".to_string()),
            point_size: scene_file.meta.point_size as f32,
            points_per_frame: scene_file.meta.points_per_frame,
            point_count: scene_file.meta.point_count.unwrap_or(DEFAULT_POINT_COUNT),
            point_count_defaulted: scene_file.meta.point_count.is_none(),
            decay: scene_file.meta.decay as f32,
            color_speed: scene_file.meta.color_speed as f32,
            color_falloff: (scene_file.meta.color_falloff as f32).max(0.0),
            color_contrast: (scene_file.meta.color_contrast as f32).max(0.0),
            haze: (scene_file.meta.haze as f32).clamp(0.0, 1.0),
            exposure: (scene_file.meta.exposure as f32).clamp(0.01, 100.0),
            background: Vec3::from(scene_file.meta.background.map(|v| v as f32))
                .clamp(Vec3::ZERO, Vec3::ONE),
            transforms,
            transform_names,
            colors: transform_colors,
            palette,
            color_mode,
            colormap,
            camera_focus,
            camera_distance: folded.distance,
            camera_orientation: folded.orientation,
            camera_path,
            zoom,
        }, applied))
    }

    /// The gradient this scene actually renders through, whichever mode it is
    /// in. In `Transforms` mode this is built on the spot from `colors`, so
    /// the GUI can draw the colormap you are getting rather than only the one
    /// you authored.
    pub fn effective_palette(&self) -> Palette {
        match (self.color_mode, &self.palette) {
            (ColorMode::Palette, Some(p)) => p.clone(),
            // A scene in palette mode with no palette is only reachable
            // mid-edit; falling back keeps it rendering rather than white.
            _ => Palette::from_transform_colors(&self.colors),
        }
    }

    /// Rebuild the colormap from whichever source `color_mode` selects.
    /// Call after any colour edit, and after adding or removing a transform —
    /// the transform ring's spacing is `k/N`, so N changing recolours it.
    pub fn regenerate_colormap(&mut self) {
        self.colormap = self.effective_palette().to_colormap();
    }

    /// Switch colour source. Adopting palette mode with no palette in hand
    /// freezes the current transform ring into one, so the toggle always
    /// lands on the gradient that was already on screen instead of on
    /// something unrelated — you edit away from what you had.
    pub fn set_color_mode(&mut self, mode: ColorMode) {
        if mode == ColorMode::Palette && self.palette.is_none() {
            // (Mix needs nothing adopted: it reads the per-transform RGBs
            // directly, which every scene already has.)
            let mut p = Palette::from_transform_colors(&self.colors);
            p.name = None;
            self.palette = Some(p);
        }
        self.color_mode = mode;
        self.regenerate_colormap();
    }

    /// Install a gradient and switch to it.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = Some(palette);
        self.color_mode = ColorMode::Palette;
        self.regenerate_colormap();
    }

    /// Save the scene back to a TOML file, decomposing each transform matrix
    /// into translation / uniform scale / XYZ euler rotation (the format the
    /// loader understands). Only exact for matrices built that way — which is
    /// everything the loader and the in-app gizmo editing produce.
    /// Build the serialization structs for the current scene state
    fn to_scene_file(&self) -> SceneFile {
        let transforms: Vec<TransformDef> = self
            .transforms
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let trs = crate::rot::Trs::of(spec.matrix);
                let (scale, trans) = (trs.scale, trs.translation);
                // Degrees while they reproduce the matrix, which is nearly
                // always; the exact rotation vector for the rest, rather than
                // writing a chart reading that doesn't reload to what it says.
                let chartable = trs.is_faithful(spec.matrix)
                    && crate::rot::Orientation::from_xyz_degrees(trs.rotation.to_xyz_degrees())
                        .angle_to(trs.rotation)
                        < 1e-4;
                let degrees = trs.rotation.to_xyz_degrees();

                let variations: BTreeMap<String, f64> = spec
                    .variations
                    .iter()
                    .enumerate()
                    .filter(|&(_, &w)| w != 0.0)
                    .map(|(slot, &w)| (VARIATION_NAMES[slot].to_string(), tidy(w)))
                    .collect();
                let is_pure_linear =
                    variations.len() == 1 && variations.get("linear") == Some(&1.0);

                let color = self.colors.get(i).copied().unwrap_or(Vec3::ONE);
                let scale = if approx(scale.x as f64, scale.y as f64)
                    && approx(scale.x as f64, scale.z as f64)
                {
                    ScaleDef::Uniform(tidy(scale.x))
                } else {
                    ScaleDef::PerAxis(scale.to_array().map(tidy))
                };

                // Serialize post-affine only if it's not identity
                let (post_translation, post_scale, post_rotation) =
                    if spec.post_affine == Mat4::IDENTITY {
                        (None, None, None)
                    } else {
                        let post_trs = crate::rot::Trs::of(spec.post_affine);
                        let post_trans = if post_trs.translation.length_squared() > 1e-8 {
                            Some(post_trs.translation.to_array().map(tidy))
                        } else {
                            None
                        };
                        let post_scale = if (post_trs.scale - Vec3::ONE).length_squared() > 1e-8 {
                            let s = post_trs.scale;
                            Some(if approx(s.x as f64, s.y as f64) && approx(s.x as f64, s.z as f64) {
                                ScaleDef::Uniform(tidy(s.x))
                            } else {
                                ScaleDef::PerAxis(s.to_array().map(tidy))
                            })
                        } else {
                            None
                        };
                        let post_rot = if post_trs.rotation.angle_to(crate::rot::Orientation::IDENTITY) > 1e-4 {
                            Some(post_trs.rotation.to_xyz_degrees().map(tidy))
                        } else {
                            None
                        };
                        (post_trans, post_scale, post_rot)
                    };

                TransformDef {
                    name: self.transform_names.get(i).cloned().flatten(),
                    translation: trans.to_array().map(tidy),
                    scale,
                    rotation: if chartable { degrees.map(tidy) } else { [0.0; 3] },
                    rotvec: (!chartable).then(|| {
                        tidy_rotvec(
                            crate::rot::Orientation::IDENTITY
                                .shortest_turn_to(trs.rotation)
                                .as_rotation_vector(),
                        )
                    }),
                    color: color.to_array().map(tidy),
                    weight: tidy(spec.weight),
                    color_value: Some(tidy(spec.color_value)),
                    color_speed: spec.explicit_color_speed.map(tidy),
                    variations: if is_pure_linear || variations.is_empty() {
                        None
                    } else {
                        Some(variations)
                    },
                    post_translation,
                    post_scale,
                    post_rotation,
                }
            })
            .collect();

        SceneFile {
            meta: SceneMeta {
                name: self.name.clone(),
                author: Some(self.author.clone()),
                point_size: tidy(self.point_size),
                points_per_frame: self.points_per_frame,
                decay: tidy(self.decay),
                color_speed: tidy(self.color_speed),
                color_falloff: tidy(self.color_falloff),
                color_contrast: tidy(self.color_contrast),
                haze: tidy(self.haze),
                exposure: tidy(self.exposure),
                background: self.background.to_array().map(tidy),
                // Written only when the `[palette]` presence rule wouldn't
                // reproduce the mode on the next load — in practice, only for
                // `mix`. Everything else keeps expressing itself the way it
                // already does, so no existing file grows a key.
                color_mode: (self.color_mode == ColorMode::Mix)
                    .then(|| self.color_mode.name().to_string()),
                // Point count is a render property chosen in the Render
                // window and persisted to prefs, not part of the artwork, so
                // saving no longer writes it. `None` also means the merge
                // path leaves an existing `point_count =` line in a hand-
                // authored file exactly as it found it; loading still honours
                // it (below prefs, above the default).
                point_count: None,
            },
            camera: Some({
                // A 1-key path is a transient in-app authoring state; the
                // loader requires 2+ keys, so don't write it out
                let path = self
                    .camera_path
                    .as_ref()
                    .filter(|p| p.playable());
                let base_chart = self.camera_orientation.yaw_pitch_roll();
                let chartable = self.camera_orientation.chart_is_faithful();
                CameraDef {
                    focus: Some(self.camera_focus.to_array().map(tidy)),
                    offset: None,
                    distance: Some(tidy(self.camera_distance)),
                    // The readable chart where it can name the framing, the
                    // exact rotation vector where it can't — which is only at
                    // the poles, and only reachable at all since the camera
                    // stopped being three angles.
                    yaw: chartable.then(|| tidy(base_chart.yaw.radians())),
                    pitch: chartable.then(|| tidy(base_chart.pitch.radians())),
                    roll: chartable
                        .then(|| tidy(base_chart.roll.radians()))
                        .filter(|r| *r != 0.0),
                    rotvec: (!chartable).then(|| {
                        tidy_rotvec(
                            crate::rot::Orientation::IDENTITY
                                .shortest_turn_to(self.camera_orientation)
                                .as_rotation_vector(),
                        )
                    }),
                    // One key names the loop, and `once` is the default, so
                    // an ordinary path never mentions it at all.
                    path_loop: path
                        .map(|p| p.loops.kind())
                        .filter(|k| *k != crate::path::LoopKind::Once)
                        .map(|k| k.as_str().to_string()),
                    path_zoom_periods: path
                        .and_then(|p| p.loops.zoom().map(|z| z.periods))
                        .filter(|n| *n != 1),
                    // The two keys `path_loop` replaced. Read on load, never
                    // written — like camera `offset`, leaving them would let a
                    // stale one contradict the key that now decides.
                    path_closed: None,
                    path_zoom_loop: None,
                    path_seconds: path.and_then(|p| p.seconds.map(tidy)),
                    path_ease: path.and_then(|p| p.ease),
                    path: path.map(path_keys_to_defs),
                }
            }),
            // Written whenever the scene has one, even in `transforms` mode —
            // that's what `enabled = false` is for, and dropping the gradient
            // because it isn't currently selected would make the A/B toggle
            // destructive.
            palette: self
                .palette
                .as_ref()
                .map(|p| PaletteDef::from_palette(p, self.color_mode == ColorMode::Palette)),
            zoom: self.zoom.as_ref().map(|z| ZoomDef {
                // Names move around under edits and indices don't, but a name
                // is what a human wants to read; prefer it when there is one.
                map: self
                    .transform_names
                    .get(z.map)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| z.map.to_string()),
                radius: tidy(z.radius),
                levels: tidy(z.levels),
                octave_falloff: tidy(z.octave_falloff),
                edge_guard: Some(tidy(z.edge_guard)),
                octave_fade: None,
            }),
            // Regrouped from the transforms' own symmetries, so a motif
            // enrolled or withdrawn in the panel comes back out as the right
            // blocks. Motifs sharing a group share a block.
            symmetries: symmetry_blocks(&self.transforms, &self.transform_names),
            transforms,
        }
    }

    /// Save the scene back to a TOML file, decomposing each transform matrix
    /// into translation / uniform scale / XYZ euler rotation (the format the
    /// loader understands). Only exact for matrices built that way — which is
    /// everything the loader and the in-app gizmo editing produce.
    ///
    /// When the target file already exists, the existing document is edited
    /// in place: comments and formatting are preserved, and only values that
    /// actually changed are rewritten. The one structural exception is the
    /// transform list — if transforms were added or removed, the whole
    /// [[transform]] array is rebuilt (per-transform comments are lost, but
    /// the header/meta/camera comments survive).
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let file = self.to_scene_file();

        // Fresh serialization: used for new files, and as the fallback (and
        // transform-array donor) for the comment-preserving merge
        let fresh = toml::to_string(&file)
            .map_err(|e| format!("Failed to serialize scene: {}", e))?;
        let fresh = inline_variation_tables(&fresh).unwrap_or(fresh);
        let fresh = rewrite_palette_table(&fresh, file.palette.as_ref()).unwrap_or(fresh);

        let content = fs::read_to_string(path.as_ref())
            .ok()
            .and_then(|s| s.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|doc| merge_scene_into_document(doc, &file, &fresh))
            .unwrap_or(fresh);

        if let Some(dir) = path.as_ref().parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
            }
        }
        fs::write(path.as_ref(), content)
            .map_err(|e| format!("Failed to write scene file: {}", e))
    }
}

/// Round in f64 space: trims both decomposition noise and the f32->f64
/// representation error that would otherwise litter files with values like
/// 0.0020000000949949026. `+ 0.0` normalizes -0.0.
fn tidy(v: f32) -> f64 {
    (v as f64 * 10_000.0).round() / 10_000.0 + 0.0
}

/// A rotation vector, rounded for the file. Finer than [`tidy`]: this is the
/// exact form, used precisely where the readable one won't do, so rounding it
/// to four places would defeat the purpose.
fn tidy_rotvec(v: Vec3) -> [f64; 3] {
    v.to_array().map(|x| (x as f64 * 1e6).round() / 1e6 + 0.0)
}

/// Write a path's keys in the most readable form that still round-trips.
///
/// Two tiers. Ordinary paths get yaw/pitch/roll, as they always have, with the
/// yaw column **chained rather than wrapped** so the documented convention
/// survives: successive keys spanning a full turn still visibly span a full
/// turn in the file. Wrapping them into (-π, π] would keep playing correctly —
/// the routes are stored separately — while quietly killing the one thing a
/// human reads the column for.
///
/// Anything the chart can't name (a framing at the poles) gets `rotvec`. Both
/// tiers add `turns` only on the segments whose route the yaw column doesn't
/// already imply, so a hand-edited `yaw` can never end up contradicting a
/// `turns` sitting next to it.
fn path_keys_to_defs(p: &CameraPath) -> Vec<PathKeyDef> {
    use crate::rot::{winding_from_chart, Angle, Orientation, YawPitchRoll};
    const TAU: f32 = std::f32::consts::TAU;

    let n = p.keys.len();
    let segments = p.segments();
    let charts: Vec<YawPitchRoll> = p.keys.iter().map(|k| k.orientation.yaw_pitch_roll()).collect();
    let chartable = p.keys.iter().all(|k| k.orientation.chart_is_faithful());

    // Chain the yaw column through the routes, so the file shows the journey.
    let mut written = charts.clone();
    if chartable {
        for i in 1..n {
            let step = charts[i - 1].yaw.shortest_to(charts[i].yaw).radians()
                + TAU * p.winding(i - 1) as f32;
            written[i].yaw = Angle::from_radians(written[i - 1].yaw.radians() + step);
        }
    }

    // Whatever the chart doesn't already say has to be said out loud. For a
    // closed path this is usually the closing segment, whose route the chained
    // column has no room left to express.
    let route_stated = |i: usize| -> Option<i32> {
        let w = p.winding(i);
        let j = (i + 1) % n;
        // Mirror exactly what the loader will infer, or `turns` gets written
        // where it says nothing. The closing segment of a loop is the case:
        // the loader takes it the short way by rule, so a `turns = 0` there is
        // noise, and stating anything else is the only way to override it.
        let implied = if j < i || !chartable {
            0
        } else {
            winding_from_chart(written[i], written[j])
        };
        (w != implied).then_some(w)
    };

    (0..n)
        .map(|i| {
            // An exact route is written as it stands: the yaw column can't
            // carry it (that is why it exists), and `turns` can't name it.
            let (turns, route) = match (i < segments).then(|| p.route(i)) {
                Some(crate::path::Route::Exact(t)) => {
                    (None, Some(tidy_rotvec(t.as_rotation_vector())))
                }
                Some(crate::path::Route::Turns(_)) => (route_stated(i), None),
                None => (None, None),
            };
            let common = PathKeyDef {
                turns,
                route,
                distance: Some(tidy(p.keys[i].distance)),
                focus: Some(p.keys[i].focus.to_array().map(tidy)),
                ..Default::default()
            };
            if chartable {
                PathKeyDef {
                    yaw: Some(tidy(written[i].yaw.radians())),
                    pitch: Some(tidy(written[i].pitch.radians())),
                    roll: Some(tidy(written[i].roll.radians())).filter(|r| *r != 0.0),
                    ..common
                }
            } else {
                PathKeyDef {
                    rotvec: Some(tidy_rotvec(
                        Orientation::IDENTITY
                            .shortest_turn_to(p.keys[i].orientation)
                            .as_rotation_vector(),
                    )),
                    ..common
                }
            }
        })
        .collect()
}

// === Comment-preserving merge helpers ===

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 5e-5
}

fn value_as_f64(v: &toml_edit::Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// Replace a key's value, keeping the old value's decor (e.g. a trailing
/// `# comment` on the same line)
fn set_value(table: &mut toml_edit::Table, key: &str, new: toml_edit::Value) {
    let mut new = new;
    if let Some(old) = table.get(key).and_then(|i| i.as_value()) {
        *new.decor_mut() = old.decor().clone();
    }
    table[key] = toml_edit::Item::Value(new);
}

/// Set a float key unless it already holds (approximately) this value.
/// `default`: skip writing entirely when the key is absent and the value
/// equals the loader's default — keeps minimal files minimal.
fn set_f64(table: &mut toml_edit::Table, key: &str, v: f64, default: Option<f64>) {
    match table.get(key).and_then(|i| i.as_value()).and_then(value_as_f64) {
        Some(old) if approx(old, v) => {}
        Some(_) => set_value(table, key, v.into()),
        None => {
            if !default.is_some_and(|d| approx(d, v)) {
                set_value(table, key, v.into());
            }
        }
    }
}

fn set_i64(table: &mut toml_edit::Table, key: &str, v: i64, default: Option<i64>) {
    match table.get(key).and_then(|i| i.as_integer()) {
        Some(old) if old == v => {}
        Some(_) => set_value(table, key, v.into()),
        None => {
            if default != Some(v) {
                set_value(table, key, v.into());
            }
        }
    }
}

fn set_str(table: &mut toml_edit::Table, key: &str, v: &str) {
    if table.get(key).and_then(|i| i.as_str()) != Some(v) {
        set_value(table, key, v.into());
    }
}

fn set_bool(table: &mut toml_edit::Table, key: &str, v: bool) {
    if table.get(key).and_then(|i| i.as_bool()) != Some(v) {
        set_value(table, key, v.into());
    }
}

fn set_arr3(table: &mut toml_edit::Table, key: &str, v: [f64; 3], default: Option<[f64; 3]>) {
    if let Some(old) = table.get(key).and_then(|i| i.as_array()) {
        let matches = old.len() == 3
            && old
                .iter()
                .zip(v)
                .all(|(o, n)| value_as_f64(o).is_some_and(|x| approx(x, n)));
        if matches {
            return;
        }
    } else if default.is_some_and(|d| d.iter().zip(v).all(|(a, b)| approx(*a, b))) {
        return;
    }
    let mut arr = toml_edit::Array::new();
    for x in v {
        arr.push(x);
    }
    set_value(table, key, toml_edit::Value::Array(arr));
}

fn set_variations(table: &mut toml_edit::Table, vars: &Option<BTreeMap<String, f64>>) {
    let Some(map) = vars else {
        table.remove("variations");
        return;
    };
    // Compare against the existing table (inline or section form)
    let old: Option<BTreeMap<String, f64>> = match table.get("variations") {
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) => Some(
            t.iter()
                .filter_map(|(k, v)| value_as_f64(v).map(|f| (k.to_string(), f)))
                .collect(),
        ),
        Some(toml_edit::Item::Table(t)) => Some(
            t.iter()
                .filter_map(|(k, i)| {
                    i.as_value().and_then(value_as_f64).map(|f| (k.to_string(), f))
                })
                .collect(),
        ),
        _ => None,
    };
    if let Some(old) = &old {
        let same = old.len() == map.len()
            && old
                .iter()
                .zip(map.iter())
                .all(|((ka, va), (kb, vb))| ka == kb && approx(*va, *vb));
        if same {
            return;
        }
    }
    let mut it = toml_edit::InlineTable::new();
    for (k, v) in map {
        it.insert(k, (*v).into());
    }
    it.fmt();
    let had = table.remove("variations").is_some();
    let _ = had;
    table["variations"] = toml_edit::value(it);
}

/// Edit an existing scene document in place so comments and formatting
/// survive a save. Returns None (falling back to `fresh`) on any structural
/// surprise.
fn merge_scene_into_document(
    mut doc: toml_edit::DocumentMut,
    file: &SceneFile,
    fresh: &str,
) -> Option<String> {
    apply_palette_table(&mut doc, file.palette.as_ref());

    {
        let meta = doc
            .entry("meta")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()?;
        set_str(meta, "name", &file.meta.name);
        if let Some(a) = &file.meta.author {
            set_str(meta, "author", a);
        }
        set_f64(meta, "point_size", file.meta.point_size, Some(0.012));
        set_i64(meta, "points_per_frame", file.meta.points_per_frame as i64, Some(0));
        set_f64(meta, "decay", file.meta.decay, Some(0.8));
        set_f64(meta, "color_speed", file.meta.color_speed, Some(0.5));
        set_f64(meta, "color_falloff", file.meta.color_falloff, Some(0.0));
        set_f64(meta, "color_contrast", file.meta.color_contrast, Some(1.0));
        match &file.meta.color_mode {
            Some(m) => set_str(meta, "color_mode", m),
            // Dropped when the palette-presence rule can say it again, so a
            // scene that leaves `mix` doesn't keep a stale key that would
            // override the rule on the next load.
            None => {
                meta.remove("color_mode");
            }
        }
        // `haze` was called `fog` when it darkened distant material rather
        // than thinning it. The loader still reads either, so a file left
        // holding both would be ambiguous — write the new key and drop the
        // old one in the same breath.
        meta.remove("fog");
        set_f64(meta, "haze", file.meta.haze, Some(0.0));
        set_f64(meta, "exposure", file.meta.exposure, Some(default_exposure()));
        set_arr3(meta, "background", file.meta.background, Some(default_background()));
        if let Some(pc) = file.meta.point_count {
            set_i64(meta, "point_count", pc as i64, None);
        }
    }

    {
        let cam = file.camera.as_ref()?;
        let camera = doc
            .entry("camera")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()?;
        set_arr3(camera, "focus", cam.focus?, Some([0.0; 3]));
        set_f64(camera, "distance", cam.distance?, None);
        set_f64(camera, "yaw", cam.yaw?, Some(0.0));
        set_f64(camera, "pitch", cam.pitch?, Some(0.0));
        match cam.roll {
            Some(r) => set_f64(camera, "roll", r, Some(0.0)),
            // Rolled back to level: drop the key rather than write `roll = 0`
            None => {
                camera.remove("roll");
            }
        }
        // Folded into yaw/pitch/distance at load; leaving it would
        // double-apply on the next load
        camera.remove("offset");

        // The loop, in the one key that names it. The two legacy keys go the
        // way `offset` does: removed, so a stale `path_closed = true` can't
        // sit next to a `path_loop` that says otherwise.
        //
        // (`path_zoom_loop` was never written here at all, which meant turning
        // a zoom loop on and pressing Ctrl+S over an existing file silently
        // dropped it — the merge path only wrote keys it had a case for.)
        camera.remove("path_closed");
        camera.remove("path_zoom_loop");
        match &cam.path_loop {
            Some(s) => set_str(camera, "path_loop", s),
            None => {
                camera.remove("path_loop");
            }
        }
        match cam.path_zoom_periods {
            Some(n) => set_i64(camera, "path_zoom_periods", n as i64, None),
            None => {
                camera.remove("path_zoom_periods");
            }
        }
        match cam.path_seconds {
            Some(s) => set_f64(camera, "path_seconds", s, None),
            None => {
                camera.remove("path_seconds");
            }
        }
        match cam.path_ease {
            Some(b) => set_bool(camera, "path_ease", b),
            None => {
                camera.remove("path_ease");
            }
        }
        match &cam.path {
            None => {
                camera.remove("path");
            }
            Some(keys) => {
                let existing = camera
                    .get("path")
                    .and_then(|i| i.as_array_of_tables())
                    .map(|a| a.len());
                if existing == Some(keys.len()) {
                    // Same key count: update values in place (keeps comments)
                    let arr = camera.get_mut("path")?.as_array_of_tables_mut()?;
                    for (t, def) in arr.iter_mut().zip(keys) {
                        if let Some(y) = def.yaw {
                            set_f64(t, "yaw", y, None);
                        }
                        if let Some(p) = def.pitch {
                            set_f64(t, "pitch", p, None);
                        }
                        if let Some(d) = def.distance {
                            set_f64(t, "distance", d, None);
                        }
                        if let Some(f) = def.focus {
                            set_arr3(t, "focus", f, None);
                        }
                        match def.roll {
                            Some(r) => set_f64(t, "roll", r, Some(0.0)),
                            None => {
                                t.remove("roll");
                            }
                        }
                        // The route out of this key. Not a framing like the
                        // fields above — it says which way the segment
                        // *travels* — and it has to be written here too, or
                        // saving over an existing file with the same number of
                        // keys drops the corkscrew and quietly straightens the
                        // shot. Both spellings are rewritten from what the
                        // path actually holds, so the file can never end up
                        // with a `turns` and a `route` disagreeing.
                        match def.turns {
                            Some(n) => set_i64(t, "turns", n as i64, None),
                            None => {
                                t.remove("turns");
                            }
                        }
                        match def.route {
                            Some(r) => set_arr3(t, "route", r, None),
                            None => {
                                t.remove("route");
                            }
                        }
                    }
                } else {
                    // Key count changed: rebuild from the fresh serialization
                    let fresh_doc: toml_edit::DocumentMut = fresh.parse().ok()?;
                    camera["path"] = fresh_doc
                        .get("camera")
                        .and_then(|c| c.as_table())
                        .and_then(|c| c.get("path"))?
                        .clone();
                }
            }
        }
    }

    // The band. This used to be missing entirely, which meant editing the zoom
    // in the app and pressing Ctrl+S over an existing file dropped every one of
    // those edits without a word — the merge only wrote tables it had a case
    // for, and `[zoom]` wasn't one. Same class of miss as `path_zoom_loop`.
    match &file.zoom {
        Some(z) => {
            let zoom = doc
                .entry("zoom")
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()?;
            set_str(zoom, "map", &z.map);
            set_f64(zoom, "radius", z.radius, None);
            set_f64(zoom, "levels", z.levels, None);
            set_f64(zoom, "octave_falloff", z.octave_falloff, None);
            if let Some(g) = z.edge_guard {
                set_f64(zoom, "edge_guard", g, None);
            }
            // The old name for `edge_guard`, dropped as it is written, so a
            // file can never end up holding both and being ambiguous about
            // which one meant it. `meta.fog` gets the same treatment.
            zoom.remove("octave_fade");
        }
        None => {
            doc.remove("zoom");
        }
    }

    // The symmetry blocks, rebuilt wholesale rather than merged key by key.
    //
    // They are *derived* — `symmetry_blocks` regroups them from the transforms'
    // own symmetries every save — so there is no stable correspondence between
    // an existing block and a new one to merge against: enrolling one motif can
    // split a block in two or merge two into one. Rebuilding is the only
    // description of the result that is always right. The cost is comments
    // written inside a `[[symmetry]]` block; everything else in the file keeps
    // them. (Not writing this at all was the `[zoom]` mistake above, one
    // version further on: a save would have dropped the scene's symmetry
    // silently, which is as destructive as an edit gets.)
    doc.remove("symmetry");
    if !file.symmetries.is_empty() {
        let fresh_doc: toml_edit::DocumentMut = fresh.parse().ok()?;
        doc["symmetry"] = fresh_doc.get("symmetry")?.clone();
    }

    let existing_len = doc
        .get("transform")
        .and_then(|t| t.as_array_of_tables())
        .map(|a| a.len());
    if existing_len == Some(file.transforms.len()) {
        let arr = doc.get_mut("transform")?.as_array_of_tables_mut()?;
        for (i, (t, def)) in arr.iter_mut().zip(&file.transforms).enumerate() {
            match &def.name {
                Some(n) => set_str(t, "name", n),
                None => {
                    t.remove("name");
                }
            }
            set_arr3(t, "translation", def.translation, None);
            match def.scale {
                ScaleDef::Uniform(s) => {
                    // An existing array form won't approx-match a float; clear
                    // it so set_f64 writes the scalar cleanly
                    if t.get("scale").is_some_and(|i| i.as_array().is_some()) {
                        t.remove("scale");
                    }
                    set_f64(t, "scale", s, Some(1.0));
                }
                ScaleDef::PerAxis(a) => set_arr3(t, "scale", a, None),
            }
            set_arr3(t, "rotation", def.rotation, Some([0.0; 3]));
            set_arr3(t, "color", def.color, None);
            set_f64(t, "weight", def.weight, Some(1.0));
            if let Some(cv) = def.color_value {
                // The loader auto-assigns i/(n-1); don't add an explicit key
                // when the value still matches that
                let n = file.transforms.len();
                let auto = if n <= 1 { 0.5 } else { i as f64 / (n - 1) as f64 };
                set_f64(t, "color_value", cv, Some(tidy(auto as f32)));
            }
            match def.color_speed {
                Some(cs) => set_f64(t, "color_speed", cs, None),
                None => {
                    t.remove("color_speed");
                }
            }
            set_variations(t, &def.variations);
        }
    } else {
        // Transform count changed: rebuild the array from the fresh
        // serialization (loses per-transform comments only)
        let fresh_doc: toml_edit::DocumentMut = fresh.parse().ok()?;
        doc["transform"] = fresh_doc.get("transform")?.clone();
    }

    Some(doc.to_string())
}

/// Rewrite each `[transform.variations]` sub-table as an inline
/// `variations = { ... }` on the transform itself
/// Replace a document's `[palette]` with a freshly built one (or remove it).
///
/// Unlike the rest of the merge, this is a wholesale rewrite rather than a
/// key-by-key update, and deliberately so: a palette's *shape* changes under
/// editing — stops appear and vanish, a cosine palette becomes stops when you
/// grab a handle — so there is no stable set of keys to update in place. It
/// also lets serde's `[[palette.stops]]` array-of-tables come out as the
/// inline `stops = [ { at = …, color = "#…" } ]` a person would write.
fn rewrite_palette_table(content: &str, def: Option<&PaletteDef>) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = content.parse().ok()?;
    apply_palette_table(&mut doc, def);
    Some(doc.to_string())
}

fn apply_palette_table(doc: &mut toml_edit::DocumentMut, def: Option<&PaletteDef>) {
    let Some(def) = def else {
        doc.remove("palette");
        return;
    };
    let mut table = def.to_table();
    // Keep an existing table's position (and the comment above it) when there
    // is one; otherwise it lands after the tables already present.
    if let Some(toml_edit::Item::Table(old)) = doc.get("palette") {
        *table.decor_mut() = old.decor().clone();
        table.set_position(old.position().unwrap_or(usize::MAX));
    }
    doc["palette"] = toml_edit::Item::Table(table);
}

fn inline_variation_tables(content: &str) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = content.parse().ok()?;
    let transforms = doc.get_mut("transform")?.as_array_of_tables_mut()?;
    for t in transforms.iter_mut() {
        if let Some(toml_edit::Item::Table(vars)) = t.get("variations") {
            let mut inline = vars.clone().into_inline_table();
            inline.fmt();
            // Remove first so the fresh key gets clean `key = value` decor
            t.remove("variations");
            t["variations"] = toml_edit::value(inline);
        }
    }
    Some(doc.to_string())
}

#[cfg(test)]
mod tests {

    // === rotation round-trips =============================================

    /// Every transform in every shipped scene must survive a save.
    ///
    /// `rotation` is XYZ degrees, which is a chart with poles: a rotation near
    /// a quarter turn about Y reads back as something like
    /// `[179.2, 87.6, -179.3]` — correct, but a place where small errors turn
    /// into large ones. Where the degrees can't reproduce the matrix, an exact
    /// `rotvec` is written instead; either way the matrix has to come back.
    /// Build a set of maps with the given contractions, all weight 1.
    fn maps_with(contractions: &[f32]) -> Vec<TransformSpec> {
        contractions
            .iter()
            .map(|&s| TransformSpec {
                matrix: Mat4::from_scale(Vec3::splat(s)),
                post_affine: Mat4::IDENTITY,
                color_value: 0.5,
                weight: 1.0,
                color_speed: 0.5,
                explicit_color_speed: None,
                symmetry: None,
                variations: TransformSpec::linear_variations(),
            })
            .collect()
    }

    /// The Sierpinski gasket in 3D: four half-scale maps, d = log4/log2 = 2.
    #[test]
    fn similarity_dimension_matches_known_fractals() {
        let d = similarity_dimension(&maps_with(&[0.5; 4])).unwrap();
        assert!((d - 2.0).abs() < 1e-3, "4 maps at 0.5 should be d=2, got {d}");

        // Cantor-style: 2 maps at 1/3 -> log2/log3
        let d = similarity_dimension(&maps_with(&[1.0 / 3.0; 2])).unwrap();
        let want = 2.0f32.ln() / 3.0f32.ln();
        assert!((d - want).abs() < 1e-3, "2 maps at 1/3 should be {want}, got {d}");

        // 20 maps at 1/3 is the Menger sponge: log20/log3 = 2.727
        let d = similarity_dimension(&maps_with(&[1.0 / 3.0; 20])).unwrap();
        let want = 20.0f32.ln() / 3.0f32.ln();
        assert!((d - want).abs() < 1e-3, "menger should be {want}, got {d}");
    }

    /// Where a map keeps its scale must not change what the scale *means*.
    /// A KIFS map parks its contraction in the post-affine slot and leaves the
    /// pre-affine at identity; reading only `matrix` scored it as
    /// non-contracting, which dropped it out of `d` entirely (the whole scene
    /// reported `dimension n/a`) and out of `color_falloff`'s speed resolution.
    #[test]
    fn contraction_counts_both_affine_slots() {
        let pre = TransformSpec {
            matrix: Mat4::from_scale(Vec3::splat(0.5)),
            post_affine: Mat4::IDENTITY,
            ..maps_with(&[0.5])[0].clone()
        };
        let post = TransformSpec {
            matrix: Mat4::IDENTITY,
            post_affine: Mat4::from_scale(Vec3::splat(0.5)),
            ..maps_with(&[0.5])[0].clone()
        };
        let split = TransformSpec {
            matrix: Mat4::from_scale(Vec3::splat(0.25)),
            post_affine: Mat4::from_scale(Vec3::splat(2.0)),
            ..maps_with(&[0.5])[0].clone()
        };
        for (label, t) in [("pre", &pre), ("post", &post), ("split", &split)] {
            let s = t.linear_determinant().abs().powf(1.0 / 3.0);
            assert!(
                (s - 0.5).abs() < 1e-5,
                "{label}: contraction should be 0.5, got {s}"
            );
            assert!(
                (t.contraction() - 0.5).abs() < 1e-5,
                "{label}: clamped contraction should be 0.5, got {}",
                t.contraction()
            );
        }

        // And a scene built the KIFS way still has a dimension at all.
        let kifs: Vec<TransformSpec> = (0..4).map(|_| post.clone()).collect();
        let d = similarity_dimension(&kifs).expect("post-affine scale must yield a dimension");
        assert!((d - 2.0).abs() < 1e-3, "4 maps at 0.5 should be d=2, got {d}");
    }

    /// The dimension lock's whole claim is that `d` does not move. Assert it
    /// directly: scale one map, apply the factor to the rest, re-measure.
    #[test]
    fn dimension_lock_holds_the_dimension() {
        for start in [[0.5f32, 0.5, 0.5, 0.5], [0.62, 0.44, 0.44, 0.31]] {
            let mut maps = maps_with(&start);
            let d0 = similarity_dimension(&maps).unwrap();

            // Drag map 0's contraction well away from where it started, in the
            // small steps a real drag would deliver.
            let mut s0 = start[0];
            for _ in 0..40 {
                let next = s0 * 1.01;
                if next >= 0.999 {
                    break;
                }
                let d = similarity_dimension(&maps).unwrap();
                let f = dimension_lock_factor(d, s0, next).expect("balance should exist");
                for (i, m) in maps.iter_mut().enumerate() {
                    if i == 0 {
                        continue;
                    }
                    m.matrix.x_axis *= f;
                    m.matrix.y_axis *= f;
                    m.matrix.z_axis *= f;
                }
                maps[0].matrix = Mat4::from_scale(Vec3::splat(next));
                s0 = next;
            }

            let d1 = similarity_dimension(&maps).unwrap();
            assert!(
                (d1 - d0).abs() < 1e-2,
                "d drifted {d0:.4} -> {d1:.4} over a 40-step drag from {start:?}"
            );
            // And the edit actually did something.
            assert!(s0 > start[0] * 1.2, "the drag should have moved map 0");
        }
    }

    /// Parse a scene from a string, through a temp file (the loader takes a
    /// path so it can report one).
    fn load_str(toml: &str, tag: &str) -> Result<super::Scene, String> {
        let tmp = std::env::temp_dir().join(format!("fracturize-sym-{tag}.toml"));
        std::fs::write(&tmp, toml).unwrap();
        let out = super::Scene::load(&tmp);
        let _ = std::fs::remove_file(&tmp);
        out
    }

    fn two_motif_scene(block: &str) -> String {
        format!(
            r#"
[meta]
name = "test"
{block}
[[transform]]
name = "petal"
translation = [0.6, 0.1, 0.0]
scale = 0.45
color = [1.0, 0.3, 0.2]

[[transform]]
name = "spine"
translation = [0.0, 0.4, 0.0]
scale = 0.5
color = [0.2, 0.5, 1.0]
"#
        )
    }

    /// The headline of the format: six lines make an effective map set of 120
    /// while the scene still holds two transforms.
    #[test]
    fn a_symmetry_block_lands_on_the_motifs_it_names() {
        let scene = load_str(
            &two_motif_scene(
                "\n[[symmetry]]\ngroup = \"I\"\napplies_to = [\"petal\"]\n",
            ),
            "lands",
        )
        .unwrap();

        assert_eq!(scene.transforms.len(), 2, "the orbit must not be expanded");
        let petal = scene.transforms[0].symmetry.as_ref().expect("petal is enrolled");
        assert_eq!(petal.order(), 60);
        assert!(scene.transforms[1].symmetry.is_none(), "spine is not named");
    }

    #[test]
    fn a_symmetry_block_round_trips_through_a_save() {
        for (tag, block) in [
            ("cyclic", "\n[[symmetry]]\ngroup = \"C5\"\naxis = [0.0, 1.0, 0.0]\napplies_to = [\"petal\", \"spine\"]\n"),
            ("axis", "\n[[symmetry]]\ngroup = \"D3\"\naxis = [0.3, 0.9, -0.2]\napplies_to = [\"petal\"]\n"),
            ("mirror", "\n[[symmetry]]\ngroup = \"O\"\nmirror = true\napplies_to = [\"petal\"]\n"),
            ("orbit-colour", "\n[[symmetry]]\ngroup = \"C7\"\ncolor = \"orbit\"\napplies_to = [\"spine\"]\n"),
            // A repeat writes a different set of keys entirely, so it is a
            // separate path through both the reader and the writer.
            (
                "repeat",
                "\n[[symmetry]]\nrepeat = 9\naxis = [0.0, 1.0, 0.0]\nstep = [0.0, 0.21, 0.0]\n\
                 turn = 137.5\nshrink = 0.88\napplies_to = [\"petal\"]\n",
            ),
            (
                "repeat-defaults",
                "\n[[symmetry]]\nrepeat = 4\napplies_to = [\"spine\"]\n",
            ),
        ] {
            let scene = load_str(&two_motif_scene(block), tag).unwrap();
            let tmp = std::env::temp_dir().join(format!("fracturize-sym-rt-{tag}.toml"));
            let _ = std::fs::remove_file(&tmp);
            scene.save(&tmp).unwrap();
            let after = super::Scene::load(&tmp).unwrap();

            for (i, (a, b)) in scene.transforms.iter().zip(&after.transforms).enumerate() {
                assert_eq!(
                    a.symmetry.is_some(),
                    b.symmetry.is_some(),
                    "{tag}: transform {i} lost or gained a symmetry"
                );
                if let (Some(a), Some(b)) = (&a.symmetry, &b.symmetry) {
                    assert_eq!(a, b, "{tag}: transform {i}'s group changed");
                    assert_eq!(a.order(), b.order(), "{tag}: order changed");
                }
            }
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Saving over an existing file goes through the comment-preserving merge,
    /// which is a different path from a fresh write — and the one that has
    /// historically dropped whole tables (`[zoom]`, `path_zoom_loop`).
    #[test]
    fn a_save_over_an_existing_file_keeps_the_symmetry() {
        let tmp = std::env::temp_dir().join("fracturize-sym-merge.toml");
        let source = two_motif_scene(
            "\n[[symmetry]]\ngroup = \"C6\"\napplies_to = [\"petal\"]\n",
        );
        std::fs::write(&tmp, &source).unwrap();

        // Load and save back over itself, twice — the merge path both times.
        for pass in 0..2 {
            let scene = super::Scene::load(&tmp).unwrap();
            assert_eq!(
                scene.transforms[0].symmetry.as_ref().map(|s| s.order()),
                Some(6),
                "pass {pass}: the group did not survive"
            );
            scene.save(&tmp).unwrap();
        }
        let text = std::fs::read_to_string(&tmp).unwrap();
        assert!(text.contains("[[symmetry]]"), "the block was dropped:\n{text}");
        let _ = std::fs::remove_file(&tmp);
    }

    /// Withdrawing the last motif has to remove the block, not leave an empty
    /// one behind that fails to load.
    #[test]
    fn withdrawing_every_motif_removes_the_block() {
        let tmp = std::env::temp_dir().join("fracturize-sym-withdraw.toml");
        std::fs::write(
            &tmp,
            two_motif_scene("\n[[symmetry]]\ngroup = \"C4\"\napplies_to = [\"petal\"]\n"),
        )
        .unwrap();

        let mut scene = super::Scene::load(&tmp).unwrap();
        scene.transforms[0].symmetry = None;
        scene.save(&tmp).unwrap();

        let text = std::fs::read_to_string(&tmp).unwrap();
        assert!(!text.contains("[[symmetry]]"), "an empty block was left:\n{text}");
        assert!(super::Scene::load(&tmp).unwrap().transforms[0].symmetry.is_none());
        let _ = std::fs::remove_file(&tmp);
    }

    /// Every way of writing the block wrong should say so rather than render
    /// something that quietly isn't symmetric.
    #[test]
    fn malformed_symmetry_blocks_are_load_errors() {
        for (tag, block, expect) in [
            (
                "unknown group",
                "\n[[symmetry]]\ngroup = \"hexagonal\"\napplies_to = [\"petal\"]\n",
                "hexagonal",
            ),
            (
                "unknown motif",
                "\n[[symmetry]]\ngroup = \"C3\"\napplies_to = [\"petla\"]\n",
                "petla",
            ),
            (
                "no motifs",
                "\n[[symmetry]]\ngroup = \"C3\"\napplies_to = []\n",
                "applies_to",
            ),
            (
                "both group and repeat",
                "\n[[symmetry]]\ngroup = \"C3\"\nrepeat = 4\napplies_to = [\"petal\"]\n",
                "both",
            ),
            (
                "neither group nor repeat",
                "\n[[symmetry]]\naxis = [0.0, 1.0, 0.0]\napplies_to = [\"petal\"]\n",
                "neither",
            ),
            (
                "a repeat that grows",
                "\n[[symmetry]]\nrepeat = 4\nshrink = 1.5\napplies_to = [\"petal\"]\n",
                "scale",
            ),
            (
                "two blocks claiming one motif",
                "\n[[symmetry]]\ngroup = \"C3\"\napplies_to = [\"petal\"]\n\n\
                 [[symmetry]]\ngroup = \"C4\"\napplies_to = [\"petal\"]\n",
                "two symmetry blocks",
            ),
            (
                "unknown colour",
                "\n[[symmetry]]\ngroup = \"C3\"\ncolor = \"rainbow\"\napplies_to = [\"petal\"]\n",
                "rainbow",
            ),
        ] {
            let Err(err) = load_str(&two_motif_scene(block), "bad") else {
                panic!("{tag} should not load");
            };
            assert!(
                err.contains(expect),
                "{tag}: the error should mention '{expect}', got: {err}"
            );
        }
    }

    /// An unnamed motif is referenced by index, the same escape hatch
    /// `[zoom].map` has — so a symmetry can survive on a transform the author
    /// never named.
    #[test]
    fn an_unnamed_motif_round_trips_by_index() {
        let tmp = std::env::temp_dir().join("fracturize-sym-index.toml");
        let mut scene = super::Scene::blank();
        scene.transforms[0].symmetry = Some(
            crate::symmetry::Symmetry::new(
                crate::symmetry::OrbitKind::Cyclic(3),
                Vec3::Y,
                false,
                crate::symmetry::OrbitColor::Shared,
            )
            .unwrap(),
        );
        let _ = std::fs::remove_file(&tmp);
        scene.save(&tmp).unwrap();
        let after = super::Scene::load(&tmp).unwrap();
        assert_eq!(after.transforms[0].symmetry.as_ref().map(|s| s.order()), Some(3));
        assert!(after.transforms[1].symmetry.is_none());
        let _ = std::fs::remove_file(&tmp);
    }

    /// A post-affine slot has to survive the same round trip the pre-affine
    /// one does. No scene on disk carries one yet, so
    /// `every_scenes_transforms_survive_a_save` proves nothing about it —
    /// this builds the cases by hand.
    #[test]
    fn post_affine_survives_a_save() {
        let cases: [(&str, Mat4); 5] = [
            ("identity", Mat4::IDENTITY),
            (
                "translate",
                Mat4::from_translation(Vec3::new(0.25, -0.5, 0.125)),
            ),
            (
                "rotate 72 about Y",
                Mat4::from_rotation_y(72.0f32.to_radians()),
            ),
            (
                "full trs",
                Mat4::from_scale_rotation_translation(
                    Vec3::new(0.8, 0.5, 0.8),
                    glam::Quat::from_rotation_z(0.7),
                    Vec3::new(0.1, 0.2, 0.3),
                ),
            ),
            // The XYZ-euler pole. `TransformDef::rotation` has a `rotvec`
            // escape hatch for exactly this; `post_rotation` does not, so
            // this is the case that says whether it needs one.
            (
                "quarter turn about Y (euler pole)",
                Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ),
        ];

        let tmp = std::env::temp_dir().join("fracturize-roundtrip-post-affine.toml");
        for (label, post) in cases {
            let mut scene = super::Scene::blank();
            scene.transforms[0].post_affine = post;
            let _ = std::fs::remove_file(&tmp);
            scene.save(&tmp).unwrap();
            let after = super::Scene::load(&tmp).unwrap();

            let diff = post
                .to_cols_array()
                .iter()
                .zip(after.transforms[0].post_affine.to_cols_array().iter())
                .fold(0.0f32, |acc, (x, y)| acc.max((x - y).abs()));
            assert!(
                diff < 1e-3,
                "{}: post_affine moved {:.6}\n  wrote {:?}\n  read  {:?}",
                label,
                diff,
                post.to_cols_array(),
                after.transforms[0].post_affine.to_cols_array(),
            );
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn every_scenes_transforms_survive_a_save() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scenes");
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();

        let tmp = std::env::temp_dir().join("fracturize-roundtrip-transforms.toml");
        for file in &files {
            let name = file.file_stem().unwrap().to_string_lossy().into_owned();
            let before = super::Scene::load(file).unwrap();
            let _ = std::fs::remove_file(&tmp);
            before.save(&tmp).unwrap();
            let after = super::Scene::load(&tmp).unwrap();

            assert_eq!(before.transforms.len(), after.transforms.len(), "{}", name);
            for (i, (a, b)) in
                before.transforms.iter().zip(after.transforms.iter()).enumerate()
            {
                let worst = |x: Mat4, y: Mat4| {
                    x.to_cols_array()
                        .iter()
                        .zip(y.to_cols_array().iter())
                        .fold(0.0f32, |acc, (p, q)| acc.max((p - q).abs()))
                };
                let diff = worst(a.matrix, b.matrix);
                assert!(diff < 1e-3, "{} transform {}: matrix moved {:.6}", name, i, diff);
                // The post-affine slot has to survive too, or a scene built
                // the KIFS way silently loads back as a different attractor.
                let diff = worst(a.post_affine, b.post_affine);
                assert!(diff < 1e-3, "{} transform {}: post_affine moved {:.6}", name, i, diff);
            }
        }
        let _ = std::fs::remove_file(&tmp);
    }

    /// A matrix the degrees chart can't say cleanly still round-trips, by
    /// falling back to the exact form rather than writing a lie.
    #[test]
    fn a_pole_rotation_falls_back_to_rotvec() {
        // A quarter turn about Y is exactly where XYZ euler gives up.
        let rotation = crate::rot::Orientation::from_axis_angle(
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_2,
        );
        let m = crate::rot::Trs {
            scale: glam::Vec3::splat(0.5),
            rotation,
            translation: glam::Vec3::new(0.1, 0.2, 0.3),
        }
        .matrix();

        let trs = crate::rot::Trs::of(m);
        assert!(trs.is_faithful(m), "a clean TRS must decompose");
        assert!(
            trs.rotation.angle_to(rotation) < 1e-4,
            "and recover the rotation itself, whatever the chart says about it"
        );

        // Whichever form gets written, reloading has to land on the same
        // matrix — that is the whole contract.
        let back = crate::rot::Orientation::from_xyz_degrees(trs.rotation.to_xyz_degrees());
        assert!(
            back.angle_to(rotation) < 1e-3,
            "degrees round trip: {:?} -> {:?}",
            trs.rotation.to_xyz_degrees(),
            back
        );
    }

    // === [palette] ========================================================

    /// The minimum scene a palette test needs, as text.
    fn scene_with(palette: &str) -> String {
        format!(
            r#"[meta]
name = "PaletteTest"

[camera]
distance = 3.0

{palette}
[[transform]]
translation = [0.5, 0.5, 0.0]
scale = 0.5
color = [0.9, 0.5, 0.25]

[[transform]]
translation = [-0.5, -0.5, 0.0]
scale = 0.5
color = [0.25, 0.6, 0.9]
"#
        )
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fracturize_palette_{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("scene.toml")
    }

    #[test]
    fn no_palette_table_means_transform_mode() {
        let path = temp("absent");
        std::fs::write(&path, scene_with("")).unwrap();
        let scene = Scene::load(&path).unwrap();
        assert_eq!(scene.color_mode, ColorMode::Transforms);
        assert!(scene.palette.is_none());
        // ...and renders exactly what it always did
        assert_eq!(scene.colormap, generate_colormap(&scene.colors));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_palette_table_selects_palette_mode_by_its_presence() {
        let path = temp("present");
        std::fs::write(&path, scene_with("[palette]\nname = \"ember\"\n\n")).unwrap();
        let scene = Scene::load(&path).unwrap();
        assert_eq!(scene.color_mode, ColorMode::Palette);
        assert_eq!(scene.colormap, crate::palette::library::get("ember").unwrap().to_colormap());
        // The per-transform colours are still there, still meaningful
        assert_eq!(scene.colors.len(), 2);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn enabled_false_keeps_the_palette_but_renders_the_ring() {
        let path = temp("disabled");
        std::fs::write(&path, scene_with("[palette]\nname = \"ocean\"\nenabled = false\n\n")).unwrap();
        let scene = Scene::load(&path).unwrap();
        assert_eq!(scene.color_mode, ColorMode::Transforms);
        assert!(scene.palette.is_some(), "the gradient stays on file");
        assert_eq!(scene.colormap, generate_colormap(&scene.colors));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// The A/B toggle has to survive Ctrl+S, or flipping to `transforms` and
    /// saving would silently delete the gradient (or silently flip back).
    #[test]
    fn the_mode_toggle_survives_a_save() {
        let path = temp("toggle");
        std::fs::write(&path, scene_with("[palette]\nname = \"jade\"\n\n")).unwrap();

        let mut scene = Scene::load(&path).unwrap();
        scene.set_color_mode(ColorMode::Transforms);
        scene.save(&path).unwrap();

        let back = Scene::load(&path).unwrap();
        assert_eq!(back.color_mode, ColorMode::Transforms);
        assert_eq!(back.palette.as_ref().and_then(|p| p.name.clone()).as_deref(), Some("jade"));

        // ...and back again
        let mut scene = back;
        scene.set_color_mode(ColorMode::Palette);
        scene.save(&path).unwrap();
        let back = Scene::load(&path).unwrap();
        assert_eq!(back.color_mode, ColorMode::Palette);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// `scene_with` drops its fragment between `[camera]` and the transforms,
    /// which is the wrong side of the file for a `[meta]` key.
    fn scene_with_mode(mode: &str) -> String {
        scene_with("").replace("name = \"PaletteTest\"", &format!("name = \"PaletteTest\"\ncolor_mode = \"{mode}\""))
    }

    /// Mix has no `[palette]` table to be inferred from and no per-transform
    /// state that distinguishes it, so `meta.color_mode` is the only thing
    /// carrying it across a save. If that key goes missing the scene silently
    /// reverts to `transforms`, which renders — just not what was authored.
    #[test]
    fn mix_mode_survives_a_save() {
        let path = temp("mix_save");
        std::fs::write(&path, scene_with("")).unwrap();

        let mut scene = Scene::load(&path).unwrap();
        assert_eq!(scene.color_mode, ColorMode::Transforms);
        scene.set_color_mode(ColorMode::Mix);
        assert!(scene.palette.is_none(), "mix adopts nothing: it reads the transform colours directly");
        scene.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("color_mode = \"mix\""), "the mode has to be written:\n{text}");

        let back = Scene::load(&path).unwrap();
        assert_eq!(back.color_mode, ColorMode::Mix);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// ...and leaving mix has to remove the key again, or the scene would
    /// reload into a mode it was explicitly switched out of.
    #[test]
    fn leaving_mix_drops_the_key() {
        let path = temp("mix_leave");
        std::fs::write(&path, scene_with_mode("mix")).unwrap();

        let mut scene = Scene::load(&path).unwrap();
        assert_eq!(scene.color_mode, ColorMode::Mix);
        scene.set_color_mode(ColorMode::Transforms);
        scene.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("color_mode"), "the key should be gone:\n{text}");
        assert_eq!(Scene::load(&path).unwrap().color_mode, ColorMode::Transforms);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// An explicit key beats the palette-presence rule in both directions —
    /// otherwise a scene couldn't keep a gradient on file while rendering mix.
    #[test]
    fn an_explicit_mode_outranks_the_palette_table() {
        let path = temp("mix_over_palette");
        std::fs::write(
            &path,
            scene_with("[palette]\nname = \"ember\"\n\n")
                .replace("name = \"PaletteTest\"", "name = \"PaletteTest\"\ncolor_mode = \"mix\""),
        )
        .unwrap();

        let scene = Scene::load(&path).unwrap();
        assert_eq!(scene.color_mode, ColorMode::Mix);
        assert!(scene.palette.is_some(), "the gradient stays on file");
        // Mix ignores the colormap entirely, so it must not be the palette's:
        // switching back to `palette` is what should install that.
        assert_eq!(scene.colormap, generate_colormap(&scene.colors));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// A typo'd mode refuses to load rather than quietly rendering the default
    /// — wrong colours are harder to notice than a failed load.
    #[test]
    fn an_unknown_mode_is_an_error() {
        let path = temp("mix_typo");
        std::fs::write(&path, scene_with_mode("mixx")).unwrap();
        match Scene::load(&path) {
            Ok(_) => panic!("'mixx' should not load"),
            Err(e) => {
                assert!(e.contains("mixx"), "the message should name the bad mode: {e}");
                assert!(e.contains("mix"), "and list the real ones: {e}");
            }
        }
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// The GPU branches on this: only mix writes RGB into the point's spare
    /// bits, and the shader reads those bits instead of the colormap.
    #[test]
    fn only_mix_packs_rgb() {
        assert!(ColorMode::Mix.packs_rgb());
        assert!(!ColorMode::Transforms.packs_rgb());
        assert!(!ColorMode::Palette.packs_rgb());
        for m in ColorMode::ALL {
            assert_eq!(ColorMode::parse(m.name()), Some(m), "{} must parse back", m.name());
        }
        assert_eq!(ColorMode::parse("MIX"), Some(ColorMode::Mix), "names are case-insensitive");
        assert_eq!(ColorMode::parse("rainbow"), None);
    }

    #[test]
    fn authored_stops_round_trip_through_a_save() {
        let path = temp("stops");
        std::fs::write(
            &path,
            scene_with(
                "[palette]\ncyclic = true\ninterpolate = \"oklab\"\nrotate = 0.25\nreverse = true\n\
                 stops = [ { at = 0.0, color = \"#0c040a\" }, { at = 0.4, color = \"#b23818\" }, \
                 { at = 0.75, color = \"#fcde9e\" } ]\n\n",
            ),
        )
        .unwrap();

        let scene = Scene::load(&path).unwrap();
        let before = scene.colormap;
        let p = scene.palette.clone().unwrap();
        assert_eq!(p.stops().unwrap().len(), 3);
        assert_eq!(p.interpolate, crate::palette::Interpolate::Oklab);
        assert!(p.reverse);
        assert!((p.rotate - 0.25).abs() < 1e-6);

        scene.save(&path).unwrap();
        let back = Scene::load(&path).unwrap();
        assert_eq!(back.color_mode, ColorMode::Palette);
        assert_eq!(back.palette.unwrap(), p);
        assert_eq!(back.colormap, before, "the rendered gradient must not drift");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_cosine_palette_round_trips() {
        let path = temp("cosine");
        std::fs::write(
            &path,
            scene_with(
                "[palette]\ncosine = { a = [0.5, 0.5, 0.5], b = [0.5, 0.5, 0.5], \
                 c = [1.0, 1.0, 1.0], d = [0.0, 0.33, 0.67] }\n\n",
            ),
        )
        .unwrap();
        let scene = Scene::load(&path).unwrap();
        let before = scene.colormap;
        scene.save(&path).unwrap();
        let back = Scene::load(&path).unwrap();
        assert_eq!(back.colormap, before);
        assert!(matches!(back.palette.unwrap().body, crate::palette::Body::Cosine(_)));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// Adding a palette to a hand-authored scene must not cost it its
    /// comments — the whole point of the merge path.
    #[test]
    fn adding_a_palette_preserves_comments() {
        let path = temp("comments");
        let src = "# A scene with a story.\n[meta]\nname = \"Told\"  # inline\n\n                   [camera]\ndistance = 3.0\n\n                   # The first map.\n[[transform]]\ntranslation = [0.5, 0.5, 0.0]\n                   scale = 0.5\ncolor = [0.9, 0.5, 0.25]\n";
        std::fs::write(&path, src).unwrap();

        let mut scene = Scene::load(&path).unwrap();
        scene.set_palette(crate::palette::library::get("abyss").unwrap());
        scene.save(&path).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# A scene with a story."), "{out}");
        assert!(out.contains("# The first map."), "{out}");
        assert!(out.contains("# inline"), "{out}");
        assert!(out.contains("[palette]") && out.contains("abyss"), "{out}");
        assert_eq!(Scene::load(&path).unwrap().color_mode, ColorMode::Palette);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// Dropping a palette has to actually remove the table, or the next load
    /// would put it straight back.
    #[test]
    fn clearing_a_palette_removes_the_table() {
        let path = temp("cleared");
        std::fs::write(&path, scene_with("[palette]\nname = \"neon\"\n\n")).unwrap();
        let mut scene = Scene::load(&path).unwrap();
        scene.palette = None;
        scene.color_mode = ColorMode::Transforms;
        scene.save(&path).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("[palette]"), "{out}");
        assert!(Scene::load(&path).unwrap().palette.is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_broken_palette_is_a_load_error_not_a_silent_fallback() {
        let path = temp("broken");
        std::fs::write(&path, scene_with("[palette]\nname = \"no-such-gradient\"\n\n")).unwrap();
        let err = match Scene::load(&path) {
            Err(e) => e,
            Ok(_) => panic!("a palette naming a gradient that doesn't exist must not load"),
        };
        assert!(err.contains("unknown palette"), "{err}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn switching_to_palette_mode_adopts_the_ring_that_was_showing() {
        let mut scene = Scene::blank();
        let ring = scene.colormap;
        scene.set_color_mode(ColorMode::Palette);
        assert!(scene.palette.is_some());
        assert_eq!(scene.colormap, ring, "the toggle must not change what is on screen");
    }

    /// The transform ring depends on the transform count; a palette does not.
    /// This is the defect palette mode exists to escape, so pin both halves.
    #[test]
    fn a_palette_is_independent_of_the_transform_count() {
        let mut scene = Scene::blank();
        let ring_before = scene.colormap;
        scene.colors.push(Vec3::new(0.2, 0.9, 0.4));
        scene.regenerate_colormap();
        assert_ne!(scene.colormap, ring_before, "adding a colour moves the whole ring");

        scene.set_palette(crate::palette::library::get("fern").unwrap());
        let with_palette = scene.colormap;
        scene.colors.push(Vec3::new(0.9, 0.2, 0.4));
        scene.regenerate_colormap();
        assert_eq!(scene.colormap, with_palette, "a palette does not care how many maps there are");
    }
    use super::*;

    #[test]
    fn blank_scene_is_a_usable_starting_point() {
        let s = Scene::blank();
        // Two transforms, because one contracting map converges to a single
        // point and there'd be nothing on screen to work against.
        assert_eq!(s.transforms.len(), 2);
        assert_eq!(s.colors.len(), 2);
        assert_eq!(s.transform_names.len(), 2);
        for t in &s.transforms {
            // Contracting, so the chaos game converges rather than diverging.
            assert!(t.contraction() < 1.0, "contraction {}", t.contraction());
            assert!(t.weight > 0.0);
            // Purely affine: nothing to reverse-engineer out of the inspector.
            assert_eq!(t.variations, TransformSpec::linear_variations());
        }
        assert!(s.camera_path.is_none(), "a blank scene flies the default orbit");
        assert!(s.camera_distance > 0.0);
    }

    /// The loader's rotation convention, pinned as a test: EulerRot::XYZ
    /// composes as Rx * Ry * Rz on column vectors. External generators
    /// (tools/lsystem_to_ifs.py) decompose matrices assuming exactly this.
    #[test]
    fn euler_xyz_is_rx_ry_rz() {
        let (a, b, c) = (0.3f32, -0.7, 1.1);
        let q = glam::Quat::from_euler(glam::EulerRot::XYZ, a, b, c);
        let m = glam::Mat3::from_rotation_x(a)
            * glam::Mat3::from_rotation_y(b)
            * glam::Mat3::from_rotation_z(c);
        let diff = (glam::Mat3::from_quat(q) - m).to_cols_array();
        let max = diff.iter().fold(0.0f32, |acc, v| acc.max(v.abs()));
        assert!(max < 1e-5, "EulerRot::XYZ convention drift: {}", max);
    }

    #[test]
    fn variations_serialize_inline() {
        let scene = {
            let dir = std::env::temp_dir().join("fracturize_inline_test");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("v.toml");
            std::fs::write(
                &path,
                r#"
[meta]
name = "Inline"

[[transform]]
translation = [0.0, 0.0, 0.5]
scale = 0.5
color = [1.0, 0.2, 0.2]
variations = { swirl = 0.35, linear = 0.65 }
"#,
            )
            .unwrap();
            Scene::load(&path).unwrap()
        };
        let dir = std::env::temp_dir().join("fracturize_inline_test");
        let out = dir.join("saved.toml");
        scene.save(&out).unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(
            content.contains("variations = {"),
            "expected inline variations, got:\n{}",
            content
        );
        assert!(!content.contains("[transform.variations]"), "{}", content);
        Scene::load(&out).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn merge_save_preserves_comments() {
        let src = r#"# A scene with a story in its comments.
# This header block must survive saving.

[meta]
name = "Commented"
author = "test"
point_size = 0.0015
point_count = 6_000_000
color_speed = 0.65

[camera]
distance = 3.0
offset = [0.0, 0.0, 0.0]
focus = [0.0, 0.0, 0.0]

# The ocean: automatic processing. This comment matters.
[[transform]]
name = "ocean"
translation = [0.55, 0.1, -0.3]
scale = 0.7
rotation = [8.0, 35.0, 18.0]
color = [0.22, 0.30, 0.52] # cool blue
weight = 2.4
variations = { swirl = 0.3, linear = 0.7 }

# The workspace: small, hot, dense.
[[transform]]
name = "workspace"
translation = [-0.85, 0.05, 0.15]
scale = 0.32
color = [1.0, 0.58, 0.22]
weight = 1.2
"#;
        let dir = std::env::temp_dir().join("fracturize_merge_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commented.toml");
        std::fs::write(&path, src).unwrap();

        let mut scene = Scene::load(&path).unwrap();
        // Edit only transform 1's weight (the probability lever)
        scene.transforms[1].weight = 1.9;
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();

        // Comments survive
        assert!(out.contains("# A scene with a story in its comments."), "{}", out);
        assert!(out.contains("# The ocean: automatic processing. This comment matters."), "{}", out);
        assert!(out.contains("# The workspace: small, hot, dense."), "{}", out);
        assert!(out.contains("# cool blue"), "{}", out);
        // Untouched values keep their exact formatting
        assert!(out.contains("translation = [0.55, 0.1, -0.3]"), "{}", out);
        assert!(out.contains("point_count = 6_000_000"), "{}", out);
        assert!(out.contains("variations = { swirl = 0.3, linear = 0.7 }"), "{}", out);
        // The legacy offset is folded away, and the edit landed
        assert!(!out.contains("offset"), "{}", out);
        assert!(out.contains("weight = 1.9"), "{}", out);
        // And the merged file still loads to the same state
        let reloaded = Scene::load(&path).unwrap();
        assert!((reloaded.transforms[1].weight - 1.9).abs() < 1e-4);
        let diff = (reloaded.transforms[0].matrix - scene.transforms[0].matrix).to_cols_array();
        assert!(diff.iter().all(|v| v.abs() < 1e-4));

        // Add a transform: the array is rebuilt, header comments survive
        scene.transforms.push(scene.transforms[0].clone());
        scene.transform_names.push(None);
        scene.colors.push(Vec3::ONE);
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# A scene with a story in its comments."), "{}", out);
        assert_eq!(Scene::load(&path).unwrap().transforms.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scene_save_load_roundtrip() {
        let src = r#"
[meta]
name = "Roundtrip"
author = "test"
point_size = 0.002
color_speed = 0.4
color_falloff = 0.8
color_contrast = 2.0
fog = 0.35
exposure = 1.8
point_count = 123456

[camera]
focus = [0.0, 1.0, 0.0]
offset = [0.0, 1.5, 0.0]
distance = 3.5
roll = 0.35

[[transform]]
name = "Spine"
translation = [0.1, 0.15, -0.2]
scale = 0.95
rotation = [10.0, 30.0, -45.0]
color = [1.0, 0.0, 0.8]
weight = 3.0
color_speed = 0.1
variations = { spherical = 0.7, linear = 0.3 }

[[transform]]
translation = [0.8, 0.0, 0.0]
scale = 0.3
color = [0.0, 1.0, 1.0]
"#;
        let dir = std::env::temp_dir().join("fracturize_scene_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("src.toml");
        std::fs::write(&src_path, src).unwrap();

        let scene = Scene::load(&src_path).unwrap();
        let saved_path = dir.join("saved.toml");
        scene.save(&saved_path).unwrap();
        let reloaded = Scene::load(&saved_path).unwrap();

        assert_eq!(reloaded.name, scene.name);
        assert_eq!(reloaded.color_falloff, scene.color_falloff);
        assert!((reloaded.haze - 0.35).abs() < 1e-4, "haze {}", reloaded.haze);
        // The splat renderer's primary brightness control. It used to live only
        // on `App`, so tuning it and pressing Ctrl+S wrote nothing and the scene
        // reopened at 1.0 looking wrong — the shipped scenes worked around that
        // in a *comment* saying what to type on the command line.
        assert!((reloaded.exposure - 1.8).abs() < 1e-4, "exposure {}", reloaded.exposure);
        let roll = reloaded.camera_orientation.yaw_pitch_roll().roll.radians();
        assert!((roll - 0.35).abs() < 1e-4, "roll {}", roll);
        // point_count is deliberately *not* round-tripped: it's a render
        // property owned by the Render window and prefs, so saving to a fresh
        // file omits it and the reload falls back to the default. Loading a
        // hand-authored file that still carries one keeps honouring it —
        // which is exactly what `scene` (loaded from `src`) shows.
        assert_eq!(scene.point_count, 123456);
        assert_eq!(reloaded.point_count, DEFAULT_POINT_COUNT);
        assert!(
            !std::fs::read_to_string(&saved_path).unwrap().contains("point_count"),
            "saving must not write point_count into a new scene file"
        );
        assert_eq!(reloaded.transform_names, scene.transform_names);
        assert_eq!(reloaded.transforms.len(), scene.transforms.len());
        for (a, b) in scene.transforms.iter().zip(reloaded.transforms.iter()) {
            let diff = (a.matrix - b.matrix).to_cols_array();
            let max = diff.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(max < 1e-3, "matrix drift {} exceeds tolerance", max);
            assert!((a.weight - b.weight).abs() < 1e-4);
            assert!((a.color_value - b.color_value).abs() < 1e-4);
            assert_eq!(a.explicit_color_speed.is_some(), b.explicit_color_speed.is_some());
            for (wa, wb) in a.variations.iter().zip(b.variations.iter()) {
                assert!((wa - wb).abs() < 1e-4);
            }
        }
        for (ca, cb) in scene.colors.iter().zip(reloaded.colors.iter()) {
            assert!((*ca - *cb).length() < 1e-3);
        }
        assert!((scene.camera_distance - reloaded.camera_distance).abs() < 1e-4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zoom_loop_roundtrips_and_needs_a_zoom_map() {
        let scene_src = |zoom: &str| format!(
            r#"
[meta]
name = "Looper"

[camera]
distance = 3.6
pitch = 1.1
path_zoom_loop = 2
path_seconds = 12.0

[[camera.path]]
yaw = 0.0

{zoom}

[[transform]]
name = "descent"
translation = [0.0, 0.0, 0.0]
scale = 0.6
rotation = [0.0, 34.0, 0.0]
color = [0.3, 0.5, 0.8]
"#
        );

        // Without a [zoom] map there is no symmetry to close under, and
        // pretending otherwise would give a path that claims to loop and
        // doesn't.
        let dir = std::env::temp_dir().join("fracturize_zoom_loop_test");
        std::fs::create_dir_all(&dir).unwrap();
        let bare = dir.join("bare.toml");
        std::fs::write(&bare, scene_src("")).unwrap();
        // Scene has no Debug, so match rather than unwrap_err
        match Scene::load(&bare) {
            Err(e) => assert!(e.contains("[zoom]"), "{}", e),
            Ok(_) => panic!("a zoom loop without a [zoom] map should not load"),
        }

        let path = dir.join("looper.toml");
        std::fs::write(&path, scene_src("[zoom]
map = \"descent\"")).unwrap();
        let scene = Scene::load(&path).unwrap();
        let cp = scene.camera_path.as_ref().expect("path");
        let z = cp.loops.zoom().expect("zoom loop");
        assert_eq!(z.periods, 2);
        // Two periods of a 0.6 map
        assert!((z.scale - 0.36).abs() < 1e-4, "scale {}", z.scale);
        assert_eq!(
            cp.loops.kind(),
            crate::path::LoopKind::Zoom,
            "a zoom loop is not a key-to-key loop"
        );
        // One key is a path when it closes under the symmetry
        assert!(cp.playable(), "a one-key zoom loop should fly");

        // ...and it survives a save
        let out = dir.join("saved.toml");
        scene.save(&out).unwrap();
        let again = Scene::load(&out).unwrap();
        assert_eq!(again.camera_path.unwrap().loops.zoom().unwrap().periods, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zoom_band_settings_roundtrip_and_default_sensibly() {
        let src = |zoom: &str| format!(
            r#"
[meta]
name = "Banded"

[camera]
distance = 3.0
focus = [0.0, 0.0, 0.0]

[[transform]]
name = "descent"
translation = [0.0, 0.0, 0.0]
scale = 0.6
rotation = [0.0, 34.0, 0.0]
color = [0.4, 0.6, 0.9]

[[transform]]
name = "spur"
translation = [0.4, 0.5, 0.1]
scale = 0.45
color = [0.9, 0.5, 0.3]

{zoom}
"#
        );

        let dir = std::env::temp_dir().join("fracturize_zoom_band_test");
        std::fs::create_dir_all(&dir).unwrap();

        // A [zoom] with only a map takes every band setting from ZoomSpec's
        // defaults — including the edge guard, which is on unless a scene
        // says otherwise.
        let bare = dir.join("bare.toml");
        std::fs::write(&bare, src("[zoom]\nmap = \"descent\"")).unwrap();
        let scene = Scene::load(&bare).unwrap();
        let spec = scene.zoom.as_ref().expect("zoom");
        assert_eq!(*spec, crate::renorm::ZoomSpec { map: 0, ..Default::default() });
        assert_eq!(spec.edge_guard, 1.0, "the guard is the default; see DEFAULT_EDGE_GUARD");

        // An authored width reaches the spec, and so does the old key: the
        // guard replaced a deal-side taper called `octave_fade`, and scenes
        // written against that must keep loading.
        let guarded = dir.join("guarded.toml");
        std::fs::write(&guarded, src("[zoom]\nmap = \"descent\"\nedge_guard = 3.0")).unwrap();
        let guarded_spec = Scene::load(&guarded).unwrap().zoom.take().expect("zoom");
        assert_eq!(guarded_spec.edge_guard, 3.0);

        let legacy = dir.join("legacy.toml");
        std::fs::write(&legacy, src("[zoom]\nmap = \"descent\"\noctave_fade = 0.0")).unwrap();
        let legacy_spec = Scene::load(&legacy).unwrap().zoom.take().expect("zoom");
        assert_eq!(legacy_spec.edge_guard, 0.0, "octave_fade must still be read");

        // Authored values survive a load/save/load round trip. The guard is
        // written back out even when it matches the default, so a saved scene
        // states what its band is doing rather than leaving it implied.
        let authored = dir.join("authored.toml");
        std::fs::write(
            &authored,
            src("[zoom]\nmap = \"descent\"\nradius = 6.5\nlevels = 12.0\nedge_guard = 2.5"),
        )
        .unwrap();
        let scene = Scene::load(&authored).unwrap();
        let spec = scene.zoom.as_ref().expect("zoom");
        assert_eq!(spec.edge_guard, 2.5);
        assert_eq!(spec.radius, 6.5);

        let out = dir.join("saved.toml");
        scene.save(&out).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("edge_guard"), "{text}");
        let again = Scene::load(&out).unwrap();
        assert_eq!(again.zoom.as_ref().unwrap(), spec);

        // Saving *over an existing file* is the comment-preserving merge, a
        // different path from the one above — and it used to skip [zoom]
        // entirely, so editing the band in the app and pressing Ctrl+S threw
        // the edits away silently. It also has to migrate the old key rather
        // than leave a file saying two things at once.
        let mut edited = Scene::load(&legacy).unwrap();
        let z = edited.zoom.as_mut().unwrap();
        z.radius = 6.0;
        z.edge_guard = 2.0;
        edited.save(&legacy).unwrap();
        let text = std::fs::read_to_string(&legacy).unwrap();
        assert!(!text.contains("octave_fade"), "the old key must not survive a save: {text}");
        let reloaded = Scene::load(&legacy).unwrap().zoom.take().expect("zoom");
        assert_eq!(reloaded.radius, 6.0, "a band edit must survive Ctrl+S over the file");
        assert_eq!(reloaded.edge_guard, 2.0);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The loop is one key now (`path_loop`), and it has to survive the merge
    /// save that writes over an existing file — which is where the pair of
    /// keys it replaced went wrong. `path_zoom_loop` was never written by the
    /// merge path at all, so turning a zoom loop on and pressing Ctrl+S on a
    /// scene loaded from disk silently dropped it.
    #[test]
    fn the_loop_survives_a_merge_save() {
        let src = r#"
[meta]
name = "Looper"

[camera]
distance = 3.6
pitch = 1.1
path_zoom_loop = 2      # legacy spelling

[[camera.path]]
yaw = 0.0

[zoom]
map = "descent"

[[transform]]
name = "descent"
translation = [0.0, 0.0, 0.0]
scale = 0.6
rotation = [0.0, 34.0, 0.0]
color = [0.3, 0.5, 0.8]
"#;
        let dir = std::env::temp_dir().join("fracturize_loop_merge_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("looper.toml");
        std::fs::write(&path, src).unwrap();

        // Legacy key loads...
        let scene = Scene::load(&path).unwrap();
        assert_eq!(
            scene.camera_path.as_ref().unwrap().loops.zoom().unwrap().periods,
            2
        );

        // ...and saving over the file migrates it rather than dropping it.
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains(r#"path_loop = "zoom""#), "{}", out);
        assert!(out.contains("path_zoom_periods = 2"), "{}", out);
        assert!(!out.contains("path_zoom_loop"), "{}", out);
        assert_eq!(
            Scene::load(&path).unwrap().camera_path.unwrap().loops.zoom().unwrap().periods,
            2
        );

        // A ping-pong is the same round trip, through a mode the pair of flags
        // could not have expressed at all.
        let pong = dir.join("pong.toml");
        std::fs::write(
            &pong,
            src.replace("path_zoom_loop = 2      # legacy spelling", "path_loop = \"pingpong\"")
                .replace("[[camera.path]]\nyaw = 0.0", "[[camera.path]]\nyaw = 0.0\n\n[[camera.path]]\nyaw = 1.0"),
        )
        .unwrap();
        let scene = Scene::load(&pong).unwrap();
        assert_eq!(scene.camera_path.as_ref().unwrap().loops, crate::path::Loop::PingPong);
        scene.save(&pong).unwrap();
        assert!(std::fs::read_to_string(&pong).unwrap().contains(r#"path_loop = "pingpong""#));
        assert_eq!(
            Scene::load(&pong).unwrap().camera_path.unwrap().loops,
            crate::path::Loop::PingPong
        );

        // An unknown loop is a load error, not a silent fall-back to `once`:
        // a scene that quietly played the wrong way would be worse than one
        // that refused to load.
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, src.replace("path_zoom_loop = 2", "path_loop = \"bounce\"")).unwrap();
        match Scene::load(&bad) {
            Err(e) => assert!(e.contains("path_loop"), "{}", e),
            Ok(_) => panic!("an unknown path_loop should not load"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_route_says_what_a_winding_cannot_and_is_checked_against_its_keys() {
        let src = r#"
[meta]
name = "Route"

[camera]
focus = [0.0, 0.0, 0.0]
distance = 3.0
yaw = 0.0
pitch = 0.0

# two full turns of pitch, ending where it began
[[camera.path]]
route = [12.566371, 0.0, 0.0]

[[camera.path]]

[[transform]]
translation = [0.0, 0.0, 0.5]
scale = 0.5
color = [1.0, 0.2, 0.2]
"#;
        let dir = std::env::temp_dir().join("fracturize_route_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("route.toml");
        std::fs::write(&path, src).unwrap();

        let scene = Scene::load(&path).unwrap();
        let p = scene.camera_path.as_ref().expect("path parsed");
        let crate::path::Route::Exact(t) = p.route(0) else {
            panic!("a stated route must load as an exact one, not {:?}", p.route(0));
        };
        assert!((t.magnitude() - 4.0 * std::f32::consts::PI).abs() < 1e-3, "{:?}", t);
        assert!(t.as_rotation_vector().x > 0.0, "about X, as written");

        // It has to survive a save over its own file: silently dropping it
        // would straighten the shot on the next Ctrl+S.
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("route = ["), "{}", out);
        let reloaded = Scene::load(&path).unwrap();
        assert_eq!(reloaded.camera_path.as_ref().unwrap().routes, p.routes);

        // A hand-written 6.28 for a full turn is a route someone means: it
        // misses by a fifth of a degree, and loads.
        let near = dir.join("near.toml");
        std::fs::write(&near, src.replace("12.566371", "6.28")).unwrap();
        assert!(Scene::load(&near).is_ok(), "a rounded full turn must still load");

        // One that lands somewhere else entirely is refused, naming the key
        // and the miss. This is the property `turns` had for free — a stored
        // displacement can contradict its endpoints — bought back at the door.
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, src.replace("12.566371, 0.0, 0.0", "12.566371, 0.0, 3.0")).unwrap();
        match Scene::load(&bad) {
            Err(e) => {
                assert!(e.contains("key 1"), "{}", e);
                assert!(e.contains("misses by"), "{}", e);
            }
            Ok(_) => panic!("a route that misses the key it runs to should not load"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn camera_path_roundtrip() {
        let src = r#"
[meta]
name = "Pathed"

[camera]
focus = [0.0, 0.2, 0.0]
distance = 3.0
yaw = 0.5
pitch = 0.3
path_closed = true
path_seconds = 8.0

# approach from afar
[[camera.path]]
yaw = 0.0
distance = 6.0

[[camera.path]]
yaw = 3.14
pitch = 0.6
focus = [0.0, 0.5, 0.0]

[[camera.path]]
yaw = 6.28
distance = 1.5

[[transform]]
translation = [0.0, 0.0, 0.5]
scale = 0.5
color = [1.0, 0.2, 0.2]
"#;
        let dir = std::env::temp_dir().join("fracturize_path_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pathed.toml");
        std::fs::write(&path, src).unwrap();

        let scene = Scene::load(&path).unwrap();
        let p = scene.camera_path.as_ref().expect("path parsed");
        assert_eq!(p.keys.len(), 3);
        assert_eq!(p.loops, crate::path::Loop::Closed);
        assert_eq!(p.seconds, Some(8.0));
        // Omitted fields inherit the base camera
        assert!(
            (p.keys[0].orientation.yaw_pitch_roll().pitch.radians() - 0.3).abs() < 1e-5,
            "pitch inherits base"
        );
        assert!((p.keys[0].focus - Vec3::new(0.0, 0.2, 0.0)).length() < 1e-6);
        assert!((p.keys[1].distance - 3.0).abs() < 1e-6, "distance inherits base");
        assert!((p.keys[1].focus - Vec3::new(0.0, 0.5, 0.0)).length() < 1e-6);

        // Merge-save keeps the path (and its comment); values survive a reload
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# approach from afar"), "{}", out);
        assert!(out.contains("[[camera.path]]"), "{}", out);
        // The legacy `path_closed` is migrated to the one key that names the
        // loop, and removed — leaving it would let a stale flag contradict the
        // key that now decides. Same treatment camera `offset` gets.
        assert!(out.contains(r#"path_loop = "closed""#), "{}", out);
        assert!(!out.contains("path_closed"), "{}", out);
        let reloaded = Scene::load(&path).unwrap();
        let q = reloaded.camera_path.as_ref().unwrap();
        assert_eq!(q.keys.len(), 3);
        assert_eq!(q.loops, crate::path::Loop::Closed);
        for (a, b) in p.keys.iter().zip(&q.keys) {
            assert!(a.orientation.angle_to(b.orientation) < 1e-3);
            assert!((a.distance - b.distance).abs() < 1e-3);
            assert!((a.focus - b.focus).length() < 1e-3);
        }
        // Routes survive too, or a corkscrew quietly unwinds on save.
        assert_eq!(p.routes, q.routes, "routes must round-trip");

        // Fresh save (new file) round-trips too
        let fresh_path = dir.join("fresh.toml");
        scene.save(&fresh_path).unwrap();
        let fresh = Scene::load(&fresh_path).unwrap();
        assert_eq!(fresh.camera_path.as_ref().unwrap().keys.len(), 3);

        // A scene without a path stays path-free after save
        let no_path = {
            let p2 = dir.join("nopath.toml");
            std::fs::write(&p2, "[meta]\nname = \"n\"\n\n[[transform]]\ntranslation = [0.0, 0.0, 0.5]\nscale = 0.5\ncolor = [1.0, 1.0, 1.0]\n").unwrap();
            let s = Scene::load(&p2).unwrap();
            s.save(&p2).unwrap();
            Scene::load(&p2).unwrap()
        };
        assert!(no_path.camera_path.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn per_axis_scale_roundtrip() {
        let src = r#"
[meta]
name = "Anisotropic"

[[transform]]
translation = [0.0, 0.5, 0.0]
scale = [0.05, 0.6, 0.05]
rotation = [0.0, 20.0, 5.0]
color = [0.4, 0.8, 0.3]

[[transform]]
translation = [0.3, 0.0, 0.0]
scale = 0.7
color = [0.9, 0.5, 0.2]
"#;
        let dir = std::env::temp_dir().join("fracturize_scale_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("src.toml");
        std::fs::write(&src_path, src).unwrap();

        let scene = Scene::load(&src_path).unwrap();
        let (s0, _, _) = scene.transforms[0].matrix.to_scale_rotation_translation();
        assert!((s0 - Vec3::new(0.05, 0.6, 0.05)).length() < 1e-4);

        // Round-trip through both save paths: in-place merge (existing file)
        // and fresh serialization (new file)
        for path in [&src_path, &dir.join("fresh.toml")] {
            scene.save(path).unwrap();
            let reloaded = Scene::load(path).unwrap();
            for (a, b) in scene.transforms.iter().zip(reloaded.transforms.iter()) {
                let diff = (a.matrix - b.matrix).to_cols_array();
                let max = diff.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(max < 1e-3, "matrix drift {} exceeds tolerance", max);
            }
        }
        // Uniform transform must stay scalar in the file
        let saved = std::fs::read_to_string(&src_path).unwrap();
        assert!(saved.contains("scale = 0.7"), "uniform scale kept scalar:\n{saved}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// The `transforms` colour source: N per-transform colours spread evenly
/// around a cyclic gradient.
///
/// Thin wrapper over [`Palette::from_transform_colors`], which is where the
/// logic (and the test pinning it byte-for-byte to the version this replaced)
/// now lives.
pub fn generate_colormap(colors: &[Vec3]) -> Colormap {
    Palette::from_transform_colors(colors).to_colormap()
}
