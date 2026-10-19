//! The exact emission-absorption transport for the volume renderer's points,
//! computed the slow way, to measure the fast way against.
//!
//! The volume renderer estimates, for every fragment, the transmittance of the
//! material in front of it from per-pixel depth slices of absorbance. Whether
//! that estimate is any good was once judged by eye, which is how two rounds of
//! constants got tuned against symptoms. This is the thing to judge it against.
//! (The moment reconstruction that estimate replaced, and the emulators that
//! scored it on the CPU, are gone; their numbers are in the plan.)
//!
//! **What it does.** `SplatRenderer::dump_fragments` draws the same points
//! through the same vertex stage, kernel, world-space absorbance and emission
//! weights, but writes each fragment to a list instead of blending it (and
//! records the transmittance the GPU path would have given it). Here every
//! pixel's fragments are sorted by true view depth and composited front to
//! back: no reconstruction, no slices, no clamping.
//!
//! ```text
//!     tau   = 0
//!     for each fragment, nearest first:
//!         L += e · exp(-tau) · (colour, 1)
//!         tau += a
//!     T_total = exp(-tau)
//! ```
//!
//! `L` is what pass B accumulates, and `T_total` is what the tonemap reads, so
//! both fields can go through any tonemap and be compared numerically with the
//! GPU's. A fragment's own absorbance is not counted in front of it, exactly as
//! pass B subtracts it.
//!
//! Slow on purpose: a few million points, a small image, a sort. It is a
//! measuring instrument, reached with `FRACTURIZE_VOLUME_REFERENCE=<prefix>` on
//! a `--volume --render --supersample 1` still.

use std::fmt::Write as _;

use crate::gpu::points::splat::FragmentDump;

/// One fragment after the reference has seen its pixel.
pub struct Frag {
    /// Position in its pixel's front-to-back order, 0 = nearest.
    pub rank: u32,
    /// Exact optical depth of the material nearer than it.
    pub tau_front: f32,
    /// The GPU path's transmittance for it.
    pub t_gpu: f32,
    /// Its own absorbance deposit in this pixel.
    pub a: f32,
    /// Its emission weight in this pixel.
    pub e: f32,
    pub depth: f32,
}

pub struct Reference {
    /// `(Σ e·T·colour, Σ e·T)` per pixel.
    pub l: Vec<[f32; 4]>,
    /// Total optical depth per pixel, `-ln T_total`.
    pub tau: Vec<f32>,
    pub frags: Vec<Frag>,
    /// Fragments the draw made, kept or not.
    pub count: u64,
}

/// Composite a dump exactly.
pub fn composite(dump: &FragmentDump, width: u32, height: u32) -> Reference {
    let n = dump.words.len() / 8;
    let pix = |i: usize| -> u32 {
        let w = dump.words[i * 8];
        let (x, y) = (w & 0xffff, w >> 16);
        y * width + x
    };
    let f = |i: usize, k: usize| f32::from_bits(dump.words[i * 8 + k]);
    // Sort by (pixel, depth). Depth is positive, so its bits order like it.
    let mut order: Vec<(u64, u32)> = (0..n)
        .map(|i| (((pix(i) as u64) << 32) | f(i, 1).to_bits() as u64, i as u32))
        .collect();
    order.sort_unstable_by_key(|&(k, _)| k);

    // `FRACTURIZE_REF_PIXEL=x,y`: print every fragment of that pixel.
    let watch = std::env::var("FRACTURIZE_REF_PIXEL").ok().and_then(|v| {
        let (x, y) = v.split_once(',')?;
        Some(y.trim().parse::<u32>().ok()? * width + x.trim().parse::<u32>().ok()?)
    });
    let px = (width * height) as usize;
    let mut l = vec![[0.0f32; 4]; px];
    let mut tau = vec![0.0f32; px];
    let mut frags = Vec::with_capacity(n);
    let mut start = 0;
    while start < order.len() {
        let pixel = (order[start].0 >> 32) as u32;
        let mut end = start;
        while end < order.len() && (order[end].0 >> 32) as u32 == pixel {
            end += 1;
        }
        let mut t = 0.0f64;
        let mut acc = [0.0f64; 4];
        for (rank, &(_, idx)) in order[start..end].iter().enumerate() {
            let i = idx as usize;
            let (a, e) = (f(i, 2) as f64, f(i, 3) as f64);
            let tr = (-t).exp();
            acc[0] += e * tr * f(i, 4) as f64;
            acc[1] += e * tr * f(i, 5) as f64;
            acc[2] += e * tr * f(i, 6) as f64;
            acc[3] += e * tr;
            if watch == Some(pixel) {
                eprintln!(
                    "pixel {} rank {} depth {:.5} a {:.5} e {:.5} T gpu {:.4} exact {:.4}",
                    pixel, rank, f(i, 1), a, e, f(i, 7), tr
                );
            }
            frags.push(Frag {
                rank: rank as u32,
                tau_front: t as f32,
                t_gpu: f(i, 7),
                a: a as f32,
                e: e as f32,
                depth: f(i, 1),
            });
            t += a;
        }
        l[pixel as usize] = [acc[0] as f32, acc[1] as f32, acc[2] as f32, acc[3] as f32];
        tau[pixel as usize] = t as f32;
        start = end;
    }
    Reference { l, tau, frags, count: dump.count }
}

