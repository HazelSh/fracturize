//! Stage 2 of colouring: the 1-D → RGB map, broken out so it can be swapped.
//!
//! The renderer has always been a palette renderer — `chaos.wgsl` writes an
//! 8-bit index, never a colour, and both point shaders resolve it against a
//! 256-entry storage buffer. What changes here is only *what fills those 256
//! entries*, which is why palette mode needs no GPU work at all.
//!
//! Two sources fill them (see [`crate::scene::ColorMode`]):
//!
//! - `transforms` — the ring built from the per-transform RGBs, which is what
//!   the renderer did before palettes existed. [`Palette::from_transform_colors`]
//!   reproduces `generate_colormap` exactly: N stops evenly spaced around a
//!   cyclic gradient. Note the defect this inherits — adding a transform moves
//!   every other transform's colour, because the spacing is `k/N`.
//! - `palette` — an independent gradient that doesn't care how many transforms
//!   the scene has. This is the Apophysis model.
//!
//! **Colours here are linear RGB**, like `Scene::background` and everything
//! else handed to the GPU: the surface is sRGB, so the value in a stop is the
//! value the fragment shader emits before the hardware encodes it. Anything
//! that *displays* a palette (the `--info` swatch, the GUI strip) therefore
//! has to encode it first — see [`to_srgb8`]. Anything that *imports* one from
//! a file written in display space has to decode it — see [`srgb8_to_linear`].

pub mod axes;
pub mod import;
pub mod library;
pub mod random;
pub mod spec;

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// 256-colour gradient, in the layout the GPU storage buffer wants.
/// Linear RGB in `.rgb`; alpha is unused and always 1.
pub type Colormap = [[f32; 4]; 256];

/// A single control point: a position around the gradient and its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    /// Position in 0..1. Stops are kept sorted by this.
    pub at: f32,
    /// Linear RGB
    pub color: Vec3,
}

/// Iñigo Quílez's cosine palette: `c(t) = a + b·cos(2π(c·t + d))`.
///
/// Twelve numbers that are always smooth and always cyclic, which makes this
/// both a compact authored form and the generator that produces the fewest
/// unusable rolls (see [`random`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cosine {
    /// Bias (the mean colour)
    pub a: Vec3,
    /// Amplitude
    pub b: Vec3,
    /// Frequency, in cycles across the gradient
    pub c: Vec3,
    /// Phase
    pub d: Vec3,
}

impl Cosine {
    pub fn sample(&self, t: f32) -> Vec3 {
        let ang = (self.c * t + self.d) * std::f32::consts::TAU;
        self.a + self.b * Vec3::new(ang.x.cos(), ang.y.cos(), ang.z.cos())
    }
}

/// How a palette's colours are defined.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Control points, interpolated. The canonical form: it's what an editor
    /// manipulates, it's compact and diffable in TOML, and it subsumes the
    /// transform ring exactly.
    Stops(Vec<Stop>),
    /// 256 explicit entries, for palettes imported verbatim from flam3 /
    /// Apophysis. Kept as-is rather than fitted to stops so an import is
    /// lossless and round-trips.
    Entries(Box<[Vec3; 256]>),
    /// Procedural, twelve numbers (see [`Cosine`]).
    Cosine(Cosine),
}

/// Interpolation space between control points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Interpolate {
    /// Componentwise lerp in linear RGB. The default, and what the transform
    /// ring has always done, so switching a scene to palette mode with the
    /// same colours changes nothing.
    #[default]
    Rgb,
    /// Lerp in Oklab. Perceptually even, and no muddy grey midpoint between
    /// complementary stops — the failure mode `rgb` has.
    Oklab,
}

impl Interpolate {
    pub const ALL: [Interpolate; 2] = [Interpolate::Rgb, Interpolate::Oklab];

    pub fn name(&self) -> &'static str {
        match self {
            Interpolate::Rgb => "rgb",
            Interpolate::Oklab => "oklab",
        }
    }

    pub fn parse(s: &str) -> Option<Interpolate> {
        Interpolate::ALL.into_iter().find(|i| i.name().eq_ignore_ascii_case(s))
    }
}

