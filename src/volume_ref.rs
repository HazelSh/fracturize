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