fn quantile(v: &mut [f32], q: f32) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f32 * q) as usize]
}

/// The error of the GPU path against the reference, as a text report.
///
/// `gpu_l` is the GPU's accumulation (`rgba`, 4 floats a pixel) and `gpu_b0`
/// the first moment target (`b0` in `x`), both at the reference's size.
pub fn report(r: &Reference, gpu_l: &[f32], gpu_b0: &[f32]) -> String {
    let mut s = String::new();
    let px = r.l.len();

    // ---- (a) L ----
    let (mut sum_ref, mut sum_gpu, mut sum_abs) = (0.0f64, 0.0f64, 0.0f64);
    let mut ratios = Vec::new();
    let (mut col_err, mut col_w) = (0.0f64, 0.0f64);
    for p in 0..px {
        let (rw, gw) = (r.l[p][3] as f64, gpu_l[p * 4 + 3] as f64);
        sum_ref += rw;
        sum_gpu += gw;
        sum_abs += (gw - rw).abs();
        if rw > 1e-9 && gw > 1e-9 {
            ratios.push((gw / rw).ln() as f32);
            let rc = [r.l[p][0] as f64 / rw, r.l[p][1] as f64 / rw, r.l[p][2] as f64 / rw];
            let gc = [gpu_l[p * 4] as f64 / gw, gpu_l[p * 4 + 1] as f64 / gw, gpu_l[p * 4 + 2] as f64 / gw];
            col_err += rw * ((rc[0] - gc[0]).abs() + (rc[1] - gc[1]).abs() + (rc[2] - gc[2]).abs()) / 3.0;
            col_w += rw;
        }
    }
    let _ = writeln!(s, "(a) L, the attenuated emission pass B accumulates");
    let _ = writeln!(s, "    total density   gpu / reference = {:.4}", sum_gpu / sum_ref.max(1e-30));
    let _ = writeln!(s, "    relative L1     sum|gpu - ref| / sum ref = {:.4}", sum_abs / sum_ref.max(1e-30));
    let _ = writeln!(
        s,
        "    per pixel ln(gpu/ref): median {:+.3}  p10 {:+.3}  p90 {:+.3}  ({} pixels)",
        quantile(&mut ratios.clone(), 0.5),
        quantile(&mut ratios.clone(), 0.1),
        quantile(&mut ratios, 0.9),
        px
    );
    let _ = writeln!(s, "    mean colour error (per channel, 0-1 of the colour) {:.4}", col_err / col_w.max(1e-30));

    // ---- (b) T_total ----
    let bins = [(0.0f32, 0.3f32), (0.3, 1.0), (1.0, 3.0), (3.0, 10.0), (10.0, f32::INFINITY)];
    let _ = writeln!(s, "\n(b) T_total, the share of background that survives (gpu = exp(-b0), blurred)");
    let _ = writeln!(s, "    ref tau       pixels    mean T ref   mean T gpu   mean |dT|   median b0/tau");
    for &(lo, hi) in &bins {
        let (mut n, mut tr, mut tg, mut ad) = (0usize, 0.0f64, 0.0f64, 0.0f64);
        let mut q = Vec::new();
        for p in 0..px {
            let t = r.tau[p];
            if t > 0.0 && t >= lo && t < hi {
                let b = gpu_b0[p * 4];
                let (a, c) = ((-t as f64).exp(), (-(b as f64)).exp());
                n += 1;
                tr += a;
                tg += c;
                ad += (a - c).abs();
                q.push(b / t);
            }
        }
        if n > 0 {
            let _ = writeln!(
                s,
                "    {:>4.1}-{:<5.1} {:>9}   {:>9.4}    {:>9.4}   {:>9.4}   {:>9.3}",
                lo,
                if hi.is_finite() { hi } else { 999.0 },
                n,
                tr / n as f64,
                tg / n as f64,
                ad / n as f64,
                quantile(&mut q, 0.5)
            );
        }
    }

    // ---- (c) where, fragment by fragment ----
    let total_e: f64 = r.frags.iter().map(|f| f.e as f64).sum();
    let err = |f: &Frag| (f.t_gpu as f64 - (-(f.tau_front as f64)).exp()).abs() * f.e as f64;
    let total_err: f64 = r.frags.iter().map(err).sum();
    let _ = writeln!(
        s,
        "\n(c) where the transmittance error sits, per fragment (weights: emission e)\n    \
         fragments {} kept of {}; weighted mean |T_gpu - T_exact| = {:.4}",
        r.frags.len(),
        r.count,
        total_err / total_e.max(1e-30)
    );
    let mut table = |title: &str, labels: &[&str], class: &dyn Fn(&Frag) -> usize| {
        let _ = writeln!(s, "    by {}:", title);
        let _ = writeln!(s, "      {:<16} {:>8} {:>8} {:>9} {:>9} {:>8}", "", "frags%", "e%", "bias", "mean|d|", "err%");
        for (c, label) in labels.iter().enumerate() {
            let (mut n, mut e, mut bias, mut ad) = (0usize, 0.0f64, 0.0f64, 0.0f64);
            for f in r.frags.iter().filter(|f| class(f) == c) {
                let d = f.t_gpu as f64 - (-(f.tau_front as f64)).exp();
                n += 1;
                e += f.e as f64;
                bias += d * f.e as f64;
                ad += d.abs() * f.e as f64;
            }
            if n > 0 {
                let _ = writeln!(
                    s,
                    "      {:<16} {:>7.1}% {:>7.1}% {:>+9.4} {:>9.4} {:>7.1}%",
                    label,
                    100.0 * n as f64 / r.frags.len() as f64,
                    100.0 * e / total_e.max(1e-30),
                    bias / e.max(1e-30),
                    ad / e.max(1e-30),
                    100.0 * ad / total_err.max(1e-30)
                );
            }
        }
    };
    table("rank in its pixel (0 = front)", &["0 (front)", "1", "2-3", "4-7", "8+"], &|f| match f.rank {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        4..=7 => 3,
        _ => 4,
    });
    table(
        "exact optical depth in front",
        &["< 0.1", "0.1 - 1", "1 - 3", "3 - 10", "> 10"],
        &|f| match f.tau_front {
            t if t < 0.1 => 0,
            t if t < 1.0 => 1,
            t if t < 3.0 => 2,
            t if t < 10.0 => 3,
            _ => 4,
        },
    );
    table(
        "the fragment's own deposit a",
        &["< 0.1", "0.1 - 1", "1 - 3", "> 3"],
        &|f| match f.a {
            a if a < 0.1 => 0,
            a if a < 1.0 => 1,
            a if a < 3.0 => 2,
            _ => 3,
        },
    );
    let mut depths: Vec<f32> = r.frags.iter().map(|f| f.depth).collect();
    let (d1, d2) = (quantile(&mut depths.clone(), 1.0 / 3.0), quantile(&mut depths, 2.0 / 3.0));
    table("view depth (terciles)", &["near third", "middle third", "far third"], &|f| {
        if f.depth < d1 {
            0
        } else if f.depth < d2 {
            1
        } else {
            2
        }
    });
    let _ = writeln!(
        s,
        "SUMMARY l_ratio={:.4} l_l1={:.4} t_frag={:.4} t_total_abs={:.4}",
        sum_gpu / sum_ref.max(1e-30),
        sum_abs / sum_ref.max(1e-30),
        total_err / total_e.max(1e-30),
        {
            let (mut n, mut ad) = (0usize, 0.0f64);
            for p in 0..px {
                if r.tau[p] > 0.0 {
                    n += 1;
                    ad += ((-(r.tau[p] as f64)).exp() - (-(gpu_b0[p * 4] as f64)).exp()).abs();
                }
            }
            ad / n.max(1) as f64
        }
    );
    s
}