/// A gradient: colours, how to read between them, and two per-scene tweaks
/// that let a library palette be adjusted without forking it.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// Library name this came from, if any. Purely provenance: the resolved
    /// colours are always carried in `body`, so a scene never depends on the
    /// library still containing the same thing under that name.
    pub name: Option<String>,
    pub body: Body,
    /// Whether index 255 wraps back to index 0. The shader's lookup masks with
    /// `& 0xFFu` and `color_contrast` stretches cyclically, so cyclic is both
    /// the default and the case the rest of the renderer assumes. Imported
    /// flam3 palettes are authored for a clamped 0..255 index and may have a
    /// seam; this lets them say so.
    pub cyclic: bool,
    pub interpolate: Interpolate,
    /// Shift the whole gradient along the index, 0..1.
    pub rotate: f32,
    /// Reverse the gradient. Applied before `rotate`.
    pub reverse: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            name: None,
            body: Body::Stops(Vec::new()),
            cyclic: true,
            interpolate: Interpolate::default(),
            rotate: 0.0,
            reverse: false,
        }
    }
}

impl Palette {
    /// The gradient the renderer used before palettes existed: N per-transform
    /// colours spread evenly around a cyclic ring.
    ///
    /// Exactly reproduces the old `generate_colormap`, including its two
    /// degenerate cases (no colours = white, one colour = flat). Stop `k` sits
    /// at `k/N`, which is the source of the "adding a transform recolours
    /// every other one" defect — reproduced deliberately, because this is the
    /// mode that exists to keep working the way it always has.
    pub fn from_transform_colors(colors: &[Vec3]) -> Self {
        let stops = colors
            .iter()
            .enumerate()
            .map(|(i, &color)| Stop { at: i as f32 / colors.len() as f32, color })
            .collect();
        Self { body: Body::Stops(stops), ..Self::default() }
    }

    /// A palette from stops, cyclic and RGB-interpolated.
    pub fn from_stops(stops: Vec<Stop>) -> Self {
        let mut p = Self { body: Body::Stops(stops), ..Self::default() };
        p.sort_stops();
        p
    }

    /// A palette from 256 explicit entries (an import).
    pub fn from_entries(entries: [Vec3; 256]) -> Self {
        Self { body: Body::Entries(Box::new(entries)), ..Self::default() }
    }

    pub fn from_cosine(cosine: Cosine) -> Self {
        Self { body: Body::Cosine(cosine), ..Self::default() }
    }

    /// The stops, if this palette has any (editing only applies to `Stops`).
    pub fn stops(&self) -> Option<&[Stop]> {
        match &self.body {
            Body::Stops(s) => Some(s),
            _ => None,
        }
    }

    pub fn stops_mut(&mut self) -> Option<&mut Vec<Stop>> {
        match &mut self.body {
            Body::Stops(s) => Some(s),
            _ => None,
        }
    }

    /// Re-sort stops by position, after an edit moved one past another.
    pub fn sort_stops(&mut self) {
        if let Body::Stops(s) = &mut self.body {
            s.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
        }
    }

    /// Move control point `idx` to position `at`, and say where it ended up.
    ///
    /// Stops are kept sorted (the sampler binary-searches them), so moving one
    /// past its neighbour renumbers both. The returned index is the moved
    /// stop's *new* index — the caller is mid-drag and needs to keep hold of
    /// the stop it grabbed, not of whichever one inherited the number.
    pub fn move_stop(&mut self, idx: usize, at: f32) -> usize {
        let Some(stops) = self.stops_mut() else { return idx };
        if idx >= stops.len() {
            return idx;
        }
        stops[idx].at = at.clamp(0.0, 0.999);
        // Sort a tagged copy rather than searching for the new position by
        // value afterwards: two stops may legitimately share a position (a
        // hard edge in the gradient), and a value search would then hand back
        // whichever sorted first instead of the one being dragged.
        let mut tagged: Vec<(Stop, bool)> =
            stops.iter().enumerate().map(|(i, &s)| (s, i == idx)).collect();
        tagged.sort_by(|a, b| a.0.at.partial_cmp(&b.0.at).unwrap_or(std::cmp::Ordering::Equal));
        let landed = tagged.iter().position(|&(_, tag)| tag).unwrap_or(idx);
        *stops = tagged.into_iter().map(|(s, _)| s).collect();
        landed
    }

