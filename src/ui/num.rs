//! Numeric readouts that don't move.
//!
//! **The problem this module exists for is invisible in a screenshot.** Every
//! numeric readout used to be `format!("{:.N}", v)` dropped into a
//! proportionally-sized label or a content-sized widget. Both halves of that
//! are unstable: the *string* changes length as the value changes, and the
//! *widget* is sized to the string. So a value dithering around a boundary
//! re-lays-out its whole row, every frame, at up to 120fps. It reads as a
//! physical vibration of the interface, it is deeply unpleasant, and no static
//! capture can show it — which is why it survived every review.
//!
//! Three separate width-changing events, in order of how often they fire:
//!
//! 1. **The sign.** `format!("{:.2}", -0.004)` is `"-0.00"`. A value jittering
//!    either side of zero — which is most of them, since yaw, pitch, focus and
//!    position all pass through zero constantly while you orbit — flicks one
//!    character in and out. Note this is *not* IEEE negative zero: `-0.0 == 0.0`
//!    is true, so a `v == 0.0` guard doesn't catch it. The value has to be
//!    rounded to display precision first and the sign dropped if what's left is
//!    zero, which is what [`fixed`] does.
//! 2. **Crossing a power of ten.** `9.99` → `10.00`. Continuous on the frame
//!    rate and frame time readouts.
//! 3. **Conditional fragments** appearing and disappearing — `" (warming)"`,
//!    `"(drawing X.XM)"`, whole clusters.
//!
//! The four rules, applied everywhere rather than patched per site:
//!
//! 1. **Round before testing the sign** ([`fixed`]).
//! 2. **Monospace family** for anything numeric, so digit advances are equal.
//!    Envy Code R when fontconfig resolves it, egui's built-in otherwise; the
//!    fallback is already wired up in [`super::install_fonts`].
//! 3. **A declared character budget per readout** ([`cell`]), right-aligned,
//!    picked from the range the value can actually take.
//! 4. **Size the widget, not just the string** ([`readout`], [`drag`]). A short
//!    string in a content-sized widget still shrinks the widget, so the cell has
//!    to be fixed independently of what's in it. This is the rule that fixes the
//!    inspector's three-DragValue rows, and the one that can't be done by
//!    formatting alone.

/// Format to `decimals` places, dropping a minus sign that only survived
/// rounding — `-0.004` at two places is `0.00`, not `-0.00`.
///
/// Rounding first is the whole point: the question isn't "is this value
/// negative", it's "does the *string* need a sign", and those differ for every
/// value between zero and half an ulp of the display precision.
pub fn fixed(v: f32, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v);
    // Everything after the sign is zeros and a point => the rounded value is
    // zero and the sign is a lie that costs a character.
    if let Some(rest) = s.strip_prefix('-') {
        if rest.bytes().all(|b| b == b'0' || b == b'.') {
            return rest.to_string();
        }
    }
    s
}

/// [`fixed`], right-aligned into an explicit character budget.
///
/// Pick `width` from the range the value can actually take (`distance` is
/// `0.05..=100.0`, so five characters covers it). A value that outgrows its
/// budget is *not* truncated here — it is left long, and the fixed-size widget
/// it goes into clips it. Saturating beats reflowing, and lying beats neither.
pub fn cell(v: f32, decimals: usize, width: usize) -> String {
    format!("{:>1$}", fixed(v, decimals), width)
}

/// Width in points of `chars` monospace characters at the current style.
pub fn mono_width(ui: &egui::Ui, chars: usize) -> f32 {
    mono_width_at(ui, chars, &egui::TextStyle::Monospace)
}

fn mono_width_at(ui: &egui::Ui, chars: usize, style: &egui::TextStyle) -> f32 {
    let size = style.resolve(ui.style()).size;
    let font = egui::FontId::monospace(size);
    // Every digit has the same advance in a monospace face, so '0' speaks for
    // all of them. `fonts_mut` because measuring a glyph can populate the
    // atlas — it's a cache fill, not a mutation of anything we care about.
    ui.ctx().fonts_mut(|f| f.glyph_width(&font, '0')) * chars as f32
}

