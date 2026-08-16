//! Reference image → the point cloud the fit aims at.
//!
//! A port of `tools/image_to_target.py`, which is the prototype this was
//! designed in (LIKENESS-PLAN.md slice 0). The finding that shaped it is that
//! *how* you turn pixels into density is an artistic choice and not a
//! preprocessing detail — the same reference gives three unrelated targets —
//! so the density map stays a flag rather than becoming a hidden default.
//!
//! Two colour spaces, deliberately. Everything that decides *density* —
//! luminance, gradients, the saliency percentiles — runs on sRGB values,
//! which is where the prototype's constants were tuned and is the space in
//! which "how different are these two colours" roughly means what it says.
//! The colour carried on each sampled point is linear, because that is what a
//! scene's per-transform `color` is, and the fit matches point colours
//! directly against those.

use glam::{Vec2, Vec3};
use rand::Rng;

use super::Cloud;

/// Fractional art box of a standard Grand Archive card frame. Good enough to
/// batch a folder; override per card where the layout differs (tokens run
/// lower, full-art cards have no box at all).
pub const ART_BOX: [f32; 4] = [0.050, 0.122, 0.950, 0.660];

/// How pixels become density. Not a preprocessing detail — see the module
/// docs, and §4 of LIKENESS-PLAN.md.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum Density {
    /// Subject vs background, keyed on value and saturation. A solid body;
    /// the fit chases silhouette.
    Matte,
    /// Bright pixels are dense. Glows dominate and the body hollows out; the
    /// fit chases light rather than matter.
    Luminance,
    /// Gradient magnitude — a blueprint of the subject. Outline and internal
    /// seams both survive, which is what makes a powercell a powercell.
    Edges,
}

/// How the subject is separated from the setting.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum Weight {
    /// Unlike the border, finely detailed, near centre. A weight, never a
    /// mask — see [`saliency`].
    Saliency,
    /// Take the whole frame. The right answer for character and full-art
    /// cards, where the subject *is* the picture and nothing needs separating.
    None,
}

pub struct Opts {
    pub density: Density,
    pub weight: Weight,
    /// Fractional crop `[l, t, r, b]` of the source, or none for the whole
    /// image.
    pub crop: Option<[f32; 4]>,
    pub points: usize,
    /// Density contrast; >1 concentrates on dense regions.
    pub gamma: f32,
    /// Longest side the analysis runs at. The maps are all low-frequency
    /// relative to the art, so a full-resolution scan buys nothing but time.
    pub max_dim: usize,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            density: Density::Edges,
            weight: Weight::Saliency,
            crop: None,
            points: 60_000,
            gamma: 1.6,
            max_dim: 720,
        }
    }
}

// ---------------------------------------------------------------- grids

/// A single-channel image. Row-major, `w` wide.
#[derive(Clone)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl Grid {
    fn new(w: usize, h: usize) -> Self {
        Self { w, h, v: vec![0.0; w * h] }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }

    fn map(&self, f: impl Fn(f32) -> f32) -> Grid {
        Grid { w: self.w, h: self.h, v: self.v.iter().copied().map(f).collect() }
    }

    fn mul(&self, other: &Grid) -> Grid {
        Grid {
            w: self.w,
            h: self.h,
            v: self.v.iter().zip(&other.v).map(|(a, b)| a * b).collect(),
        }
    }

    /// Rescale so `lo`→0 and `hi`→1, clamped. The percentile stretch that
    /// every one of these maps ends with: the absolute range of a gradient
    /// magnitude or a histogram distance means nothing, the spread does.
    fn stretch(&self, lo_pct: f32, hi_pct: f32) -> Grid {
        let lo = percentile(&self.v, lo_pct);
        let hi = percentile(&self.v, hi_pct);
        let span = (hi - lo).max(1e-9);
        self.map(|x| ((x - lo) / span).clamp(0.0, 1.0))
    }
}

