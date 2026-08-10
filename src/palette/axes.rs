//! The three hues the interface spends on X, Y and Z, and the neutral it
//! spends on everything that is not an axis.
//!
//! One source, because four things wear them and they have to agree: the
//! transform gizmos (`gpu/gizmo.rs`), the axis-extension line drawn while a
//! scale handle is held (`indicators.rs`), the corner orientation cross
//! (`ui/axis_widget.rs`), and the roll ring's neutral (`ui/gizmo_ring.rs`).
//! They did *not* agree before this module: the gizmo used full-saturation
//! primaries and secondaries while `indicators.rs` used a muted set, under a
//! doc comment claiming they were "the axis colours the gizmo already uses".
//!
//! **Written in sRGB bytes, which is the space you pick a colour in.** The GPU
//! path wants linear (the surface encodes for itself — see the module note in
//! `palette/mod.rs`), so anything drawn in the world goes through
//! [`axis_linear`]; anything drawn by egui takes the bytes as they are.

use glam::Vec3;

/// X, Y, Z. Muted rather than the full-saturation primaries a gizmo usually
/// gets: these are drawn *over* the artwork, several at a time, and pure red
/// on a fractal is a stripe of interface across a picture.
pub const AXIS_SRGB: [[u8; 3]; 3] = [
    [226, 92, 92],   // X — warm red
    [126, 206, 106], // Y — green
    [102, 150, 232], // Z — blue
];

/// The neutral the axes fade toward at a gizmo's origin, and the roll ring's
/// idle colour. Faintly cool, and lighter than the reference tetrahedron's
/// grey so the two don't read as the same thing.
pub const NEUTRAL_SRGB: [u8; 3] = [214, 218, 228];

/// The identity tetrahedron drawn beside every transform: a landmark, not a
/// handle, so it gets no hue at all.
pub const REFERENCE_SRGB: [u8; 3] = [168, 170, 178];

/// One axis hue, linear, for anything the GPU draws.
pub fn axis_linear(k: usize) -> Vec3 {
    super::from_srgb8(AXIS_SRGB[k.min(2)])
}

/// The secondary belonging to the pair of axes `(ka, kb)`: yellow for X+Y,
/// cyan for Y+Z, magenta for X+Z — but *derived* from [`AXIS_SRGB`] rather
/// than written down, so it belongs to the same family.
///
/// A gizmo needs six colours, three for the axes and three for the parts that
/// span two axes, and the obvious six are the corners of the RGB cube. That is
/// what this used to draw and it is why the thing looked like a test card: the
/// primaries and the secondaries were all as loud as the display can be, and
/// the secondaries were the loudest thing in the picture because they had the
/// most area.
///
/// What actually makes [`AXIS_SRGB`] a family is not its hues — those are
/// spread right around the wheel — but that all three sit in a narrow band of
/// Oklab lightness (0.65–0.78) and low chroma (0.13–0.17), where the cube
/// corners they stand for scatter from L 0.45 to 0.97 at up to twice the
/// chroma. So a secondary is built to join that band:
///
/// - **Hue: from the canonical secondary**, the cube corner with both of the
///   pair's channels full on. Yellow has to be yellow, so it is taken from
///   where yellow is.
/// - **Chroma: the mean of the two axes'.** This is the muting, and it is what
///   stops a secondary from being the loudest thing on screen the way the old
///   pure CMY was.
/// - **Lightness: the corner's, pulled toward the two axes' mean** by
///   [`LIGHTNESS_PULL`]. Not all the way, and not left alone. Left alone, cyan
///   and yellow sit so high that holding any chroma there runs two channels out
///   of gamut and clips them to white; pulled all the way, all three flatten
///   onto one lightness and yellow becomes khaki. Part of the way keeps
///   yellow pale and magenta deep — which is what those colours *are* — inside
///   a band close enough to the axes to belong with them.
///
/// One derivation that doesn't work, and is worth not re-trying: mixing the two
/// axis colours additively. It comes out orange. `AXIS_SRGB`'s red is warm and
/// its green leans yellow, so their true mix is an amber, and correcting the
/// lightness and chroma afterwards leaves the wrong hue behind.
pub fn pair_linear(ka: usize, kb: usize) -> Vec3 {
    let mut corner = Vec3::ZERO;
    corner[ka.min(2)] = 1.0;
    corner[kb.min(2)] = 1.0;
    let lab = super::linear_to_oklab(corner);

    // Oklab's `a`/`b` are a plane: direction is hue, length is chroma.
    let hue = glam::Vec2::new(lab.y, lab.z);
    let hue = if hue.length() > 1e-6 { hue.normalize() } else { glam::Vec2::X };

    let (a, b) = (axis_linear(ka), axis_linear(kb));
    let axes_l = 0.5 * (super::linear_to_oklab(a).x + super::linear_to_oklab(b).x);
    let lightness = lab.x + LIGHTNESS_PULL * (axes_l - lab.x);
    let chroma = 0.5 * (super::chroma(a) + super::chroma(b));

    super::oklab_to_linear(Vec3::new(lightness, hue.x * chroma, hue.y * chroma))
}

/// How far a secondary's lightness is pulled from its cube corner toward the
/// mean of the two axes it spans. See [`pair_linear`] for why it is neither 0
/// nor 1.
const LIGHTNESS_PULL: f32 = 0.55;