/// A fixed-width monospace label for an already-composed string — the whole
/// FPS cluster, say, whose individual numbers have each been through [`cell`].
///
/// `width` is the *budget*, in characters; the widget is sized to it whatever
/// the string turns out to be, so a conditional fragment appearing can't shove
/// its neighbours.
pub fn text_cell(ui: &mut egui::Ui, s: &str, width: usize) -> egui::Response {
    cell_label(ui, egui::RichText::new(s).monospace(), width, &egui::TextStyle::Monospace)
}

/// [`text_cell`] at the `Small` text style, for the weak sub-readouts that sit
/// under a control rather than in a bar.
pub fn small_cell(ui: &mut egui::Ui, s: &str, width: usize) -> egui::Response {
    cell_label(
        ui,
        egui::RichText::new(s).monospace().small().weak(),
        width,
        &egui::TextStyle::Small,
    )
}

fn cell_label(
    ui: &mut egui::Ui,
    text: egui::RichText,
    width: usize,
    style: &egui::TextStyle,
) -> egui::Response {
    let w = mono_width_at(ui, width, style);
    let h = ui.text_style_height(style);
    ui.add_sized(
        [w, h],
        egui::Label::new(text)
            // Never wrap and never shrink: this is a gauge, not prose.
            .wrap_mode(egui::TextWrapMode::Extend)
            .sense(egui::Sense::hover()),
    )
}

/// Put a `DragValue` in a cell of fixed width, with a fixed number of decimals.
///
/// Takes the caller's fully-built `DragValue` rather than a pile of parameters,
/// so ranges, speeds, prefixes and suffixes stay where they're readable — and
/// so this can never silently drop one of them.
///
/// `chars` is the budget for the whole displayed string, prefix and suffix
/// included; the button's own padding is added on top.
pub fn drag(
    ui: &mut egui::Ui,
    chars: usize,
    decimals: usize,
    dv: egui::DragValue<'_>,
) -> egui::Response {
    let w = mono_width(ui, chars) + 2.0 * ui.spacing().button_padding.x + 4.0;
    let h = ui.spacing().interact_size.y;
    ui.add_sized(
        [w, h],
        dv.min_decimals(decimals)
            .max_decimals(decimals)
            // Same rounding rule as `fixed`: a position dragged through zero
            // must not flick a minus sign in and out under the pointer.
            .custom_formatter(move |v, _| fixed(v as f32, decimals)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_that_rounds_to_zero_loses_its_sign() {
        // The continuous case: yaw, pitch, focus and position all pass through
        // zero while you orbit, and each crossing used to flick a character in
        // and out of a row of readouts.
        assert_eq!(fixed(-0.004, 2), "0.00");
        assert_eq!(fixed(-0.0, 2), "0.00");
        assert_eq!(fixed(0.004, 2), "0.00");
        assert_eq!(fixed(-0.4, 0), "0");
    }

    #[test]
    fn a_value_that_does_not_round_to_zero_keeps_it() {
        assert_eq!(fixed(-0.006, 2), "-0.01");
        assert_eq!(fixed(-1.5, 1), "-1.5");
    }

    #[test]
    fn the_ieee_negative_zero_trap() {
        // `-0.0 == 0.0` is true, so the obvious guard — `if v == 0.0` — catches
        // neither this nor the far commoner -0.004 case. Rounding first catches
        // both, which is the reason `fixed` is shaped the way it is.
        let v: f32 = -0.0;
        assert!(v == 0.0, "the guard that doesn't work would take this branch");
        assert_eq!(fixed(v, 2), "0.00");
    }

    #[test]
    fn a_cell_is_the_same_width_whatever_is_in_it() {
        for v in [-99.99f32, -0.004, 0.0, 9.99, 10.0, 99.99] {
            assert_eq!(cell(v, 2, 6).chars().count(), 6, "value {}", v);
        }
    }

    #[test]
    fn crossing_a_power_of_ten_does_not_change_the_width() {
        // The frame-time readouts live on this boundary.
        assert_eq!(cell(9.99, 2, 6).chars().count(), cell(10.0, 2, 6).chars().count());
        assert_eq!(cell(9.99, 2, 6), "  9.99");
        assert_eq!(cell(10.0, 2, 6), " 10.00");
    }

    #[test]
    fn an_oversized_value_is_left_long_rather_than_falsified() {
        // The widget clips it. Truncating here would silently show "12.3" for
        // 12345.6, which is worse than a clipped gauge.
        assert_eq!(cell(12345.6, 1, 4), "12345.6");
    }
}