    /// Freeze a procedural or imported palette into editable stops, so the
    /// GUI's handles can act on it. `n` stops sampled evenly.
    pub fn to_stops(&self, n: usize) -> Self {
        let n = n.max(2);
        let stops = (0..n)
            .map(|i| {
                let at = i as f32 / n as f32;
                Stop { at, color: self.sample(at) }
            })
            .collect();
        Self { body: Body::Stops(stops), rotate: 0.0, reverse: false, ..self.clone() }
    }

    /// Short human-readable description, for `--info` and the GUI.
    pub fn describe(&self) -> String {
        let body = match &self.body {
            Body::Stops(s) => format!("{} stops", s.len()),
            Body::Entries(_) => "256 entries".to_string(),
            Body::Cosine(_) => "cosine".to_string(),
        };
        let mut out = match &self.name {
            Some(n) => format!("{} ({})", n, body),
            None => body,
        };
        if !self.cyclic {
            out.push_str(", clamped");
        }
        if self.interpolate == Interpolate::Oklab {
            out.push_str(", oklab");
        }
        if self.rotate != 0.0 {
            out.push_str(&format!(", rotate {:.3}", self.rotate));
        }
        if self.reverse {
            out.push_str(", reversed");
        }
        out
    }

    /// Colour at position `t`, with `reverse` and `rotate` applied.
    pub fn sample(&self, t: f32) -> Vec3 {
        let u = if self.reverse { self.rotate - t } else { t - self.rotate };
        // Rotating a clamped gradient can only push colour off one end, so
        // wrap where wrapping is meaningful and clamp where it isn't.
        let u = if self.cyclic { u.rem_euclid(1.0) } else { u.clamp(0.0, 1.0) };
        self.sample_base(u)
    }

    /// Colour at `t` in the underlying gradient, ignoring rotate/reverse.
    fn sample_base(&self, t: f32) -> Vec3 {
        match &self.body {
            Body::Cosine(c) => c.sample(t).clamp(Vec3::ZERO, Vec3::ONE),
            Body::Entries(e) => {
                let i = (t * 256.0) as isize;
                let i = if self.cyclic {
                    i.rem_euclid(256) as usize
                } else {
                    i.clamp(0, 255) as usize
                };
                e[i]
            }
            Body::Stops(stops) => self.sample_stops(stops, t),
        }
    }

    fn sample_stops(&self, stops: &[Stop], t: f32) -> Vec3 {
        match stops.len() {
            // No colours at all: white, so a scene with an empty palette still
            // renders something rather than going black and looking broken.
            0 => Vec3::ONE,
            1 => stops[0].color,
            n => {
                // Index of the last stop at or before t. Stops are sorted, so
                // this is a partition point.
                let i = stops.partition_point(|s| s.at <= t);
                let (a, b, span_start, span_len) = if i == 0 {
                    if self.cyclic {
                        // Below the first stop: the wrap segment, entered from
                        // its far side.
                        let last = &stops[n - 1];
                        (last, &stops[0], last.at - 1.0, stops[0].at + 1.0 - last.at)
                    } else {
                        return stops[0].color;
                    }
                } else if i == n {
                    if self.cyclic {
                        let last = &stops[n - 1];
                        (last, &stops[0], last.at, stops[0].at + 1.0 - last.at)
                    } else {
                        return stops[n - 1].color;
                    }
                } else {
                    (&stops[i - 1], &stops[i], stops[i - 1].at, stops[i].at - stops[i - 1].at)
                };
                // Coincident stops are a hard edge, not a divide by zero.
                let local = if span_len > 1e-6 { (t - span_start) / span_len } else { 0.0 };
                mix(a.color, b.color, local.clamp(0.0, 1.0), self.interpolate)
            }
        }
    }

    /// Resolve to the 256 entries the GPU reads.
    ///
    /// Sampled at `i/256`, not `(i+0.5)/256`: that is what the transform ring
    /// has always done, and matching it is what makes Phase 0 a pure
    /// refactor. The half-entry offset against the shader's own `+0.5` read is
    /// 1/512 of the gradient and invisible.
    pub fn to_colormap(&self) -> Colormap {
        let mut map = [[0.0f32; 4]; 256];
        for (i, entry) in map.iter_mut().enumerate() {
            let c = self.sample(i as f32 / 256.0);
            *entry = [c.x, c.y, c.z, 1.0];
        }
        map
    }