/// [`NEUTRAL_SRGB`], linear.
pub fn neutral_linear() -> Vec3 {
    super::from_srgb8(NEUTRAL_SRGB)
}

/// [`REFERENCE_SRGB`], linear.
pub fn reference_linear() -> Vec3 {
    super::from_srgb8(REFERENCE_SRGB)
}

/// An axis hue as an egui colour, for the parts of the interface egui paints.
///
/// `const` so the widgets that want these in a `const` table can have them
/// there and still not own a second copy of the numbers.
pub const fn axis_color32(k: usize) -> egui::Color32 {
    let [r, g, b] = AXIS_SRGB[k];
    egui::Color32::from_rgb(r, g, b)
}

/// [`NEUTRAL_SRGB`] as an egui colour.
pub const fn neutral_color32() -> egui::Color32 {
    let [r, g, b] = NEUTRAL_SRGB;
    egui::Color32::from_rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip the two halves of the interface depend on: egui reads
    /// the bytes, the GPU reads the linear form, and they have to be the same
    /// colour or a gizmo shaft and the cross in the corner disagree about
    /// which way is X.
    #[test]
    fn linear_and_srgb_forms_agree() {
        for k in 0..3 {
            assert_eq!(crate::palette::to_srgb8(axis_linear(k)), AXIS_SRGB[k], "axis {k}");
        }
        assert_eq!(crate::palette::to_srgb8(neutral_linear()), NEUTRAL_SRGB);
        assert_eq!(crate::palette::to_srgb8(reference_linear()), REFERENCE_SRGB);
    }

    /// The derived secondaries have to actually be yellow, cyan and magenta —
    /// each one led by the two channels of the axes it spans, with the third
    /// clearly behind. This is what would break first if the derivation drifted
    /// toward interpolating the primaries instead of mixing them: an olive, a
    /// slate and a mauve, all three of them muddy.
    #[test]
    fn the_pairs_are_the_secondary_they_should_be() {
        for (ka, kb) in [(0, 1), (1, 2), (0, 2)] {
            let c = pair_linear(ka, kb).to_array();
            let odd = 3 - ka - kb; // the channel neither axis leads
            for k in [ka, kb] {
                assert!(
                    c[k] > c[odd] * 1.6,
                    "pair ({ka},{kb}) should lead on {k} over {odd}, got {c:?}"
                );
            }
        }
    }

    /// A secondary is as colourful as the axes it came from — that is the
    /// muting, and the whole point of deriving them rather than reaching for
    /// the cube corner: six colours that look like one set rather than three
    /// soft ones and three shouted ones.
    ///
    /// Its lightness lands strictly *between* its corner's and the axes' mean.
    /// Both ends are failures with names: at the corner, cyan and yellow clip
    /// to white; at the axes' mean, yellow goes khaki.
    #[test]
    fn the_pairs_wear_the_primaries_weight() {
        for (ka, kb) in [(0, 1), (1, 2), (0, 2)] {
            let mut corner = Vec3::ZERO;
            corner[ka] = 1.0;
            corner[kb] = 1.0;
            let corner_l = crate::palette::linear_to_oklab(corner).x;
            let axes_l = 0.5
                * (crate::palette::linear_to_oklab(axis_linear(ka)).x
                    + crate::palette::linear_to_oklab(axis_linear(kb)).x);
            let got_l = crate::palette::linear_to_oklab(pair_linear(ka, kb)).x;
            let (lo, hi) = (corner_l.min(axes_l), corner_l.max(axes_l));
            assert!(
                got_l > lo && got_l < hi,
                "pair ({ka},{kb}) L {got_l} must sit between corner {corner_l} and axes {axes_l}"
            );
        }
        // Secondaries keep the lightness *order* their corners have — yellow
        // pale, magenta deep. Flattening that is exactly what pulling all the
        // way would do.
        let l = |ka: usize, kb: usize| crate::palette::linear_to_oklab(pair_linear(ka, kb)).x;
        assert!(l(0, 1) > l(1, 2), "yellow should stay lighter than cyan");
        assert!(l(1, 2) > l(0, 2), "cyan should stay lighter than magenta");

        for (ka, kb) in [(0, 1), (1, 2), (0, 2)] {
            let (a, b) = (axis_linear(ka), axis_linear(kb));
            let want_c = 0.5 * (crate::palette::chroma(a) + crate::palette::chroma(b));
            let got_c = crate::palette::chroma(pair_linear(ka, kb));
            // Loose: `oklab_to_linear` clamps back into gamut, and cyan's
            // corner is the least chromatic of the three, so asking for the
            // axes' chroma there lands right on the gamut boundary.
            assert!(
                (got_c - want_c).abs() < 0.04,
                "pair ({ka},{kb}) chroma {got_c} vs {want_c}"
            );
        }
    }

    /// Each axis is recognisably its own hue — the whole point of spending
    /// three colours here. A muted palette makes this worth pinning: it is the
    /// direction you'd drift if you kept softening them.
    #[test]
    fn the_three_axes_are_distinguishable() {
        for k in 0..3 {
            let c = axis_linear(k);
            let ch = c.to_array();
            let dominant = ch[k];
            for (j, v) in ch.iter().enumerate() {
                if j != k {
                    assert!(
                        dominant > v * 1.8,
                        "axis {k} must be dominated by channel {k}, got {c:?}"
                    );
                }
            }
        }
    }
}