// ---- Emulating estimators on the CPU, over the same fragments ----

/// How a fragment's share of its own slice is read.
#[derive(Clone, Copy, Debug)]
pub enum Within {
    /// The shipped rule: the slice's mass is spread uniformly.
    Linear,
    /// Each slice remembers where its nearest fragment is: nothing is seen
    /// before it, a linear ramp after.
    FrontRamp,
    /// Each slice remembers its absorbance-weighted centroid: a step there.
    CentroidStep,
    /// ...and a smooth step of half-width 0.15 around it.
    CentroidRamp,
    /// A smooth step of this half-width (in hundredths of a slice) around the centroid.
    CentroidRampW(u32),
    /// A uniform of the slice's own mean and variance (one more word a slice).
    Moments,
}

/// One estimator: `bins` slices of the span, read `within` a slice.
#[derive(Clone, Copy, Debug)]
pub struct Estimator {
    pub bins: usize,
    pub within: Within,
    /// The refined span starts where this much absorbance (at most) lies in
    /// front, and ends where this much (at most) lies behind its end... in the
    /// emulator: caps on the 1% and 99% masses. `f64::INFINITY` for none.
    pub lo_cap: f64,
    pub hi_cap: f64,
}

/// The transmittance each fragment of one pixel would get (`frags` nearest
/// first: depth and clamped absorbance), mirroring `fs_abs_coarse`, `fs_refine`,
/// `fs_abs` and `bins_transmittance_ro` step by step.
pub fn estimate(frags: &[(f32, f32)], est: Estimator) -> Vec<f32> {
    let nb = est.bins;
    let nbf = nb as f64;
    let ln: Vec<f64> = frags.iter().map(|f| (f.0 as f64).ln()).collect();
    let near = ln[0];
    let span = ln[ln.len() - 1] - near;
    let scale = if span > 1e-6 { nbf / span } else { 1e-30 };
    // Coarse pass: uniform slices of the span.
    let mut coarse = vec![0.0f64; nb];
    for (i, f) in frags.iter().enumerate() {
        let u = ((ln[i] - near) * scale).clamp(0.0, nbf - 1e-3);
        coarse[u as usize] += f.1 as f64;
    }
    // Refine: the span that holds the middle 98% of the mass.
    let total: f64 = coarse.iter().sum();
    let (mut near2, mut scale2) = (near, scale);
    if total > 0.0 {
        let (lo_mass, hi_mass) = ((total * 0.01).min(est.lo_cap), (total * 0.99).min(est.hi_cap));
        let (mut lo, mut hi, mut cum) = (nb, nb - 1, 0.0);
        for (bin, m) in coarse.iter().enumerate() {
            cum += m;
            if lo == nb && cum >= lo_mass {
                lo = bin;
            }
            if cum >= hi_mass && hi == nb - 1 && bin < nb - 1 && lo != nb {
                hi = bin;
            }
        }
        if lo != nb {
            let width = 1.0 / scale;
            near2 = near + lo as f64 * width;
            scale2 = nbf / ((hi + 1 - lo) as f64 * width);
        }
    }
    // Final slices.
    let place = |l: f64| -> (usize, f64) {
        let t = ((l - near2) * scale2 / nbf).clamp(0.0, 1.0);
        let w = 0.5 + (2.0 * t - 1.0).asin() * std::f64::consts::FRAC_1_PI;
        let u = (w * nbf).clamp(0.0, nbf - 1e-3);
        (u as usize, u - u.floor())
    };
    let pos: Vec<(usize, f64)> = ln.iter().map(|&l| place(l)).collect();
    let mut mass = vec![0.0f64; nb];
    let mut first = vec![1.0f64; nb];
    let mut moment = vec![0.0f64; nb];
    let mut moment2 = vec![0.0f64; nb];
    for (i, f) in frags.iter().enumerate() {
        let (b, fr) = pos[i];
        mass[b] += f.1 as f64;
        moment[b] += f.1 as f64 * fr;
        moment2[b] += f.1 as f64 * fr * fr;
        first[b] = first[b].min(fr);
    }
    let mut cum = vec![0.0f64; nb];
    let mut run = 0.0;
    for b in 0..nb {
        run += mass[b];
        cum[b] = run;
    }
    frags
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let (b, fr) = pos[i];
            let before = if b > 0 { cum[b - 1] } else { 0.0 };
            let inside = (mass[b] - f.1 as f64).max(0.0);
            let share = match est.within {
                Within::Linear => fr,
                Within::FrontRamp => ((fr - first[b]) / (1.0 - first[b] + 1e-9)).clamp(0.0, 1.0),
                Within::CentroidStep => {
                    let c = moment[b] / mass[b].max(1e-30);
                    if fr > c { 1.0 } else { 0.0 }
                }
                Within::CentroidRamp | Within::CentroidRampW(_) => {
                    let w = match est.within { Within::CentroidRampW(w) => w as f64 * 0.01, _ => 0.15 };
                    let c = moment[b] / mass[b].max(1e-30);
                    let t = ((fr - (c - w)) / (2.0 * w)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                }
                Within::Moments => {
                    let m = mass[b].max(1e-30);
                    let c = moment[b] / m;
                    let var = (moment2[b] / m - c * c).max(0.0);
                    let h = (3.0 * var).sqrt().max(1e-4);
                    ((fr - (c - h)) / (2.0 * h)).clamp(0.0, 1.0)
                }
            };
            (-(before + share * inside)).exp() as f32
        })
        .collect()
}

