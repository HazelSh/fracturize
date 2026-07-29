//! Depth-cue fog: one amount, everything else derived.
//!
//! Additive point clouds have no shading and no occlusion, so depth is
//! invisible in them — fog is the only cue this renderer has for "that arm is
//! behind this one". It has been in the shaders all along
//! (`shaders/points/render.wgsl`, `splat.wgsl` both read `fog_near/far/
//! brightness/saturation` off `CameraUniforms`) and it has never looked like
//! it worked, for three compounding reasons:
//!
//! 1. brightness and saturation defaulted to `1.0`, which is a mathematical
//!    no-op — the sliders started at "off" and you had to move *two* of them
//!    before anything happened;
//! 2. near/far defaulted to a fixed 3.0–4.5 world-unit slab that was never
//!    scaled to the scene, so on most scenes it sat wholly in front of or
//!    behind the geometry;
//! 3. none of it was written to the scene file, so any tuning died at Ctrl+S.
//!
//! The fix is to stop exposing the shader's parameters as the user's
//! parameters. There is one control, `amount`, from 0 (off) to 1; the two
//! falloffs come from it, and the near/far band comes from the camera
//! distance, so it follows the framing instead of needing to be re-dialled
//! every time you zoom.

/// Where the fog band starts and ends, as fractions of the orbit distance.
///
/// The camera distance is already the scene's own statement of its scale —
/// scene files set it to frame the attractor, and `randomize.rs` derives it as
/// `radius * 2.4`. Inverting that gives `radius ≈ distance / 2.4`, and a band
/// of `distance ± radius` spans exactly the depth the form occupies. Doing it
/// this way rather than by measuring the attractor keeps fog free of any
/// sampling cost and makes it track zoom for nothing.
const NEAR_FRAC: f32 = 1.0 - 1.0 / 2.4;
const FAR_FRAC: f32 = 1.0 + 1.0 / 2.4;

/// How much brightness survives at the far plane at full strength. Not zero:
/// points that fade to pure black read as a hole in the form rather than as
/// distance, and the far tail of a fractal arm is exactly where that matters.
const MIN_BRIGHTNESS: f32 = 0.08;

/// How much *saturation* survives there. Much gentler than the brightness
/// falloff, and the tuning that decided whether fog was worth keeping at all.
/// The old `--fog` values (0.4 brightness / 0.3 saturation) desaturated nearly
/// as hard as they darkened, which on this renderer reads as "someone drained
/// the colour out of my fractal", not as depth — and colour is most of what
/// these images are. Darkening is what carries the cue on a dark background;
/// a light desaturation on top of it just keeps the far material from
/// competing.
const MIN_SATURATION: f32 = 0.55;

/// Fog band in world units for a camera at `distance`.
pub fn auto_band(distance: f32) -> (f32, f32) {
    let d = distance.max(0.01);
    (d * NEAR_FRAC, d * FAR_FRAC)
}

/// `(brightness, saturation)` multipliers at the far plane for a given amount.
/// `amount == 0` yields `(1.0, 1.0)`, the shader's no-op.
pub fn falloff(amount: f32) -> (f32, f32) {
    let a = amount.clamp(0.0, 1.0);
    (
        1.0 - (1.0 - MIN_BRIGHTNESS) * a,
        1.0 - (1.0 - MIN_SATURATION) * a,
    )
}

/// Recover an amount from a view file written before this module existed,
/// which stored the four raw shader values instead. Only brightness is
/// consulted: the old `adjust_fog_intensity` scaled brightness and saturation
/// by the same factor on every keypress, so they never carried independent
/// information to lose.
pub fn amount_from_brightness(brightness: f32) -> f32 {
    ((1.0 - brightness) / (1.0 - MIN_BRIGHTNESS)).clamp(0.0, 1.0)
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
            let (b, _) = falloff(a);
            assert!(
                (amount_from_brightness(b) - a).abs() < 1e-5,
                "amount {a} -> brightness {b} -> {}",
                amount_from_brightness(b)
            );
        }
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
