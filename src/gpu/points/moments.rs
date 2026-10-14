//! Moment-based transmittance: the reconstruction behind volume mode's first three
//! slices, kept so the exact reference can still score it (`volume_ref.rs`'s
//! `emulate_moments_1c`). **No shader uses it any more**: it was replaced by
//! per-pixel slices, which measured 3-10x closer to the exact transport (see
//! VOLUME-RENDERER-PLAN.md, slice 1d).
//!
//! It was the Rust twin of `moment_transmittance()` in an earlier
//! `splat.wgsl`; the tests below still pin the reconstruction itself.
//!
//! # What it solves
//!
//! Pass A of the volume renderer cannot keep a per-pixel list of the depths and
//! absorbances deposited there — there are hundreds to thousands of soft
//! deposits per pixel and nowhere to put them. It keeps instead the total
//! absorbance `b0 = Σ aᵢ` and the first four power moments of the depth,
//! `bₖ = Σ aᵢ·zᵢᵏ`. Pass B then asks, for a point at depth `z0`: how much of the
//! absorbance lies *in front* of it? That is `τ(z0)` and `T(z0) = exp(−τ(z0))`.
//!
//! Four moments pin the distribution down to a three-point **canonical
//! representation** (Münstermann, Krumpen, Klein & Peters 2018, *Moment-Based
//! Order-Independent Transparency*, building on the Hamburger moment problem):
//! the extreme distribution that has these moments and a point mass at `z0`.
//! It puts mass at `z0` and at two further depths `z1`, `z2`, and the amount of
//! absorbance strictly in front of `z0` is bounded between what that
//! distribution puts in front of `z0` and what it puts in front-or-at. Taking
//! `overestimation` of the mass *at* `z0` lands between the two bounds, which is
//! the paper's recommended compromise.
//!
//! # Why the formulas are written the way they are
//!
//! Everything here is f32 on the GPU, and the Hankel matrix of the moments is
//! close to singular whenever the pixel's absorbance sits at one or two depths
//! — which is not an edge case in this renderer, it is a pixel with a single
//! point in it. The Cholesky factors would divide by zero. The **bias** blends
//! the normalised moments toward `(0, 0.375, 0, 0.375)`, the moments of a
//! distribution with real spread, by an amount just big enough to keep the
//! factorisation positive definite; see [`BIAS`] for how it was chosen.

/// How far the normalised moments are blended toward [`BIAS_VECTOR`] before the
/// Hankel matrix is factored.
///
/// The paper's order (its reference code ships 6e-5 for four power moments),
/// kept as insurance rather than tuned: `the_bias_is_the_smallest_that_survives_one_layer`
/// builds single-layer pixels the way the GPU does (f32 sums of a thousand
/// deposits, plus exactly-degenerate layers) and finds the reconstruction
/// correct down to 3e-8 and broken at 0. The shipped value is a thousand times
/// above that on the CPU, which is the margin for a GPU whose fma and division
/// round differently. It distorts nothing visible: it moves the normalised
/// moments by that fraction of the way to [`BIAS_VECTOR`].
pub const BIAS: f32 = 3.0e-5;

/// The moments `(b1, b2, b3, b4)` of a distribution with real spread on
/// `[-1, 1]` — here the arcsine-like `{z = ±√(3/4)·…}` two-point mix. What the
/// moments are pulled toward so the Hankel matrix stays positive definite.
pub const BIAS_VECTOR: [f32; 4] = [0.0, 0.375, 0.0, 0.375];

/// Fraction of the absorbance mass sitting exactly at the queried depth that is
/// counted as in front of it. The paper recommends 0.25; this is 0.1, **chosen by
/// measurement for the volume renderer** and not a free parameter to retune.
///
/// At 0.25 the reconstruction darkens even the front-most material once a
/// pixel's optical depth is large (a soft front: on a slab of total 4 the front
/// face reads T = 0.89 rather than 1, and the loss compounds as `exp(-τ·share)`),
/// so past opacity ~5 no constant emission gain can restore brightness: the
/// gain that would is 44x at opacity 5 and unbounded by 10. At 0.1 the gain that
/// restores it is a smooth `cσ/(1 - e^-cσ)` out to opacity ~10. At 0 there is no
/// front fuzz at all, but a thick slab under-absorbs by 0.6 in T and the render
/// barely changes between opacity 3 and 30: occlusion has gone. 0.1 keeps the
/// slab within the test's bound below.
pub const OVERESTIMATION: f32 = 0.1;

