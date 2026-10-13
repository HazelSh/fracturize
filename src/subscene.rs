//! Subscenes: several IFS attractors composed into one scene.
//!
//! A composite is **not** one bigger IFS. The union of two map sets has an
//! attractor that mixes them — a walker that takes a fern map and then a
//! lattice map lands somewhere neither scene contains — so concatenating the
//! transform lists would draw neither picture. What a composite wants is the
//! *union of the attractors*, and the chaos game gives that for free if each
//! walker stays inside one subscene's maps for its whole life.
//!
//! So a subscene is a **layer** of the one chaos pass:
//!
//! * every transform carries the layer it belongs to (`TransformSpec::layer`,
//!   0 for the scene's own maps, `k` for `subscenes[k - 1]`);
//! * the 16,384 walkers are dealt out between the visible layers in
//!   proportion to their `share`, and a walker only ever draws maps from its
//!   own layer. Walkers write a fixed stride of the point ring, so **the point
//!   budget divides exactly as the walkers do** — a share is a fraction of the
//!   buffer, not a hint;
//! * a layer's attractor is built in its own space and carried into the
//!   scene's by its `placement` — a similarity (translate, rotate, *uniform*
//!   scale) applied to each point as it is written, never to the walker. That
//!   is what lets a subscene be moved, turned and resized without re-deriving
//!   one of its maps, and why the scale is uniform: a similarity conjugates a
//!   TRS map into a TRS map, so every transform inside a placed subscene is
//!   still something the file format can write and the gizmos can drag.
//!
//! Colour is per layer too (each subscene keeps its own colour mode, palette,
//! speeds and contrast), and that needs one more trick: a point stores an 8-bit
//! colormap index, which cannot also say which layer's colormap it means. So
//! in a composite the chaos pass *resolves* the colour as it writes — looking
//! the index up in that layer's colormap, contrast-stretched — and stores packed
//! RGB, exactly what `mix` mode already stores. Every renderer then draws a
//! composite as a mix-mode scene and needs no change at all. The cost is that a
//! colour edit in a composite re-forms the cloud (like a transform edit does)
//! instead of recolouring it in place.
//!
//! This module holds the data and the pure arithmetic; `scene.rs` loads and
//! saves it, `gpu/points/compute.rs` uploads it.

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::palette::spec::PaletteDef;
use crate::palette::{Colormap, Palette};
use crate::scene::{ColorMode, SymmetryDef, TransformDef};

/// Most layers a scene can hold: the scene's own maps plus fifteen subscenes.
///
/// The layer table rides in a uniform, so it has a fixed size; 16 x 96 bytes is
/// nothing. Past a handful a composite is a different kind of scene anyway.
pub const MAX_LAYERS: usize = 16;

/// Where a subscene's attractor sits in the scene: a similarity.
///
/// Uniform scale on purpose (see the module docs): conjugating a TRS map by a
/// similarity gives a TRS map, so the transforms inside keep decomposing, and
/// a gizmo drag on one of them can be carried back into the subscene's own
/// space without introducing shear the file cannot write.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub translation: Vec3,
    pub rotation: crate::rot::Orientation,
    pub scale: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Placement {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: crate::rot::Orientation::IDENTITY,
        scale: 1.0,
    };

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(self.scale),
            self.rotation.as_quat(),
            self.translation,
        )
    }

    /// The nearest placement to an arbitrary matrix — what a gizmo drag hands
    /// back. The scale is the geometric mean of the three axes, so a drag that
    /// stretched one axis still lands on a similarity rather than a shear.
    pub fn from_matrix(m: Mat4) -> Self {
        let trs = crate::rot::Trs::of(m);
        let s = trs.scale.abs();
        let scale = (s.x * s.y * s.z).cbrt().max(1e-4);
        Self { translation: trs.translation, rotation: trs.rotation, scale }
    }
}

/// A layer's colour settings. The scene's own layer keeps these on `Scene`
/// itself (they predate subscenes, and everything reads them there); each
/// subscene carries its own copy of the same five things plus the resolved
/// colormap.
#[derive(Clone)]
pub struct LayerColour {
    pub color_mode: ColorMode,
    pub palette: Option<Palette>,
    pub color_speed: f32,
    pub color_falloff: f32,
    pub color_contrast: f32,
    /// Resolved from the above and this layer's transform colours; rebuilt by
    /// `Scene::regenerate_colormap`.
    pub colormap: Colormap,
}

/// What a file-backed subscene loaded as, kept so a save can tell whether it
/// has been edited since. Unedited, it stays a `file =` reference; edited, it
/// is written inline (with `source =` saying where it came from), because the
/// referenced file belongs to whoever authored it and is never written to.
#[derive(Clone, PartialEq)]
pub struct FileBase {
    /// The subscene's maps and symmetry blocks, serialized. Comparing text is
    /// what makes "unchanged" mean "would save identically".
    pub maps_toml: String,
    pub color_mode: ColorMode,
    pub color_speed: f32,
    pub color_falloff: f32,
    pub color_contrast: f32,
    pub palette_toml: Option<String>,
}