    /// Mean luminance and its peak-to-trough swing across the gradient.
    ///
    /// The renderer has no lights, so the palette *is* the shading: a gradient
    /// with no luminance swing renders flat however pretty its hues are. Used
    /// to gate random rolls ([`random`]) and reported by `--info`.
    pub fn luminance_profile(&self) -> (f32, f32) {
        let l: Vec<f32> = (0..64).map(|i| luminance(self.sample(i as f32 / 64.0))).collect();
        let mean = l.iter().sum::<f32>() / l.len() as f32;
        let min = l.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = l.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        (mean, max - min)
    }
}

/// Rec. 709 relative luminance of a linear RGB colour.
pub fn luminance(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

/// Interpolate two linear-RGB colours in the requested space.
pub fn mix(a: Vec3, b: Vec3, t: f32, space: Interpolate) -> Vec3 {
    match space {
        Interpolate::Rgb => a.lerp(b, t),
        Interpolate::Oklab => oklab_to_linear(linear_to_oklab(a).lerp(linear_to_oklab(b), t)),
    }
}

// === Colour spaces ========================================================

/// Linear RGB → Oklab (Björn Ottosson's matrices).
pub fn linear_to_oklab(c: Vec3) -> Vec3 {
    let l = 0.4122214708 * c.x + 0.5363325363 * c.y + 0.0514459929 * c.z;
    let m = 0.2119034982 * c.x + 0.6806995451 * c.y + 0.1073969566 * c.z;
    let s = 0.0883024619 * c.x + 0.2817188376 * c.y + 0.6299787005 * c.z;
    let (l, m, s) = (cbrt(l), cbrt(m), cbrt(s));
    Vec3::new(
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    )
}

/// Chroma (colourfulness) of a linear-RGB colour: the length of Oklab's
/// `(a, b)`. Note this is deliberately *not* `linear_to_oklab(c).truncate()`,
/// which would take `(L, a)` and read lightness as colour.
pub fn chroma(c: Vec3) -> f32 {
    let lab = linear_to_oklab(c);
    (lab.y * lab.y + lab.z * lab.z).sqrt()
}

/// Oklab → linear RGB. Clamped: interpolating between two in-gamut colours
/// can leave the sRGB gamut on the way, and the GPU wants 0..1.
pub fn oklab_to_linear(c: Vec3) -> Vec3 {
    let l = c.x + 0.3963377774 * c.y + 0.2158037573 * c.z;
    let m = c.x - 0.1055613458 * c.y - 0.0638541728 * c.z;
    let s = c.x - 0.0894841775 * c.y - 1.2914855480 * c.z;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    Vec3::new(
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    )
    .clamp(Vec3::ZERO, Vec3::ONE)
}

/// Signed cube root — Oklab's LMS stage needs it to survive the small
/// negatives that out-of-gamut inputs produce.
fn cbrt(v: f32) -> f32 {
    v.signum() * v.abs().powf(1.0 / 3.0)
}

/// One linear channel → its sRGB-encoded 0..255 byte.
pub fn linear_to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// One sRGB 0..255 byte → its linear value.
pub fn srgb8_to_linear(v: u8) -> f32 {
    let s = v as f32 / 255.0;
    if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}

/// A linear-RGB colour as the sRGB bytes a display (or a terminal, or egui)
/// wants. Everything that *shows* a palette goes through this; the GPU path
/// never does, because the surface encodes for itself.
pub fn to_srgb8(c: Vec3) -> [u8; 3] {
    [linear_to_srgb8(c.x), linear_to_srgb8(c.y), linear_to_srgb8(c.z)]
}

/// sRGB bytes → linear RGB. The import direction.
pub fn from_srgb8(c: [u8; 3]) -> Vec3 {
    Vec3::new(srgb8_to_linear(c[0]), srgb8_to_linear(c[1]), srgb8_to_linear(c[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The old `generate_colormap`, verbatim, as the oracle Phase 0 has to
    /// match. Deleted from `scene.rs`; kept here so the refactor stays honest.
    fn legacy_colormap(colors: &[Vec3]) -> Colormap {
        let mut colormap = [[0.0f32; 4]; 256];
        if colors.is_empty() {
            return [[1.0, 1.0, 1.0, 1.0]; 256];
        }
        if colors.len() == 1 {
            let c = colors[0];
            return [[c.x, c.y, c.z, 1.0]; 256];
        }
        let n = colors.len();
        for (i, entry) in colormap.iter_mut().enumerate() {
            let t = i as f32 / 256.0;
            let scaled = t * n as f32;
            let idx0 = scaled.floor() as usize;
            let idx1 = (idx0 + 1) % colors.len();
            let local_t = scaled - idx0 as f32;
            let c = colors[idx0] * (1.0 - local_t) + colors[idx1] * local_t;
            *entry = [c.x, c.y, c.z, 1.0];
        }
        colormap
    }

    #[test]
    fn transform_ring_matches_the_old_generator() {
        let palettes: Vec<Vec<Vec3>> = vec![
            vec![],
            vec![Vec3::new(0.9, 0.5, 0.25)],
            vec![Vec3::new(0.9, 0.5, 0.25), Vec3::new(0.25, 0.6, 0.9)],
            vec![Vec3::X, Vec3::Y, Vec3::Z],
            vec![Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, 1.0, 0.2)],
            (0..7).map(|i| Vec3::splat(i as f32 / 7.0)).collect(),
        ];
        for colors in palettes {
            let want = legacy_colormap(&colors);
            let got = Palette::from_transform_colors(&colors).to_colormap();
            for i in 0..256 {
                for ch in 0..4 {
                    assert!(
                        (want[i][ch] - got[i][ch]).abs() < 1e-6,
                        "{} colours, entry {} channel {}: {} vs {}",
                        colors.len(), i, ch, want[i][ch], got[i][ch]
                    );
                }
            }
        }
    }

    /// The bug this exists to stop coming back: dragging a control point past
    /// its neighbour renumbers both, and if the caller keeps using the index
    /// it grabbed, the drag silently transfers to the other stop and hauls
    /// that one along too. `move_stop` has to say where the stop it moved
    /// ended up.
    #[test]
    fn moving_a_stop_past_its_neighbour_reports_the_new_index() {
        let mut p = Palette::from_stops(vec![
            Stop { at: 0.0, color: Vec3::X },
            Stop { at: 0.25, color: Vec3::Y },
            Stop { at: 0.5, color: Vec3::Z },
            Stop { at: 0.75, color: Vec3::ONE },
        ]);

        // Drag the green stop (index 1) past two neighbours
        let landed = p.move_stop(1, 0.6);
        assert_eq!(landed, 2, "0.6 sits between 0.5 and 0.75");
        assert_eq!(p.stops().unwrap()[landed].color, Vec3::Y, "it must be the same stop");

        // ...and back again, from its new index
        let landed = p.move_stop(landed, 0.1);
        assert_eq!(landed, 1);
        assert_eq!(p.stops().unwrap()[landed].color, Vec3::Y);

        // Stops stay sorted throughout — the sampler binary-searches them
        let ats: Vec<f32> = p.stops().unwrap().iter().map(|s| s.at).collect();
        assert!(ats.windows(2).all(|w| w[0] <= w[1]), "{ats:?}");
    }

    #[test]
    fn moving_a_stop_onto_another_still_tracks_the_right_one() {
        let mut p = Palette::from_stops(vec![
            Stop { at: 0.0, color: Vec3::X },
            Stop { at: 0.5, color: Vec3::Y },
            Stop { at: 0.9, color: Vec3::Z },
        ]);
        // Exactly onto a neighbour: a value search would pick the wrong one.
        let landed = p.move_stop(2, 0.5);
        assert_eq!(p.stops().unwrap()[landed].color, Vec3::Z);
        assert_eq!(p.stops().unwrap().len(), 3, "coincident stops are legal, not merged");
    }

    #[test]
    fn move_stop_is_a_no_op_on_a_palette_without_control_points() {
        let mut p = Palette::from_cosine(Cosine {
            a: Vec3::splat(0.5),
            b: Vec3::splat(0.5),
            c: Vec3::ONE,
            d: Vec3::ZERO,
        });
        let before = p.clone();
        assert_eq!(p.move_stop(0, 0.5), 0);
        assert_eq!(p, before);
    }

    #[test]
    fn stops_land_on_their_own_colour() {
        let p = Palette::from_stops(vec![
            Stop { at: 0.0, color: Vec3::X },
            Stop { at: 0.5, color: Vec3::Y },
        ]);
        assert!((p.sample(0.0) - Vec3::X).length() < 1e-6);
        assert!((p.sample(0.5) - Vec3::Y).length() < 1e-6);
        // Midway through each half
        assert!((p.sample(0.25) - Vec3::new(0.5, 0.5, 0.0)).length() < 1e-6);
        assert!((p.sample(0.75) - Vec3::new(0.5, 0.5, 0.0)).length() < 1e-6);
    }

    #[test]
    fn clamped_palettes_hold_their_ends() {
        let mut p = Palette::from_stops(vec![
            Stop { at: 0.2, color: Vec3::X },
            Stop { at: 0.8, color: Vec3::Z },
        ]);
        p.cyclic = false;
        assert_eq!(p.sample(0.0), Vec3::X);
        assert_eq!(p.sample(0.1), Vec3::X);
        assert_eq!(p.sample(1.0), Vec3::Z);
        // ...and the cyclic version does not
        p.cyclic = true;
        assert!((p.sample(0.0) - Vec3::new(0.5, 0.0, 0.5)).length() < 1e-6);
    }

    #[test]
    fn rotate_shifts_the_gradient_forward() {
        let base = Palette::from_transform_colors(&[Vec3::X, Vec3::Y, Vec3::Z]);
        let mut rotated = base.clone();
        rotated.rotate = 1.0 / 3.0;
        for i in 0..64 {
            let t = i as f32 / 64.0;
            let want = base.sample((t - 1.0 / 3.0).rem_euclid(1.0));
            assert!((rotated.sample(t) - want).length() < 1e-5, "at t = {t}");
        }
        // A full turn is the identity
        let mut full = base.clone();
        full.rotate = 1.0;
        assert!((full.sample(0.3) - base.sample(0.3)).length() < 1e-5);
    }

    #[test]
    fn reverse_mirrors_the_gradient() {
        let base = Palette::from_transform_colors(&[Vec3::X, Vec3::Y, Vec3::Z]);
        let mut rev = base.clone();
        rev.reverse = true;
        for i in 0..64 {
            let t = i as f32 / 64.0;
            assert!((rev.sample(t) - base.sample((-t).rem_euclid(1.0))).length() < 1e-5);
        }
    }

    #[test]
    fn oklab_round_trips() {
        for c in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::ONE, Vec3::ZERO, Vec3::new(0.3, 0.6, 0.1)] {
            let back = oklab_to_linear(linear_to_oklab(c));
            assert!((back - c).length() < 1e-4, "{c} round-tripped to {back}");
        }
    }

    #[test]
    fn oklab_midpoint_beats_rgb_between_complements() {
        // Blue→yellow through linear RGB passes through a desaturated grey;
        // through Oklab it stays a colour. Chroma at the midpoint is the test.
        let (a, b) = (Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(chroma(mix(a, b, 0.5, Interpolate::Rgb)), 0.0, "linear RGB goes through grey");
        assert!(chroma(mix(a, b, 0.5, Interpolate::Oklab)) > 0.05);
    }

    #[test]
    fn srgb_bytes_round_trip() {
        for v in 0..=255u8 {
            assert_eq!(linear_to_srgb8(srgb8_to_linear(v)), v);
        }
    }

    #[test]
    fn cosine_palettes_stay_in_gamut_and_wrap() {
        let c = Cosine {
            a: Vec3::splat(0.5),
            b: Vec3::splat(0.5),
            c: Vec3::ONE,
            d: Vec3::new(0.0, 0.33, 0.67),
        };
        let p = Palette::from_cosine(c);
        for i in 0..256 {
            let s = p.sample(i as f32 / 256.0);
            assert!(s.min_element() >= 0.0 && s.max_element() <= 1.0);
        }
        assert!((p.sample(0.0) - p.sample(1.0)).length() < 1e-5, "integer frequency must wrap");
    }

    #[test]
    fn to_stops_approximates_the_source() {
        let p = Palette::from_cosine(Cosine {
            a: Vec3::splat(0.5),
            b: Vec3::splat(0.4),
            c: Vec3::ONE,
            d: Vec3::new(0.0, 0.1, 0.2),
        });
        let frozen = p.to_stops(32);
        assert_eq!(frozen.stops().map(|s| s.len()), Some(32));
        for i in 0..64 {
            let t = i as f32 / 64.0;
            assert!((frozen.sample(t) - p.sample(t)).length() < 0.05, "at t = {t}");
        }
    }
}
