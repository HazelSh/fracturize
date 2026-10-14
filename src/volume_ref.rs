//! The exact emission-absorption transport for the volume renderer's points,
//! computed the slow way, to measure the fast way against.
//!
//! The volume renderer estimates, for every fragment, the transmittance of the
//! material in front of it from a handful of moments of the pixel's absorbance
//! (`moments.rs`), after blurring them. Whether that estimate is any good was
//! being judged by eye, which is how two rounds of constants got tuned against
//! symptoms. This is the thing to judge it against.
//!
//! **What it does.** `SplatRenderer::dump_fragments` draws the same points
//! through the same vertex stage, kernel, world-space absorbance and emission
//! weights, but writes each fragment to a list instead of blending it (and
//! records the transmittance the GPU path would have given it). Here every
//! pixel's fragments are sorted by true view depth and composited front to
//! back: no reconstruction, no blur, no depth warp, no clamping.
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
    pub pixel: u32,
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
    pub width: u32,
    pub height: u32,
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
            frags.push(Frag {
                pixel,
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
    Reference { width, height, l, tau, frags, count: dump.count }
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

/// How the depth axis the moments live on is chosen.
#[derive(Clone, Copy, Debug)]
pub enum Range {
    /// One log warp for the whole frame, `z = ln(depth)·scale + bias`.
    Global { scale: f32, bias: f32 },
    /// Each pixel's own span, log-spaced from its nearest to its farthest
    /// fragment (past the exact front, if there is one).
    PerPixel,
}

/// A candidate transport estimator, evaluated on the CPU on exactly the
/// fragments the GPU drew, so it can be scored against [`Reference`] before it
/// is built in a shader.
#[derive(Clone, Copy, Debug)]
pub struct Emulation {
    pub range: Range,
    /// The nearest `front_k` fragments of each pixel are kept exactly (their
    /// depths and absorbances), and the moments cover only what is behind.
    pub front_k: usize,
    pub overestimation: f32,
    /// `Some(n)`: instead of moments, `n` equal bins of the pixel's own depth
    /// span (the `PerPixel` axis, linear in the warped depth), each holding the
    /// absorbance that fell in it, with the transmittance interpolated within a
    /// bin (opacity shadow maps).
    pub bins: Option<usize>,
}

/// Score an estimator: the same three headline numbers as [`report`]'s SUMMARY
/// (`l_ratio`, `l_l1`, `t_frag`), with its own transmittance for every fragment.
pub fn emulate(r: &Reference, e: Emulation) -> (f64, f64, f64) {
    use crate::gpu::points::moments::transmittance_excluding_with;
    let (mut sum_ref, mut sum_est, mut sum_abs) = (0.0f64, 0.0f64, 0.0f64);
    let (mut err, mut tot_e) = (0.0f64, 0.0f64);
    let mut start = 0;
    while start < r.frags.len() {
        let pixel = r.frags[start].pixel;
        let mut end = start;
        while end < r.frags.len() && r.frags[end].pixel == pixel {
            end += 1;
        }
        let group = &r.frags[start..end];
        let k = e.front_k.min(group.len());
        let a_front: f64 = group[..k].iter().map(|f| f.a as f64).sum();
        let rest = &group[k..];
        // The moments of what is behind the exact front.
        let (zs, ln_lo, ln_span) = match e.range {
            Range::Global { scale, bias } => {
                let z: Vec<f32> = rest.iter().map(|f| (f.depth.ln() * scale + bias).clamp(-1.0, 1.0)).collect();
                (z, 0.0, 1.0)
            }
            Range::PerPixel => {
                let lo = rest.first().map_or(1.0, |f| f.depth.ln());
                let hi = rest.last().map_or(1.0, |f| f.depth.ln());
                let span = (hi - lo).max(1e-6);
                (rest.iter().map(|f| ((f.depth.ln() - lo) / span * 2.0 - 1.0).clamp(-1.0, 1.0)).collect(), lo, span)
            }
        };
        let _ = (ln_lo, ln_span);
        let (mut b0, mut m) = (0.0f64, [0.0f64; 4]);
        for (f, &z) in rest.iter().zip(&zs) {
            let a = f.a as f64;
            b0 += a;
            let mut zk = 1.0f64;
            for q in 0..4 {
                zk *= z as f64;
                m[q] += a * zk;
            }
        }
        let sums = [m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32];
        // Bin occupancy, when that is the estimator: bin of z in [-1, 1].
        let nb = e.bins.unwrap_or(0);
        let bin_of = |z: f32| -> (usize, f64) {
            let u = ((z as f64 + 1.0) * 0.5 * nb as f64).clamp(0.0, nb as f64 - 1e-9);
            (u as usize, u - (u as usize) as f64)
        };
        let mut bin_a = vec![0.0f64; nb];
        if nb > 0 {
            for (f, &z) in rest.iter().zip(&zs) {
                bin_a[bin_of(z).0] += f.a as f64;
            }
        }
        let (mut l_est, mut l_ref) = (0.0f64, 0.0f64);
        let mut tau = 0.0f64;
        for (i, f) in group.iter().enumerate() {
            let t_est = if i < k {
                let t = (-tau).exp();
                tau += f.a as f64;
                t
            } else if nb > 0 {
                let (bi, frac) = bin_of(zs[i - k]);
                let before: f64 = bin_a[..bi].iter().sum();
                (-(a_front + before + frac * (bin_a[bi] - f.a as f64).max(0.0))).exp()
            } else {
                (-a_front).exp()
                    * transmittance_excluding_with(e.overestimation, b0 as f32, sums, zs[i - k], f.a) as f64
            };
            let t_ref = (-(f.tau_front as f64)).exp();
            l_est += f.e as f64 * t_est;
            l_ref += f.e as f64 * t_ref;
            err += (t_est - t_ref).abs() * f.e as f64;
            tot_e += f.e as f64;
        }
        sum_ref += l_ref;
        sum_est += l_est;
        sum_abs += (l_est - l_ref).abs();
        start = end;
    }
    (sum_est / sum_ref.max(1e-30), sum_abs / sum_ref.max(1e-30), err / tot_e.max(1e-30))
}

/// The slice-1c estimator — four moments of one frame-wide log-warped depth,
/// blurred, reconstructed with overestimation `oe`, own deposit taken back —
/// evaluated on the CPU over the same fragments, so the build that replaced it
/// can be scored against the same reference. Returns the SUMMARY numbers and
/// the mean |dT| of T_total.
///
/// Validated against the GPU path it mirrors: with no blur, its three numbers
/// equal the GPU's to the printed digit; with a blur they follow the measured
/// ones within a few percent (the own-share uses a point primitive's variance
/// for every fragment, which is what the frames measured here are made of).
pub fn emulate_moments_1c(
    r: &Reference,
    warp: (f32, f32),
    blur_px: f32,
    oe: f32,
) -> (f64, f64, f64, f64) {
    use crate::gpu::points::moments::transmittance_excluding_with;
    let (w, h) = (r.width as usize, r.height as usize);
    let z_of = |d: f32| (d.ln() * warp.0 + warp.1).clamp(-1.0, 1.0);
    // Moment fields, 5 channels (b0, b1..b4), f32 as the GPU holds them.
    let mut field = vec![[0.0f32; 5]; w * h];
    for f in &r.frags {
        let z = z_of(f.depth);
        let a = f.a;
        let m = &mut field[f.pixel as usize];
        m[0] += a;
        let mut zk = 1.0f32;
        for k in 1..5 {
            zk *= z;
            m[k] += a * zk;
        }
    }
    if blur_px > 0.0 {
        let radius = ((3.0 * blur_px).ceil() as i32).min(16);
        let weights: Vec<f32> = (-radius..=radius).map(|i| (-0.5 * (i * i) as f32 / (blur_px * blur_px)).exp()).collect();
        let wsum: f32 = weights.iter().sum();
        let pass = |src: &Vec<[f32; 5]>, dx: i32, dy: i32| -> Vec<[f32; 5]> {
            let mut out = vec![[0.0f32; 5]; w * h];
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let mut acc = [0.0f32; 5];
                    for (i, wt) in weights.iter().enumerate() {
                        let o = i as i32 - radius;
                        let (qx, qy) = ((x + dx * o).clamp(0, w as i32 - 1), (y + dy * o).clamp(0, h as i32 - 1));
                        let v = &src[qy as usize * w + qx as usize];
                        for k in 0..5 {
                            acc[k] += wt * v[k];
                        }
                    }
                    for k in 0..5 {
                        out[y as usize * w + x as usize][k] = acc[k] / wsum;
                    }
                }
            }
            out
        };
        field = pass(&pass(&field, 1, 0), 0, 1);
    }
    let own_share = if blur_px > 0.0 { (1.0 / 12.0) / (1.0 / 12.0 + blur_px * blur_px) } else { 1.0 };
    let (mut sum_ref, mut sum_est, mut sum_abs) = (0.0f64, 0.0f64, 0.0f64);
    let (mut err, mut tot_e) = (0.0f64, 0.0f64);
    let mut l_est = vec![0.0f64; w * h];
    for f in &r.frags {
        let m = &field[f.pixel as usize];
        let t = transmittance_excluding_with(
            oe,
            m[0],
            [m[1], m[2], m[3], m[4]],
            z_of(f.depth),
            f.a * own_share,
        ) as f64;
        let t_ref = (-(f.tau_front as f64)).exp();
        l_est[f.pixel as usize] += f.e as f64 * t;
        err += (t - t_ref).abs() * f.e as f64;
        tot_e += f.e as f64;
    }
    for p in 0..w * h {
        let lr = r.l[p][3] as f64;
        sum_ref += lr;
        sum_est += l_est[p];
        sum_abs += (l_est[p] - lr).abs();
    }
    let (mut n, mut ad) = (0usize, 0.0f64);
    for p in 0..w * h {
        if r.tau[p] > 0.0 {
            n += 1;
            ad += ((-(r.tau[p] as f64)).exp() - (-(field[p][0] as f64)).exp()).abs();
        }
    }
    (
        sum_est / sum_ref.max(1e-30),
        sum_abs / sum_ref.max(1e-30),
        err / tot_e.max(1e-30),
        ad / n.max(1) as f64,
    )
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

    /// The slice estimator is exact when every fragment of a pixel has a slice
    /// to itself: the second of two well-separated deposits is lit by exactly
    /// what the first lets through.
    #[test]
    fn slices_are_exact_when_deposits_are_separated() {
        let r = composite(&dump(&[(0, 0, 1.0, 1.0, 1.0), (0, 0, 100.0, 1.0, 1.0)]), 1, 1);
        let (lr, l1, tf) = emulate(
            &r,
            Emulation { range: Range::PerPixel, front_k: 0, overestimation: 0.0, bins: Some(8) },
        );
        assert!((lr - 1.0).abs() < 1e-6 && l1 < 1e-6 && tf < 1e-6, "{} {} {}", lr, l1, tf);
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