/// Separable Gaussian blur, reflecting at the edges.
///
/// Reflection rather than zero-padding because every caller here is measuring
/// *contrast against the neighbourhood*, and a zero border invents the
/// strongest contrast in the image right where the art box already has a
/// painted edge to be confused with.
fn gaussian(g: &Grid, sigma: f32) -> Grid {
    if sigma <= 0.0 {
        return g.clone();
    }
    let r = (sigma * 3.0).ceil().max(1.0) as isize;
    let kernel: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let norm: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.into_iter().map(|k| k / norm).collect();

    let mut tmp = Grid::new(g.w, g.h);
    for y in 0..g.h {
        for x in 0..g.w {
            let mut acc = 0.0;
            for (k, w) in kernel.iter().enumerate() {
                let sx = reflect(x as isize + k as isize - r, g.w);
                acc += w * g.at(sx, y);
            }
            tmp.v[y * g.w + x] = acc;
        }
    }
    let mut out = Grid::new(g.w, g.h);
    for y in 0..g.h {
        for x in 0..g.w {
            let mut acc = 0.0;
            for (k, w) in kernel.iter().enumerate() {
                let sy = reflect(y as isize + k as isize - r, g.h);
                acc += w * tmp.at(x, sy);
            }
            out.v[y * g.w + x] = acc;
        }
    }
    out
}

/// Separable box filter of odd width `k`, reflecting at the edges.
fn box_filter(g: &Grid, k: usize) -> Grid {
    let k = k.max(1) | 1;
    let r = (k / 2) as isize;
    let inv = 1.0 / k as f32;

    let mut tmp = Grid::new(g.w, g.h);
    for y in 0..g.h {
        for x in 0..g.w {
            let mut acc = 0.0;
            for d in -r..=r {
                acc += g.at(reflect(x as isize + d, g.w), y);
            }
            tmp.v[y * g.w + x] = acc * inv;
        }
    }
    let mut out = Grid::new(g.w, g.h);
    for y in 0..g.h {
        for x in 0..g.w {
            let mut acc = 0.0;
            for d in -r..=r {
                acc += tmp.at(x, reflect(y as isize + d, g.h));
            }
            out.v[y * g.w + x] = acc * inv;
        }
    }
    out
}

fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let mut i = i;
    if n == 1 {
        return 0;
    }
    while i < 0 || i >= n {
        if i < 0 {
            i = -i - 1;
        }
        if i >= n {
            i = 2 * n - i - 1;
        }
    }
    i as usize
}

/// Central-difference gradient magnitude, one-sided at the borders — the same
/// convention `numpy.gradient` uses, so the prototype's percentiles transfer.
fn gradient_magnitude(g: &Grid) -> Grid {
    let mut out = Grid::new(g.w, g.h);
    for y in 0..g.h {
        for x in 0..g.w {
            let gx = if g.w < 2 {
                0.0
            } else if x == 0 {
                g.at(1, y) - g.at(0, y)
            } else if x == g.w - 1 {
                g.at(g.w - 1, y) - g.at(g.w - 2, y)
            } else {
                (g.at(x + 1, y) - g.at(x - 1, y)) * 0.5
            };
            let gy = if g.h < 2 {
                0.0
            } else if y == 0 {
                g.at(x, 1) - g.at(x, 0)
            } else if y == g.h - 1 {
                g.at(x, g.h - 1) - g.at(x, g.h - 2)
            } else {
                (g.at(x, y + 1) - g.at(x, y - 1)) * 0.5
            };
            out.v[y * g.w + x] = gx.hypot(gy);
        }
    }
    out
}

