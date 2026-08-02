//! Atmospheric haze: one amount, everything else derived.
//!
//! An additive point cloud has no shading and no occlusion, so depth is
//! invisible in it — this is the only cue the renderer has for "that arm is
//! behind this one", and without it the 3D projection stops reading as one.
//!
//! It is *not* fog in the "multiply distant things toward black" sense, which
//! is what this was for a long time and what made it unusable. Darkening is
//! only indistinguishable from distance when the background is already black.
//! Against a pale background it does the opposite of what it's for: distant
//! material gains contrast, and higher contrast reads as *nearer*. And even on
//! the dark default it punched holes — the far tail of an arm went to black,
//! which reads as absence rather than as depth.
//!
//! What aerial perspective actually does is remove contrast against the
//! background and drain colour, so that's what this does: distance costs a
//! point **transmittance** (it contributes less, so the pixel resolves toward
//! the background, whatever colour that is) and **saturation**. On a black
//! background the result is nearly what the old darkening produced, which is
//! why the cue worked there at all; on any other background it now works too.
//!
//! One control, `amount`, from 0 (off) to 1. The two falloffs come from it,
//! and the near/far band comes from the camera distance, so it follows the
//! framing instead of needing to be re-dialled every time you zoom.

/// Where the haze band starts and ends, as fractions of the orbit distance.
///
/// The camera distance is already the scene's own statement of its scale —
/// scene files set it to frame the attractor, and `randomize.rs` derives it as
/// `radius * 2.4`. Inverting that gives `radius ≈ distance / 2.4`, and a band
/// of `distance ± radius` spans exactly the depth the form occupies. Doing it
/// this way rather than by measuring the attractor keeps haze free of any
/// sampling cost and makes it track zoom for nothing.
const NEAR_FRAC: f32 = 1.0 - 1.0 / 2.4;
/// Public because `renorm::MIN_RADIUS` is derived from it: the infinite-zoom
/// band has to reach past where haze has finished hiding things, or its edge
/// shows.
pub const FAR_FRAC: f32 = 1.0 + 1.0 / 2.4;

/// How much of a point's contribution survives at the far plane at full
/// strength.
///
/// Zero — the far plane can dissolve completely into the background. This was
/// floored at 0.12 on the theory that material which vanishes reads as a hole
/// rather than as distance, but that reasoning belonged to the old darkening
/// model, where "gone" meant a black patch. Fading to the *background* is what
/// real distance looks like, so there's nothing to protect against, and
/// stopping short of it just put the far end of the range out of reach. The
/// amount slider is the control: 1.0 means gone, and anything less leaves as
/// much as you asked for.
const MIN_TRANSMITTANCE: f32 = 0.0;

/// How much *saturation* survives there.
///
/// Stronger than it was, now that it isn't compounding with a darkening. The
/// pair have to be read together: fading toward the background alone is a
/// weak cue when the background is dark and the material is dim, and colour
/// is most of what these images are, so draining it is the half of aerial
/// perspective that still says "far away" when the fade has little contrast
/// left to remove.
const MIN_SATURATION: f32 = 0.35;

/// Haze band in world units for a camera at `distance`.
pub fn auto_band(distance: f32) -> (f32, f32) {
    let d = distance.max(0.01);
    (d * NEAR_FRAC, d * FAR_FRAC)
}

/// `(transmittance, saturation)` multipliers at the far plane for a given
/// amount. `amount == 0` yields `(1.0, 1.0)`, the shader's no-op.
pub fn falloff(amount: f32) -> (f32, f32) {
    let a = amount.clamp(0.0, 1.0);
    (
        1.0 - (1.0 - MIN_TRANSMITTANCE) * a,
        1.0 - (1.0 - MIN_SATURATION) * a,
    )
}

/// Recover an amount from a view file written before this module existed,
/// which stored the four raw shader values instead. Only the brightness field
/// is consulted: the old `adjust_fog_intensity` scaled brightness and
/// saturation by the same factor on every keypress, so they never carried
/// independent information to lose. It meant a different thing then (a
/// multiplier toward black rather than toward the background), but it ran over
/// the same 0–1 range from the same control, so it recovers the amount.
pub fn amount_from_brightness(brightness: f32) -> f32 {
    ((1.0 - brightness) / (1.0 - MIN_TRANSMITTANCE)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_the_shader_noop() {
        assert_eq!(falloff(0.0), (1.0, 1.0));
    }

    #[test]
    fn amount_round_trips_through_brightness() {
        for a in [0.0f32, 0.25, 0.5, 0.7, 1.0] {
            let (t, _) = falloff(a);
            assert!(
                (amount_from_brightness(t) - a).abs() < 1e-5,
                "amount {a} -> transmittance {t} -> {}",
                amount_from_brightness(t)
            );
        }
    }

    #[test]
    fn full_haze_dissolves_completely() {
        // The far plane reaches the background exactly — the top of the slider
        // is "gone", not "nearly gone".
        let (t, _) = falloff(1.0);
        assert_eq!(t, 0.0, "full haze must reach zero transmittance");
    }

    #[test]
    fn partial_haze_leaves_a_proportional_share() {
        // Half strength leaves half the contribution: the control is linear in
        // what survives, so "a bit of depth cue" is reachable.
        let (t, _) = falloff(0.5);
        assert!((t - 0.5).abs() < 1e-6, "transmittance {t}");
    }

    #[test]
    fn more_amount_means_less_of_both() {
        let (t_lo, s_lo) = falloff(0.3);
        let (t_hi, s_hi) = falloff(0.9);
        assert!(t_hi < t_lo, "transmittance must fall with amount");
        assert!(s_hi < s_lo, "saturation must fall with amount");
    }

    #[test]
    fn legacy_fog_defaults_map_to_a_strong_amount() {
        // The old `--fog` flag started at brightness 0.4 / saturation 0.3.
        let a = amount_from_brightness(0.4);
        assert!((0.6..=0.75).contains(&a), "legacy --fog maps to amount {a}");
    }

    #[test]
    fn band_straddles_the_framed_form() {
        // A scene framed the way `randomize.rs` frames one: radius r, camera
        // at 2.4r. The band should start just in front of the near face and
        // end just past the far one.
        let r = 1.5;
        let (near, far) = auto_band(2.4 * r);
        assert!((near - (2.4 * r - r)).abs() < 1e-4, "near {near}");
        assert!((far - (2.4 * r + r)).abs() < 1e-4, "far {far}");
    }

    #[test]
    fn band_follows_zoom() {
        let (n1, f1) = auto_band(3.0);
        let (n2, f2) = auto_band(1.5);
        assert!(n2 < n1 && f2 < f1, "band must shrink toward a closer camera");
    }
}