#[derive(Clone)]
pub struct Subscene {
    pub name: String,
    /// Credit for the maps, when they came from someone else's scene.
    pub author: Option<String>,
    /// `file = "..."` as written, relative to the composing scene's directory.
    /// While set (and the maps unedited) the subscene is a live reference.
    pub file: Option<String>,
    /// Where `file` actually is, resolved when it was loaded. A save rewrites
    /// `file` relative to wherever the composite is being written, so "Save
    /// as" into another directory keeps the reference pointing at the same
    /// scene rather than at a path that no longer resolves.
    pub file_abs: Option<std::path::PathBuf>,
    /// What the file loaded as; see [`FileBase`].
    pub base: Option<FileBase>,
    /// Provenance of an inline subscene that was once imported. Informational.
    pub source: Option<String>,
    pub visible: bool,
    /// Relative weight of this layer's claim on the point budget.
    pub share: f32,
    pub placement: Placement,
    pub colour: LayerColour,
}

fn default_one() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

fn is_zero3(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

/// A `[[subscene]]` block.
///
/// Either `file` names another scene whose maps (and colour) it borrows, or
/// the block carries its own `[[subscene.transform]]`s — never both. Everything
/// that places the subscene in *this* scene (`share`, `visible`, `translation`,
/// `rotation`, `scale`) belongs to the block either way, and the four colour
/// keys and `[subscene.palette]` override whatever the file said.
#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SubsceneDef {
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub visible: bool,
    #[serde(default = "default_one", skip_serializing_if = "is_one")]
    pub share: f64,
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub translation: [f64; 3],
    /// Extrinsic XYZ degrees, like a transform's.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub rotation: [f64; 3],
    /// The exact rotation, where degrees cannot say it. Wins over `rotation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotvec: Option<[f64; 3]>,
    /// Uniform. See [`Placement`] for why it cannot be per-axis.
    #[serde(default = "default_one", skip_serializing_if = "is_one")]
    pub scale: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_speed: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_falloff: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_contrast: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<PaletteDef>,
    #[serde(default, rename = "symmetry", skip_serializing_if = "Vec::is_empty")]
    pub symmetries: Vec<SymmetryDef>,
    #[serde(default, rename = "transform", skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<TransformDef>,
    /// An inline subscene's xaos as one matrix (`[subscene.xaos]`); see
    /// `crate::scene::XaosMatrixDef`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xaos: Option<crate::scene::XaosMatrixDef>,
}

impl Placement {
    pub fn from_def(def: &SubsceneDef) -> Result<Self, String> {
        let rotation = match def.rotvec {
            Some(v) => crate::rot::Turn::from_rotation_vector(Vec3::from(v.map(|x| x as f32))).exp(),
            None => crate::rot::Orientation::from_xyz_degrees(def.rotation.map(|v| v as f32)),
        };
        if !(def.scale > 0.0) {
            return Err(format!(
                "scale = {} — a subscene's scale must be positive (it is uniform; \
                 to mirror one, mirror its maps)",
                def.scale
            ));
        }
        Ok(Self {
            translation: Vec3::from(def.translation.map(|v| v as f32)),
            rotation,
            scale: def.scale as f32,
        })
    }

    /// Write this placement into a def: degrees where they reproduce the
    /// rotation, a rotation vector where they don't — the transform rule.
    pub fn write_into(&self, def: &mut SubsceneDef) {
        let tidy = |v: f32| (v as f64 * 10_000.0).round() / 10_000.0 + 0.0;
        def.translation = self.translation.to_array().map(tidy);
        def.scale = tidy(self.scale);
        let degrees = self.rotation.to_xyz_degrees();
        let chartable =
            crate::rot::Orientation::from_xyz_degrees(degrees).angle_to(self.rotation) < 1e-4;
        if chartable {
            def.rotation = degrees.map(tidy);
            def.rotvec = None;
        } else {
            def.rotation = [0.0; 3];
            def.rotvec = Some(
                crate::rot::Orientation::IDENTITY
                    .shortest_turn_to(self.rotation)
                    .as_rotation_vector()
                    .to_array()
                    .map(|x| (x as f64 * 1e6).round() / 1e6 + 0.0),
            );
        }
    }
}

/// Deal `walkers` out between layers in proportion to `shares`.
///
/// A share of zero (or less) gets nothing — that is how a hidden layer, or one
/// whose maps are all disabled, drops out and hands its points to the rest.
/// Any positive share gets at least one walker, so a tiny share is thin rather
/// than silently absent. Largest-remainder rounding, so the counts always sum
/// to exactly `walkers` when anything is eligible, and to zero when nothing is.
pub fn allocate_walkers(shares: &[f32], walkers: u32) -> Vec<u32> {
    let eligible: Vec<usize> =
        (0..shares.len()).filter(|&i| shares[i] > 0.0 && shares[i].is_finite()).collect();
    let mut out = vec![0u32; shares.len()];
    if eligible.is_empty() || walkers == 0 {
        return out;
    }
    let total: f64 = eligible.iter().map(|&i| shares[i] as f64).sum();
    let mut remainders: Vec<(usize, f64)> = Vec::with_capacity(eligible.len());
    let mut dealt = 0u32;
    for &i in &eligible {
        let exact = walkers as f64 * shares[i] as f64 / total;
        let whole = exact.floor() as u32;
        out[i] = whole;
        dealt += whole;
        remainders.push((i, exact - whole as f64));
    }
    // Largest remainder first; ties to the earlier layer, so it is deterministic.
    remainders.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0))
    });
    for (i, _) in remainders.into_iter().take((walkers - dealt) as usize) {
        out[i] += 1;
    }
    // Then the floor: a positive share that rounded to nothing takes one
    // walker from whichever layer has the most.
    for &i in &eligible {
        if out[i] == 0 {
            let richest = (0..out.len()).max_by_key(|&j| out[j]).expect("non-empty");
            if out[richest] > 1 {
                out[richest] -= 1;
                out[i] = 1;
            }
        }
    }
    out
}