/// Score candidate estimators on a dump: for each, the relative L1 of L, the
/// emission-weighted mean |dT| per fragment, and, on the pixels where the
/// shipped rule gives under half the exact L (the dark patches), the share of
/// them still under half and the median ln(L/L_exact).
pub fn emulate(dump: &FragmentDump, width: u32, height: u32, candidates: &[(&str, Estimator)]) -> String {
    let n = dump.words.len() / 8;
    let f = |i: usize, k: usize| f32::from_bits(dump.words[i * 8 + k]);
    let pix = |i: usize| -> u32 {
        let w = dump.words[i * 8];
        (w >> 16) * width + (w & 0xffff)
    };
    let mut order: Vec<(u64, u32)> = (0..n)
        .map(|i| (((pix(i) as u64) << 32) | f(i, 1).to_bits() as u64, i as u32))
        .collect();
    order.sort_unstable_by_key(|&(k, _)| k);
    let px = (width * height) as usize;
    let nc = candidates.len();
    // Per candidate: L per pixel, and per pixel the exact L.
    let mut l_est = vec![vec![0.0f64; px]; nc];
    let mut l_ref = vec![0.0f64; px];
    let mut l_gpu = vec![0.0f64; px];
    let mut dt_num = vec![0.0f64; nc];
    let mut dt_gpu_agree = 0.0f64;
    let mut e_sum = 0.0f64;
    let mut start = 0;
    while start < order.len() {
        let pixel = (order[start].0 >> 32) as usize;
        let mut end = start;
        while end < order.len() && (order[end].0 >> 32) as usize == pixel {
            end += 1;
        }
        let rows: Vec<usize> = order[start..end].iter().map(|o| o.1 as usize).collect();
        let fr: Vec<(f32, f32)> = rows.iter().map(|&i| (f(i, 1), f(i, 2))).collect();
        let mut tau = 0.0f64;
        let exact: Vec<f64> = fr
            .iter()
            .map(|x| {
                let t = (-tau).exp();
                tau += x.1 as f64;
                t
            })
            .collect();
        for (k, (_, est)) in candidates.iter().enumerate() {
            let t = estimate(&fr, *est);
            for (j, &i) in rows.iter().enumerate() {
                let e = f(i, 3) as f64;
                if !(e * t[j] as f64).is_finite() { continue; }
                l_est[k][pixel] += e * t[j] as f64;
                dt_num[k] += e * (t[j] as f64 - exact[j]).abs();
            }
        }
        for (j, &i) in rows.iter().enumerate() {
            let e = f(i, 3) as f64;
            l_ref[pixel] += e * exact[j];
            l_gpu[pixel] += e * f(i, 7) as f64;
            dt_gpu_agree += e * (f(i, 7) as f64 - exact[j]).abs();
            e_sum += e;
        }
        start = end;
    }
    let base = candidates.iter().position(|c| c.0.contains("shipped")).unwrap_or(0);
    let sum_ref: f64 = l_ref.iter().sum();
    let dark: Vec<usize> = (0..px).filter(|&p| l_ref[p] > 1e-9 && l_est[base][p] < 0.5 * l_ref[p]).collect();
    let mut s = String::new();
    let _ = writeln!(s, "emulated estimators over {} fragments; dark = pixels under half the exact L for `{}` ({} of {})", n, candidates[base].0, dark.len(), px);
    let _ = writeln!(s, "GPU as shipped (from the dump's own T): weighted |dT| {:.4}, relative L1 {:.4}, dark-pixel median ln(L/Lex) {:.3}",
        dt_gpu_agree / e_sum, (0..px).map(|p| (l_gpu[p] - l_ref[p]).abs()).sum::<f64>() / sum_ref,
        { let mut v: Vec<f64> = dark.iter().map(|&p| (l_gpu[p].max(1e-12) / l_ref[p]).ln()).collect(); v.sort_by(|a, b| a.partial_cmp(b).unwrap()); if v.is_empty() { 0.0 } else { v[v.len() / 2] } });
    let _ = writeln!(s, "{:<22} {:>9} {:>9} {:>12} {:>12} {:>9} {:>9} {:>9}", "estimator", "L1 of L", "|dT| w", "dark <half", "dark med ln", "words/px", "all <half", "all >2x");
    for (k, (name, est)) in candidates.iter().enumerate() {
        let l1 = (0..px).map(|p| (l_est[k][p] - l_ref[p]).abs()).sum::<f64>() / sum_ref;
        let still = dark.iter().filter(|&&p| l_est[k][p] < 0.5 * l_ref[p]).count();
        let mut v: Vec<f64> = dark.iter().map(|&p| (l_est[k][p].max(1e-12) / l_ref[p]).ln()).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = if v.is_empty() { 0.0 } else { v[v.len() / 2] };
        let words = est.bins * match est.within { Within::Linear => 1, Within::Moments => 3, _ => 2 };
        let lit: Vec<usize> = (0..px).filter(|&p| l_ref[p] > 1e-9).collect();
        let under = lit.iter().filter(|&&p| l_est[k][p] < 0.5 * l_ref[p]).count();
        let over = lit.iter().filter(|&&p| l_est[k][p] > 2.0 * l_ref[p]).count();
        let _ = writeln!(s, "{:<22} {:>9.4} {:>9.4} {:>12} {:>12.3} {:>9} {:>9} {:>9}", name, l1, dt_num[k] / e_sum, still, med, words, under, over);
    }
    s
}