/// `p`th percentile, `p` in 0..100. Copies and sorts; every caller does this
/// a handful of times on one image, not per pixel.
fn percentile(v: &[f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s: Vec<f32> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((s.len() - 1) as f32 * p / 100.0).round() as usize;
    s[idx.min(s.len() - 1)]
}

// ------------------------------------------------------------- the maps

/// The reference, in both spaces at once: sRGB for the density maps, linear
/// for the colour the points carry.
pub struct Reference {
    pub w: usize,
    pub h: usize,
    pub srgb: Vec<Vec3>,
    pub linear: Vec<Vec3>,
}

impl Reference {
    fn luma(&self) -> Grid {
        Grid {
            w: self.w,
            h: self.h,
            v: self.srgb.iter().map(|c| c.dot(Vec3::new(0.2126, 0.7152, 0.0722))).collect(),
        }
    }
}

/// Subject vs background, keyed on value and saturation.
///
/// Workshop-lit card art is a bright subject on a dark, desaturated ground, so
/// value alone nearly does it; saturation rescues coloured-but-dim parts of
/// the subject (brass in shadow) from the floor.
fn matte(r: &Reference) -> Grid {
    let key = Grid {
        w: r.w,
        h: r.h,
        v: r
            .srgb
            .iter()
            .map(|c| {
                let v = c.max_element();
                let s = (v - c.min_element()) / v.max(1e-6);
                v.max(0.6 * v + 0.4 * s * v.max(0.0).sqrt())
            })
            .collect(),
    };
    gaussian(&key, 2.0).stretch(45.0, 97.0)
}

fn edges(r: &Reference) -> Grid {
    let m = gradient_magnitude(&gaussian(&r.luma(), 1.5));
    let hi = percentile(&m.v, 99.0).max(1e-6);
    m.map(|x| (x / hi).clamp(0.0, 1.0))
}

/// How unlike the frame's border colours each pixel is.
///
/// Card art bleeds its setting out to the edges of the art box; the subject
/// rarely touches all four. So a colour model built from a border ring is a
/// background model, and distance from it is a subject signal — one that does
/// not care whether the subject is bright, which brightness keying does and
/// which is wrong for half these cards.
fn border_distinctiveness(r: &Reference) -> Grid {
    const BINS: usize = 6;
    let ring = ((r.w.min(r.h) as f32 * 0.07) as usize).max(1);

    let mut hist = vec![0.0f32; BINS * BINS * BINS];
    let mut counted = 0usize;
    let bin = |c: Vec3| {
        let q = |v: f32| ((v * BINS as f32) as usize).min(BINS - 1);
        q(c.x) * BINS * BINS + q(c.y) * BINS + q(c.z)
    };
    for y in 0..r.h {
        for x in 0..r.w {
            let on_ring = x < ring || y < ring || x + ring >= r.w || y + ring >= r.h;
            if on_ring {
                hist[bin(r.srgb[y * r.w + x])] += 1.0;
                counted += 1;
            }
        }
    }
    if counted == 0 {
        return Grid { w: r.w, h: r.h, v: vec![1.0; r.w * r.h] };
    }

    // Blur the histogram so a colour one bin away from a common background
    // colour is still recognised as background. A 6³ grid is coarse enough
    // that hard bin edges would otherwise show up as banding in the weight.
    let blurred = blur_histogram(&hist, BINS);
    let peak = blurred.iter().copied().fold(0.0f32, f32::max).max(1e-9);

    Grid {
        w: r.w,
        h: r.h,
        v: r.srgb.iter().map(|c| 1.0 - blurred[bin(*c)] / peak).collect(),
    }
}

/// A 3×3×3 tent blur over the colour histogram, separably.
fn blur_histogram(hist: &[f32], bins: usize) -> Vec<f32> {
    let idx = |a: usize, b: usize, c: usize| a * bins * bins + b * bins + c;
    let mut src = hist.to_vec();
    let mut dst = vec![0.0; hist.len()];
    for axis in 0..3 {
        for a in 0..bins {
            for b in 0..bins {
                for c in 0..bins {
                    let mut acc = 0.0;
                    for d in -1isize..=1 {
                        let shift = |v: usize| (v as isize + d).clamp(0, bins as isize - 1) as usize;
                        let w = if d == 0 { 0.5 } else { 0.25 };
                        acc += w * match axis {
                            0 => src[idx(shift(a), b, c)],
                            1 => src[idx(a, shift(b), c)],
                            _ => src[idx(a, b, shift(c))],
                        };
                    }
                    dst[idx(a, b, c)] = acc;
                }
            }
        }
        std::mem::swap(&mut src, &mut dst);
    }
    src
}

/// Local high-frequency energy.
///
/// Illustrators render the subject crisply and let the setting go soft —
/// depth of field, atmospheric haze, looser brushwork. That difference
/// survives even when the background is the brighter thing, which is what
/// rescues a small sword against a bright starfield.
fn detail_density(r: &Reference) -> Grid {
    let energy = gradient_magnitude(&r.luma());
    let k = ((r.w.min(r.h) as f32 * 0.06) as usize).max(3) | 1;
    let local = box_filter(&energy, k);
    let hi = percentile(&local.v, 97.0).max(1e-9);
    local.map(|x| (x / hi).clamp(0.0, 1.0))
}

fn center_prior(w: usize, h: usize) -> Grid {
    const SIGMA: f32 = 0.42;
    let mut g = Grid::new(w, h);
    for y in 0..h {
        let ny = if h > 1 { y as f32 / (h - 1) as f32 * 2.0 - 1.0 } else { 0.0 };
        for x in 0..w {
            let nx = if w > 1 { x as f32 / (w - 1) as f32 * 2.0 - 1.0 } else { 0.0 };
            g.v[y * w + x] = (-(nx * nx + ny * ny) / (2.0 * SIGMA * SIGMA)).exp();
        }
    }
    g
}

/// A soft subject weight: unlike the border, finely detailed, near centre.
///
/// Deliberately a weight and not a mask, and that is the important half.
/// **A wrong mask deletes the subject; a wrong weight only lets some scenery
/// in, and scenery at low density reads as atmosphere rather than as error.**
/// Nothing downstream wants a binary decision — the fit consumes a density —
/// so there is no reason to make one.
///
/// The predecessor was brightness keying (largest bright connected blob). It
/// works on the Powercell and fails on most other card art, which does not
/// reliably light its subject brighter than its setting. Deleted rather than
/// fixed; see LIKENESS-PLAN.md §5.
pub fn saliency(r: &Reference) -> Grid {
    let border = border_distinctiveness(r);
    let detail = detail_density(r);
    let centre = center_prior(r.w, r.h);

    let mut s = Grid::new(r.w, r.h);
    for i in 0..s.v.len() {
        s.v[i] = border.v[i] * detail.v[i].powf(0.7) * centre.v[i].powf(0.6);
    }
    let blurred = gaussian(&s, (r.w.min(r.h) as f32 * 0.03).max(1.0));
    blurred.stretch(30.0, 99.0)
}

/// Fade the outermost band to zero.
///
/// The art box has a painted border, and a border is a very strong edge —
/// left in, the fit spends maps drawing a rectangle around the subject. No
/// crop is precise enough to avoid this on every card, so taper instead.
fn margin_taper(w: usize, h: usize) -> Grid {
    const MARGIN: f32 = 0.025;
    let ramp = |n: usize| {
        let k = ((n as f32 * MARGIN) as usize).max(1);
        let mut r = vec![1.0f32; n];
        for i in 0..k.min(n) {
            let fade = (i as f32 / k as f32).sqrt();
            r[i] = r[i].min(fade);
            r[n - 1 - i] = r[n - 1 - i].min(fade);
        }
        r
    };
    let rx = ramp(w);
    let ry = ramp(h);
    let mut g = Grid::new(w, h);
    for y in 0..h {
        for x in 0..w {
            g.v[y * w + x] = rx[x] * ry[y];
        }
    }
    g
}

// ------------------------------------------------------------- sampling

/// Everything the fit needs from a reference, plus the intermediates so a
/// contact strip can show what the solver is actually aiming at.
pub struct Target {
    pub cloud: Cloud,
    pub reference: Reference,
    pub weight: Grid,
    pub density: Grid,
    /// Fraction of the frame the subject weight covers, for the log line.
    pub coverage: f32,
}

pub fn build(path: &str, opts: &Opts, rng: &mut impl Rng) -> Result<Target, String> {
    let reference = load(path, opts)?;

    let density = match opts.density {
        Density::Matte => matte(&reference),
        Density::Luminance => reference.luma(),
        Density::Edges => edges(&reference),
    };
    let weight = match opts.weight {
        Weight::Saliency => saliency(&reference),
        Weight::None => Grid {
            w: reference.w,
            h: reference.h,
            v: vec![1.0; reference.w * reference.h],
        },
    };

    let field = density
        .mul(&weight)
        .mul(&margin_taper(reference.w, reference.h));
    let cloud = sample(&field, &reference, opts, rng)?;
    let coverage = weight.v.iter().filter(|w| **w > 0.5).count() as f32 / weight.v.len() as f32;

    Ok(Target { cloud, reference, weight, density: field, coverage })
}

fn load(path: &str, opts: &Opts) -> Result<Reference, String> {
    let im = image::open(path).map_err(|e| format!("Failed to read {}: {}", path, e))?;
    let (w, h) = (im.width(), im.height());

    let im = match opts.crop {
        Some([l, t, r, b]) => {
            let px = |f: f32, n: u32| (f * n as f32).round().clamp(0.0, n as f32) as u32;
            let (x0, y0) = (px(l, w), px(t, h));
            let (x1, y1) = (px(r, w), px(b, h));
            if x1 <= x0 || y1 <= y0 {
                return Err(format!("crop [{l},{t},{r},{b}] is empty"));
            }
            im.crop_imm(x0, y0, x1 - x0, y1 - y0)
        }
        None => im,
    };

    let longest = im.width().max(im.height());
    let im = if longest as usize > opts.max_dim {
        let k = opts.max_dim as f32 / longest as f32;
        im.resize(
            ((im.width() as f32 * k) as u32).max(1),
            ((im.height() as f32 * k) as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        im
    };

    let rgb = im.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut srgb = Vec::with_capacity(w * h);
    let mut linear = Vec::with_capacity(w * h);
    for p in rgb.pixels() {
        srgb.push(Vec3::new(
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
        ));
        linear.push(crate::palette::from_srgb8([p[0], p[1], p[2]]));
    }
    Ok(Reference { w, h, srgb, linear })
}

/// Importance-sample `opts.points` points from the density field.
///
/// Jittered within each source pixel so the cloud does not inherit the
/// reference's raster — a fit against a visibly gridded target learns the
/// grid — and normalised the same way the attractor is, so the loss compares
/// two clouds in one convention and nothing about overall placement leaks
/// into it.
fn sample(field: &Grid, r: &Reference, opts: &Opts, rng: &mut impl Rng) -> Result<Cloud, String> {
    let mut cdf = Vec::with_capacity(field.v.len());
    let mut acc = 0.0f64;
    for v in &field.v {
        acc += (v.max(0.0) as f64).powf(opts.gamma as f64);
        cdf.push(acc);
    }
    if acc <= 0.0 {
        return Err("density map is empty — nothing to sample".to_string());
    }

    let mut pos = Vec::with_capacity(opts.points);
    let mut col = Vec::with_capacity(opts.points);
    for _ in 0..opts.points {
        let u = rng.r#gen::<f64>() * acc;
        let i = cdf.partition_point(|c| *c < u).min(cdf.len() - 1);
        let (px, py) = (i % field.w, i / field.w);
        col.push(r.linear[i]);
        // Image y runs down, world y runs up.
        pos.push(Vec2::new(
            px as f32 + rng.r#gen::<f32>(),
            (field.h - py) as f32 - rng.r#gen::<f32>(),
        ));
    }

    Ok(super::normalize(Cloud { pos, col }))
}