/// `target` relative to the directory `from`, both made absolute first — the
/// spelling a `file =` reference should have in a scene saved in `from`.
/// Falls back to the absolute path when there is no relative one (another
/// drive, or a path that can't be made absolute).
pub fn relative_path(target: &std::path::Path, from: &std::path::Path) -> String {
    use std::path::{Component, PathBuf};
    let abs = |p: &std::path::Path| -> Option<PathBuf> {
        let p = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().ok()?.join(p) };
        // Lexically normalised: `..` resolved against what precedes it.
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c),
            }
        }
        Some(out)
    };
    let (Some(t), Some(f)) = (abs(target), abs(from)) else {
        return target.to_string_lossy().into_owned();
    };
    let tc: Vec<_> = t.components().collect();
    let fc: Vec<_> = f.components().collect();
    let common = tc.iter().zip(&fc).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return t.to_string_lossy().into_owned();
    }
    let mut rel = PathBuf::new();
    for _ in common..fc.len() {
        rel.push("..");
    }
    for c in &tc[common..] {
        rel.push(c.as_os_str());
    }
    rel.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn references_are_rebased_relative_to_where_they_are_written() {
        use std::path::Path;
        assert_eq!(relative_path(Path::new("/a/scenes/fern.toml"), Path::new("/a/scenes")), "fern.toml");
        assert_eq!(relative_path(Path::new("/a/scenes/fern.toml"), Path::new("/a/scenes/mine")), "../fern.toml");
        assert_eq!(relative_path(Path::new("/a/scenes/x/../fern.toml"), Path::new("/tmp")), "../a/scenes/fern.toml");
    }

    use super::*;

    #[test]
    fn walkers_divide_in_proportion_and_sum_exactly() {
        let a = allocate_walkers(&[1.0, 3.0], 16384);
        assert_eq!(a, vec![4096, 12288]);
        let b = allocate_walkers(&[1.0, 1.0, 1.0], 16384);
        assert_eq!(b.iter().sum::<u32>(), 16384);
        assert!(b.iter().all(|&n| n == 5461 || n == 5462));
    }

    #[test]
    fn a_hidden_layer_gets_nothing_and_gives_its_share_away() {
        assert_eq!(allocate_walkers(&[1.0, 0.0, 1.0], 100), vec![50, 0, 50]);
    }

    #[test]
    fn a_tiny_share_is_thin_rather_than_absent() {
        let a = allocate_walkers(&[1.0, 1e-9], 16384);
        assert_eq!(a[1], 1);
        assert_eq!(a.iter().sum::<u32>(), 16384);
    }

    #[test]
    fn nothing_eligible_deals_nothing() {
        assert_eq!(allocate_walkers(&[0.0, 0.0], 64), vec![0, 0]);
    }

    #[test]
    fn a_placement_round_trips_through_its_matrix() {
        let p = Placement {
            translation: Vec3::new(1.0, -2.0, 0.5),
            rotation: crate::rot::Orientation::from_xyz_degrees([10.0, 20.0, 30.0]),
            scale: 0.6,
        };
        let q = Placement::from_matrix(p.matrix());
        assert!((q.translation - p.translation).length() < 1e-5);
        assert!((q.scale - p.scale).abs() < 1e-5);
        assert!(q.rotation.angle_to(p.rotation) < 1e-4);
    }

    /// The property the uniform-scale rule exists for: a TRS map conjugated
    /// by a placement is still TRS, so it decomposes and saves faithfully.
    #[test]
    fn a_placement_keeps_maps_decomposable() {
        let p = Placement {
            translation: Vec3::new(0.3, 0.1, -0.2),
            rotation: crate::rot::Orientation::from_xyz_degrees([0.0, 40.0, 15.0]),
            scale: 2.5,
        };
        let map = Mat4::from_scale_rotation_translation(
            Vec3::new(0.5, 0.2, 0.7),
            glam::Quat::from_rotation_x(0.4),
            Vec3::new(0.1, 0.2, 0.3),
        );
        let world = p.matrix() * map;
        let trs = crate::rot::Trs::of(world);
        assert!(trs.is_faithful(world));
        let back = p.matrix().inverse() * world;
        assert!(back.abs_diff_eq(map, 1e-5));
    }
}