/// What is left of a pixel's absorbance after a point's own deposit is taken out
/// is only believed above this fraction of the total. Below it the difference
/// of two fp32 sums is rounding noise (the sums are built by thousands of
/// additive blends, so their error is several ulps of the total), and the pixel
/// is treated as holding nothing but the point itself.
pub const NOISE_FLOOR: f32 = 1.0e-5;

/// `T` at the depth of a point whose own deposit — absorbance `own` at depth
/// `z` — is part of the totals, **counting everything but that point**.
///
/// The reconstruction counts a share ([`OVERESTIMATION`]) of the mass sitting at
/// the queried depth as in front of it. For a transparent *surface* that is the
/// right hedge. For emission it is a bug: the point asking is itself mass at
/// that depth, so each point partly hides itself, and the lone points of a sparse
/// filament — a point whose absorbance is a large number on its own at high
/// opacity — go dark first, the opposite of what opacity is for (measured on
/// `nautilus` at opacity 30: a lone point at 6% of its brightness). Pass A put
/// the deposit there and pass B can compute exactly the same number, so it is
/// taken back out: absorbance in front of this point *by other material*.
///
/// The price is cancellation: for a pixel the point dominates, `b0 - own` is a
/// small difference of large sums. Below [`NOISE_FLOOR`] of the total it is
/// rounding noise and the answer is 1 (a point alone in its pixel has nothing in
/// front of it); above, the error is at most that fraction of `b0` in absolute
/// absorbance, which is not visible.
pub fn transmittance_excluding(b0: f32, sums: [f32; 4], z: f32, own: f32) -> f32 {
    transmittance_excluding_with(OVERESTIMATION, b0, sums, z, own)
}

/// [`transmittance_excluding`] with the overestimation as an argument.
pub fn transmittance_excluding_with(oe: f32, b0: f32, sums: [f32; 4], z: f32, own: f32) -> f32 {
    let rest = b0 - own;
    if rest <= NOISE_FLOOR * b0 {
        return 1.0;
    }
    let mut zk = 1.0f32;
    let mut rest_sums = sums;
    for k in 0..4 {
        zk *= z;
        rest_sums[k] -= own * zk;
    }
    transmittance_with(BIAS, oe, rest, rest_sums, z)
}

/// `T(z0)`: the fraction of light that survives the absorbance in front of
/// depth `z0`, from the pixel's totals.
///
/// `b0` is the total absorbance `Σ a`; `sums` are the raw (not normalised)
/// power moments `Σ a·zᵏ` for `k = 1..=4`, with `z` warped into `[-1, 1]`.
/// Returns 1 for an empty pixel.
///
/// If the arithmetic leaves the finite range — it should not, with the bias —
/// the answer is 1, i.e. *no occlusion*, which fails toward today's splat
/// rather than toward a black hole.
pub fn transmittance(b0: f32, sums: [f32; 4], z0: f32) -> f32 {
    transmittance_with_bias(BIAS, b0, sums, z0)
}

/// [`transmittance`] with the bias as an argument, so the test that chooses it
/// runs the shipped arithmetic and not a copy of it.
fn transmittance_with_bias(bias: f32, b0: f32, sums: [f32; 4], z0: f32) -> f32 {
    transmittance_with(bias, OVERESTIMATION, b0, sums, z0)
}