pub fn default_candidates() -> Vec<(&'static str, Estimator)> {
    let e = |bins, within| Estimator { bins, within, lo_cap: f64::INFINITY, hi_cap: f64::INFINITY };
    let c = |bins, within, lo_cap, hi_cap| Estimator { bins, within, lo_cap, hi_cap };
    vec![
        ("32 lin cap .5/10", c(32, Within::Linear, 0.5, 10.0)),
        ("32 lin cap .5/5", c(32, Within::Linear, 0.5, 5.0)),
        ("32 lin cap .2/5", c(32, Within::Linear, 0.2, 5.0)),
        ("32 lin cap .2/3", c(32, Within::Linear, 0.2, 3.0)),
        ("32 lin cap .1/3", c(32, Within::Linear, 0.1, 3.0)),
        ("32 mom cap .5/10", c(32, Within::Moments, 0.5, 10.0)),
        ("32 mom cap .5/5", c(32, Within::Moments, 0.5, 5.0)),
        ("32 mom cap .2/5", c(32, Within::Moments, 0.2, 5.0)),
        ("32 mom cap .2/3", c(32, Within::Moments, 0.2, 3.0)),
        ("16 lin cap .2/5", c(16, Within::Linear, 0.2, 5.0)),
        ("16 mom cap .2/5", c(16, Within::Moments, 0.2, 5.0)),
        ("32 ramp.08 cap .2/5", c(32, Within::CentroidRampW(8), 0.2, 5.0)),
        ("32 linear (shipped)", e(32, Within::Linear)),
        ("32 front ramp", e(32, Within::FrontRamp)),
        ("32 centroid step", e(32, Within::CentroidStep)),
        ("32 centroid ramp", e(32, Within::CentroidRamp)),
        ("32 ramp w0.08", e(32, Within::CentroidRampW(8))),
        ("32 ramp w0.25", e(32, Within::CentroidRampW(25))),
        ("32 ramp w0.35", e(32, Within::CentroidRampW(35))),
        ("32 moments", e(32, Within::Moments)),
        ("16 moments", e(16, Within::Moments)),
        ("16 linear", e(16, Within::Linear)),
        ("16 front ramp", e(16, Within::FrontRamp)),
        ("16 centroid step", e(16, Within::CentroidStep)),
        ("16 centroid ramp", e(16, Within::CentroidRamp)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(frags: &[(u32, u32, f32, f32, f32)]) -> FragmentDump {
        // (x, y, depth, a, e), colour white, GPU T of 1.
        let mut words = Vec::new();
        for &(x, y, d, a, e) in frags {
            words.extend_from_slice(&[
                x | (y << 16),
                d.to_bits(),
                a.to_bits(),
                e.to_bits(),
                1.0f32.to_bits(),
                1.0f32.to_bits(),
                1.0f32.to_bits(),
                1.0f32.to_bits(),
            ]);
        }
        FragmentDump { count: frags.len() as u64, words }
    }

    /// Two deposits in one pixel, handed over far-first: the near one is lit in
    /// full, the far one by what the near one lets through.
    #[test]
    fn a_pixel_is_composited_front_to_back() {
        let r = composite(&dump(&[(0, 0, 5.0, 2.0, 1.0), (0, 0, 2.0, 1.0, 1.0)]), 1, 1);
        let want = 1.0 + (-1.0f32).exp();
        assert!((r.l[0][3] - want).abs() < 1e-6, "{} vs {}", r.l[0][3], want);
        assert!((r.tau[0] - 3.0).abs() < 1e-6);
        // Ranks, and what is in front of each.
        assert_eq!(r.frags[0].rank, 0);
        assert_eq!(r.frags[0].tau_front, 0.0);
        assert!((r.frags[1].tau_front - 1.0).abs() < 1e-6);
    }

    /// Pixels do not see each other, and the order the GPU appended in is
    /// irrelevant.
    #[test]
    fn pixels_are_independent_and_order_free() {
        let a = composite(&dump(&[(0, 0, 1.0, 1.0, 1.0), (1, 0, 1.0, 5.0, 2.0), (0, 0, 2.0, 1.0, 1.0)]), 2, 1);
        let b = composite(&dump(&[(0, 0, 2.0, 1.0, 1.0), (0, 0, 1.0, 1.0, 1.0), (1, 0, 1.0, 5.0, 2.0)]), 2, 1);
        assert_eq!(a.l, b.l);
        assert!((a.l[1][3] - 2.0).abs() < 1e-6);
    }
}