/// [`transmittance`] with the bias and the overestimation both as arguments:
/// the exact-reference experiments sweep them.
pub fn transmittance_with(bias: f32, overestimation: f32, b0: f32, sums: [f32; 4], z0: f32) -> f32 {
    if b0 <= 1.0e-20 {
        return 1.0;
    }
    let inv_b0 = 1.0 / b0;
    // Normalise, then bias.
    let mut b = [0.0f32; 4];
    for k in 0..4 {
        let n = sums[k] * inv_b0;
        b[k] = n + (BIAS_VECTOR[k] - n) * bias;
    }

    // Cholesky factorisation of the Hankel matrix [1 b1 b2; b1 b2 b3; b2 b3 b4],
    // keeping only the non-trivial entries.
    let l21_d11 = (-b[0]).mul_add(b[1], b[2]);
    let d11 = (-b[0]).mul_add(b[0], b[1]);
    let inv_d11 = 1.0 / d11;
    let l21 = l21_d11 * inv_d11;
    let squared_depth_variance = (-b[1]).mul_add(b[1], b[3]);
    let d22 = (-l21_d11).mul_add(l21, squared_depth_variance);

    // Solve for the quadratic whose roots, with z0, are the canonical nodes:
    // a scaled inverse image of (1, z0, z0²).
    let mut c = [1.0, z0, z0 * z0];
    c[1] -= b[0];
    c[2] -= b[1] + l21 * c[1];
    c[1] *= inv_d11;
    c[2] /= d22;
    c[1] -= l21 * c[2];
    c[0] -= c[1] * b[0] + c[2] * b[1];

    let inv_c2 = 1.0 / c[2];
    let p = c[1] * inv_c2;
    let q = c[0] * inv_c2;
    // A negative discriminant means the moments fell outside the realisable
    // set by rounding; the two roots then merge at the vertex.
    let disc = (p * p * 0.25 - q).max(0.0);
    let r = disc.sqrt();
    let z1 = -p * 0.5 - r;
    let z2 = -p * 0.5 + r;

    // The weights are the indicator "this node is in front of z0", with the
    // node at z0 itself counted at `OVERESTIMATION`. Interpolate them by
    // divided differences and integrate the polynomial against the moments.
    let f0 = overestimation;
    let f1 = if z1 < z0 { 1.0 } else { 0.0 };
    let f2 = if z2 < z0 { 1.0 } else { 0.0 };
    let f01 = (f1 - f0) / (z1 - z0);
    let f12 = (f2 - f1) / (z2 - z1);
    let f012 = (f12 - f01) / (z2 - z0);
    let poly2 = f012;
    let poly1 = f01 - f012 * (z0 + z1);
    let poly0 = f0 - f01 * z0 + f012 * z0 * z1;
    let fraction = poly2 * b[1] + poly1 * b[0] + poly0;

    let t = (-b0 * fraction).exp();
    if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pixel's totals for a list of `(absorbance, depth)` deposits.
    fn deposit(layers: &[(f32, f32)]) -> (f32, [f32; 4]) {
        let mut b0 = 0.0f64;
        let mut s = [0.0f64; 4];
        for &(a, z) in layers {
            b0 += a as f64;
            for k in 0..4 {
                s[k] += a as f64 * (z as f64).powi(k as i32 + 1);
            }
        }
        (b0 as f32, [s[0] as f32, s[1] as f32, s[2] as f32, s[3] as f32])
    }

    /// Exact transmittance at `z0` for a list of deposits: sum the absorbance
    /// strictly in front.
    fn exact(layers: &[(f32, f32)], z0: f32) -> f32 {
        let tau: f32 = layers.iter().filter(|(_, z)| *z < z0).map(|(a, _)| a).sum();
        (-tau).exp()
    }

    /// A slab as many thin layers, so the moments are what a pixel full of
    /// soft deposits would hold.
    fn slab(total: f32, lo: f32, hi: f32, n: usize) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| (total / n as f32, lo + (hi - lo) * (i as f32 + 0.5) / n as f32))
            .collect()
    }

    #[test]
    fn an_empty_pixel_is_transparent() {
        assert_eq!(transmittance(0.0, [0.0; 4], 0.3), 1.0);
    }

    /// All the absorbance at one depth: nothing in front of it, everything
    /// behind. This is the Hankel matrix's singular case — the one the bias
    /// exists for — and the one a pixel holding a single point produces.
    #[test]
    fn one_layer_is_clear_in_front_and_opaque_behind() {
        for &total in &[0.1f32, 1.0, 3.0, 8.0] {
            let (b0, s) = deposit(&[(total, 0.0)]);
            let front = transmittance(b0, s, -0.5);
            let behind = transmittance(b0, s, 0.5);
            assert!(front > 0.995, "total {}: in front T = {}", total, front);
            let want = (-total).exp();
            assert!(
                (behind - want).abs() < 0.01 + 0.02 * want,
                "total {}: behind T = {}, want {}",
                total, behind, want
            );
        }
    }

    /// Two separated layers: clear, then one layer's worth, then both.
    #[test]
    fn two_layers_step_down_one_at_a_time() {
        let layers = [(1.0, -0.5), (2.0, 0.4)];
        let (b0, s) = deposit(&layers);
        for &(z0, tol) in &[(-0.9f32, 0.02f32), (0.0, 0.05), (0.9, 0.02)] {
            let got = transmittance(b0, s, z0);
            let want = exact(&layers, z0);
            assert!((got - want).abs() < tol, "z0 {}: T = {}, want {}", z0, got, want);
        }
    }

    /// A uniform slab: transmittance should fall roughly exponentially through
    /// it, clear before it and `exp(-total)` after. Four moments cannot be
    /// exact for a continuous distribution; the bound here is what they
    /// actually deliver, stated so a regression is visible.
    #[test]
    fn a_slab_falls_off_exponentially_through_it() {
        let total = 4.0;
        let layers = slab(total, -0.5, 0.5, 400);
        let (b0, s) = deposit(&layers);
        let mut worst = 0.0f32;
        let mut prev = 1.0f32;
        for i in 0..=40 {
            let z0 = -0.8 + 1.6 * i as f32 / 40.0;
            let got = transmittance(b0, s, z0);
            let want = exact(&layers, z0);
            worst = worst.max((got - want).abs());
            // Monotone, up to a little reconstruction ripple.
            assert!(got <= prev + 0.03, "T rose from {} to {} at z0 {}", prev, got, z0);
            prev = got;
        }
        // Measured: 0.204 at overestimation 0.25 and ~0.4 at the shipped 0.1,
        // at the slab's middle, always on the see-through side (four moments
        // under-count the absorbance a thick slab has put in front of a
        // point). The ends are what matter for the picture and those are
        // tight; the bound is the measurement with a little room.
        assert!(worst < 0.45, "worst |T - exact| through the slab was {}", worst);
        assert!(transmittance(b0, s, -0.9) > 0.98);
        assert!((transmittance(b0, s, 0.9) - (-total).exp()).abs() < 0.03);
    }

    /// The result is a transmittance whatever it is fed.
    #[test]
    fn the_answer_is_always_a_valid_transmittance() {
        let mut state = 0x2545F4914F6CDD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f32 / (1u64 << 53) as f32
        };
        for _ in 0..20_000 {
            let n = 1 + (next() * 5.0) as usize;
            let layers: Vec<(f32, f32)> = (0..n)
                .map(|_| (next() * 3.0 * next(), next() * 2.0 - 1.0))
                .collect();
            let (b0, s) = deposit(&layers);
            let z0 = next() * 2.0 - 1.0;
            let t = transmittance(b0, s, z0);
            assert!((0.0..=1.0).contains(&t), "T = {} for {:?} at {}", t, layers, z0);
        }
    }

    /// The worst case for the Cholesky factorisation is a pixel whose
    /// absorbance sits at (nearly) one depth: the Hankel matrix is then
    /// singular, and what keeps it from being *exactly* singular is f32
    /// rounding noise in the sums, which can push a pivot negative.
    ///
    /// So the pixels here are built the way the GPU builds them — a thousand
    /// deposits at a depth jittered by 1e-4, summed in f32 in arrival order —
    /// not from exact f64 totals, which would hand the factorisation a clean
    /// zero and make any bias look fine. The test sweeps the bias and reports
    /// where the reconstruction stops being right; [`BIAS`] must sit well above.
    #[test]
    fn the_bias_is_the_smallest_that_survives_one_layer() {
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f32 / (1u64 << 53) as f32
        };
        // (b0, f32 sums, the layer's depth)
        let mut pixels = Vec::new();
        for trial in 0..60 {
            // The first few are the exactly-degenerate layers: moments that are
            // all zero, or exactly representable powers, where the pivot is
            // not merely small but 0.
            let z_layer = match trial {
                0 => 0.0,
                1 => 0.5,
                2 => -0.5,
                3 => 0.25,
                _ => -0.9 + 1.8 * next(),
            };
            let spread = if trial < 4 { 0.0 } else { [0.0, 1e-5, 1e-4, 1e-3][trial % 4] };
            let (mut b0, mut s) = (0.0f32, [0.0f32; 4]);
            for _ in 0..1000 {
                let a = 0.002 * (0.5 + next());
                let z = (z_layer + spread * (next() - 0.5)).clamp(-1.0, 1.0);
                b0 += a;
                let mut zk = 1.0f32;
                for k in 0..4 {
                    zk *= z;
                    s[k] += a * zk;
                }
            }
            pixels.push((b0, s, z_layer));
        }
        let ok = |bias: f32| -> bool {
            pixels.iter().all(|&(b0, s, z)| {
                [(-0.3f32, 1.0f32), (0.3, (-b0).exp())].iter().all(|&(dz, want)| {
                    let z0 = (z + dz).clamp(-1.0, 1.0);
                    if (z0 - z).abs() < 0.05 {
                        return true;
                    }
                    let want = if z0 > z { want } else { 1.0 };
                    let t = transmittance_with_bias(bias, b0, s, z0);
                    t.is_finite() && (t - want).abs() < 0.1
                })
            })
        };
        let mut smallest = None;
        for e in (-12..=-2).rev() {
            for m in [1.0f32, 3.0] {
                let bias = m * 10f32.powi(e);
                if ok(bias) {
                    smallest = Some(bias);
                }
            }
        }
        // The loop runs 3e-3 down to 1e-9 and keeps overwriting on success, so
        // `smallest` is the smallest passing value in the sweep (not
        // necessarily contiguous with the larger ones, which is why the
        // shipped value is also checked directly).
        println!("bias 0 -> {}", ok(0.0));
        let smallest = smallest.expect("no bias survives a single layer");
        println!("smallest surviving bias on one layer: {:e} (shipped {:e})", smallest, BIAS);
        assert!(ok(BIAS), "the shipped bias fails the one-layer case");
        assert!(BIAS >= 3.0 * smallest, "shipped bias {:e} has no margin over {:e}", BIAS, smallest);
    }

    /// A point alone in its pixel has nothing in front of it, however much
    /// absorbance it carries. Without the exclusion `T` would be
    /// `exp(-OVERESTIMATION * a)` — 6% at an absorbance of 11.
    #[test]
    fn a_lone_point_does_not_hide_itself() {
        for &a in &[0.05f32, 1.0, 11.0, 300.0] {
            for &z in &[-0.9f32, 0.0, 0.7] {
                let (b0, s) = deposit(&[(a, z)]);
                let with_self = transmittance(b0, s, z);
                let without = transmittance_excluding(b0, s, z, a);
                assert!(without > 0.999, "a {}, z {}: T excluding self = {}", a, z, without);
                if a >= 3.0 {
                    assert!(with_self < 0.8, "a {}: including self T = {}", a, with_self);
                }
            }
        }
    }

    /// With a crowd, taking one point out changes almost nothing: the
    /// exclusion is the lone-point fix, not a second model.
    #[test]
    fn in_a_crowd_excluding_one_point_barely_matters() {
        let layers = slab(4.0, -0.5, 0.5, 400);
        let (b0, s) = deposit(&layers);
        let (a, z) = layers[200];
        let with_self = transmittance(b0, s, z);
        let without = transmittance_excluding(b0, s, z, a);
        assert!((with_self - without).abs() < 0.01, "{} vs {}", with_self, without);
    }

}

