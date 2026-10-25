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

/// [`text_cell`] keeping the caller's own styling — for a readout that goes
/// weak to say it has stopped tracking the control it sits beside.
pub fn text_cell_rich(ui: &mut egui::Ui, text: egui::RichText, width: usize) -> egui::Response {
    cell_label(ui, text.monospace(), width, &egui::TextStyle::Monospace)
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
pub fn drag<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    chars: usize,
    decimals: usize,
    dv: Drag<'_, N>,
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

/// How far a drag moves a [`Drag`] relative to the speed its call site asks
/// for. Half, by Hazel's call: every site's speed was tuned against egui's
/// snapping drag, which felt about twice as strong as it should.
pub const DRAG_GAIN: f64 = 0.5;

/// The house number box: an `egui::DragValue` whose *drag* is continuous.
///
/// **egui's drag is quantized twice, and neither can be turned off.** Each
/// frame it snaps the value to the "roundest" number within one pixel of
/// travel either side (`emath::smart_aim`), then rounds that to decimals it
/// derives from the speed. Successive one-pixel windows overlap, so the value
/// sticks on round numbers for several pixels and then jumps — fine for
/// picking `0.5`, useless for dialling in anything between. Here the drag is
/// an exact accumulator instead: every pixel of travel moves the value by
/// `speed × DRAG_GAIN`, no more and no less, and reversing at a range limit
/// responds on the first pixel rather than paying back the overshoot.
///
/// The result is rounded to a grid ten times finer than one pixel's step,
/// which keeps every pixel distinct while keeping saved scenes free of
/// `0.12345679`. Only the output is rounded; the accumulator never is, so
/// rounding can't stick or drift.
///
/// Everything else — the display, click-to-type, shift for fine — is egui's.
/// Same builder methods as `DragValue`, so a call site changes only its
/// constructor.
pub struct Drag<'a, N: egui::emath::Numeric> {
    value: &'a mut N,
    shown: std::rc::Rc<std::cell::Cell<f64>>,
    inner: egui::DragValue<'a>,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
}

impl<'a, N: egui::emath::Numeric> Drag<'a, N> {
    pub fn new(value: &'a mut N) -> Self {
        let shown = std::rc::Rc::new(std::cell::Cell::new(value.to_f64()));
        let cell = shown.clone();
        let inner = egui::DragValue::from_get_set(move |v| {
            if let Some(v) = v {
                cell.set(v);
            }
            cell.get()
        });
        // egui's own integer default (`DragValue::new`), which reaches it
        // through the type rather than the builder.
        let (inner, speed) = if N::INTEGRAL {
            (inner.max_decimals(0).range(N::MIN..=N::MAX), 0.25)
        } else {
            (inner, 1.0)
        };
        let range = if N::INTEGRAL {
            N::MIN.to_f64()..=N::MAX.to_f64()
        } else {
            f64::NEG_INFINITY..=f64::INFINITY
        };
        Self { value, shown, inner, speed, range }
    }

    pub fn speed(mut self, speed: impl Into<f64>) -> Self {
        self.speed = speed.into();
        self
    }

    pub fn range<M: egui::emath::Numeric>(mut self, range: std::ops::RangeInclusive<M>) -> Self {
        self.range = range.start().to_f64()..=range.end().to_f64();
        self.inner = self.inner.range(range);
        self
    }

    pub fn prefix(mut self, prefix: impl egui::IntoAtoms<'a>) -> Self {
        self.inner = self.inner.prefix(prefix);
        self
    }

    pub fn suffix(mut self, suffix: impl egui::IntoAtoms<'a>) -> Self {
        self.inner = self.inner.suffix(suffix);
        self
    }

    pub fn min_decimals(mut self, n: usize) -> Self {
        self.inner = self.inner.min_decimals(n);
        self
    }

    pub fn max_decimals(mut self, n: usize) -> Self {
        self.inner = self.inner.max_decimals(n);
        self
    }

    pub fn fixed_decimals(mut self, n: usize) -> Self {
        self.inner = self.inner.fixed_decimals(n);
        self
    }

    pub fn custom_formatter(
        mut self,
        formatter: impl 'a + Fn(f64, std::ops::RangeInclusive<usize>) -> String,
    ) -> Self {
        self.inner = self.inner.custom_formatter(formatter);
        self
    }

    pub fn custom_parser(mut self, parser: impl 'a + Fn(&str) -> Option<f64>) -> Self {
        self.inner = self.inner.custom_parser(parser);
        self
    }
}

impl<N: egui::emath::Numeric> egui::Widget for Drag<'_, N> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let Drag { value, shown, inner, speed, range } = self;
        let speed = speed * DRAG_GAIN;
        let start = value.to_f64();
        let mut resp = ui.add(inner.speed(speed));
        let mut v = shown.get();

        let key = resp.id.with("precise drag");
        let travel = {
            let d = resp.drag_delta();
            (d.x - d.y) as f64
        };
        if resp.dragged() && travel != 0.0 {
            let fine = ui.input(|i| i.modifiers.shift_only());
            let step = if fine { speed / 10.0 } else { speed };
            let precise = ui.data(|d| d.get_temp::<f64>(key)).unwrap_or(start);
            let precise = drag_step(precise, travel, step, &range);
            ui.data_mut(|d| d.insert_temp(key, precise));
            let pixel = step * ui.input(|i| i.aim_radius() as f64);
            v = drag_output(precise, pixel);
        } else if !resp.dragged() {
            ui.data_mut(|d| d.remove::<f64>(key));
        }

        if N::INTEGRAL {
            v = v.round();
        }
        *value = N::from_f64(v);
        if value.to_f64() != start {
            resp.mark_changed();
        }
        resp
    }
}

/// One frame of a [`Drag`]: `travel` points of pointer motion at `step` per
/// point, clamped to the range. Exact — this is the accumulator.
fn drag_step(precise: f64, travel: f64, step: f64, range: &std::ops::RangeInclusive<f64>) -> f64 {
    (precise + travel * step).clamp(*range.start(), *range.end())
}

/// What a [`Drag`] writes: the accumulator, rounded to a grid ten times finer
/// than `pixel` (the value one physical pixel of travel moves it).
fn drag_output(precise: f64, pixel: f64) -> f64 {
    if !(pixel > 0.0) || !pixel.is_finite() {
        return precise;
    }
    let decimals = ((-pixel.log10()).ceil() + 1.0).clamp(0.0, 15.0) as usize;
    egui::emath::round_to_decimals(precise, decimals)
}

/// Width in points of `text` laid out (but not painted or allocated) at
/// `font` — the measuring half of [`icon_button_width`] and
/// [`text_button_width`], split out so the two don't compute it two
/// different ways.
fn galley_width(ui: &egui::Ui, font: &egui::FontId, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::PLACEHOLDER)
        .size()
        .x
}

/// Width, in points, of the widest of several `(icon, text)` toolbar-button
/// captions, at the current button text style plus the usual frame padding —
/// pass straight to `Button::min_size` (`egui::vec2(width, 0.0)`) so a toggle
/// whose caption changes length between states — Edit Mode/View Mode,
/// Play/Pause — doesn't shove whatever sits to its right sideways when it
/// flips. Same defect as an unstable numeric cell, just with words instead of
/// digits.
///
/// **Pin the width, then pin the caption to one end of it.** egui centres a
/// button's atoms inside `min_size`, so a widened button holds its *neighbours*
/// still while its own icon and text slide left and right by half the
/// difference every time the caption changes length — the same jitter, moved
/// inside the control. Append an `egui::Atom::grow()` after the text to spend
/// the slack on the right and hard-left-align the caption:
///
/// ```ignore
/// let w = num::icon_button_width(ui, &[(icons::PLAY, "Play"), (icons::PAUSE, "Pause")]);
/// ui.add(egui::Button::new((icons::PLAY, "Play", egui::Atom::grow()))
///     .min_size(egui::vec2(w, 0.0)));
/// ```
///
/// Left rather than centred because the icon is the part you aim at and
/// recognise, and a column of buttons whose icons don't line up vertically
/// reads as a column of buttons that are subtly misaligned.
///
/// Deliberately not [`mono_width`]: these are icon+word pairs set in the
/// toolbar's proportional face, where a fixed per-character advance doesn't
/// hold, and switching a single toggle to monospace would look out of place
/// beside its neighbours. So this measures the actual candidate strings —
/// icon glyph, the layout's icon-to-text gap, then the text galley — the same
/// three pieces `AtomLayout` lays out when the button is actually drawn,
/// rather than assuming anything about their widths.
pub fn icon_button_width(ui: &egui::Ui, candidates: &[(&str, &str)]) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let gap = ui.spacing().icon_spacing;
    let widest = candidates
        .iter()
        .map(|(icon, text)| {
            let icon_w = icon
                .chars()
                .next()
                .map_or(0.0, |c| ui.ctx().fonts_mut(|f| f.glyph_width(&font, c)));
            icon_w + gap + galley_width(ui, &font, text)
        })
        .fold(0.0_f32, f32::max);
    widest + 2.0 * ui.spacing().button_padding.x
}

/// [`icon_button_width`]'s icon-less sibling, for a button whose caption
/// comes out of a formatter rather than a couple of fixed strings — the
/// point-count readout, which prints anywhere from `"100k pts"` up to
/// whatever `"{max:.1}M pts"` comes out to on this machine's VRAM.
///
/// `candidates` should be the *actual* boundary strings the formatter can
/// produce — e.g. the largest value each of its branches can print — not a
/// guess at a wide-looking string. A candidate list built from a fixed
/// literal instead of the formatter's own bounds will silently stop being
/// the true widest case the moment either changes, which is exactly the kind
/// of drift this module exists to rule out.
pub fn text_button_width(ui: &egui::Ui, candidates: &[&str]) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let widest =
        candidates.iter().map(|text| galley_width(ui, &font, text)).fold(0.0_f32, f32::max);
    widest + 2.0 * ui.spacing().button_padding.x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drag one pixel at a time and collect what a `Drag` would write.
    fn drag_by_pixels(start: f64, step: f64, pixels: i32, range: std::ops::RangeInclusive<f64>) -> Vec<f64> {
        let dir = pixels.signum() as f64;
        let mut precise = start;
        (0..pixels.abs())
            .map(|_| {
                precise = drag_step(precise, dir, step, &range);
                drag_output(precise, step)
            })
            .collect()
    }

    #[test]
    fn every_pixel_of_a_drag_moves_the_value_by_the_same_step() {
        // egui's smart_aim would hold this on 0.5, 1.0, ... for several pixels
        // and then jump. Here every pixel is one step, through round numbers
        // and all.
        for &step in &[0.0025, 0.005 * DRAG_GAIN, 0.25, 0.001, 0.37] {
            let vs = drag_by_pixels(0.4917, step, 400, f64::NEG_INFINITY..=f64::INFINITY);
            let mut last = 0.4917;
            for v in vs {
                let d = v - last;
                assert!((d - step).abs() < step * 0.11, "step {step}: moved {d}");
                last = v;
            }
        }
    }

    #[test]
    fn a_drag_writes_tidy_numbers() {
        let vs = drag_by_pixels(0.0, 0.0025, 3, f64::NEG_INFINITY..=f64::INFINITY);
        assert_eq!(vs, vec![0.0025, 0.005, 0.0075]);
    }

    #[test]
    fn reversing_at_a_limit_responds_on_the_first_pixel() {
        let mut precise = 0.99;
        for _ in 0..50 {
            precise = drag_step(precise, 1.0, 0.01, &(0.0..=1.0));
        }
        assert_eq!(precise, 1.0);
        let back = drag_step(precise, -1.0, 0.01, &(0.0..=1.0));
        assert!((drag_output(back, 0.01) - 0.99).abs() < 1e-12);
    }

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

    #[test]
    fn icon_button_width_is_the_same_for_both_toggle_states() {
        // Real fonts (Phosphor included), not egui's empty test fonts, so the
        // numbers reflect what actually ships — this is the toolbar's
        // Edit Mode/View Mode toggle and Play/Pause transport button.
        let ctx = egui::Context::default();
        super::super::install_fonts(&ctx);
        let _ = ctx.run_ui(Default::default(), |ui| {
            let mode_candidates = [
                (super::super::icons::CUBE, "Edit Mode"),
                (super::super::icons::EYE, "View Mode"),
            ];
            let mode_width = icon_button_width(ui, &mode_candidates);
            // Every candidate individually, fed back through as a lone
            // "both states" slice, must fit inside the shared width — that's
            // the property that keeps the button from moving.
            for candidate in mode_candidates {
                assert!(icon_button_width(ui, &[candidate]) <= mode_width);
            }
            println!("Edit Mode/View Mode shared width: {mode_width}");

            let play_candidates =
                [(super::super::icons::PLAY, "Play"), (super::super::icons::PAUSE, "Pause")];
            let play_width = icon_button_width(ui, &play_candidates);
            for candidate in play_candidates {
                assert!(icon_button_width(ui, &[candidate]) <= play_width);
            }
            println!("Play/Pause shared width: {play_width}");

            // The two toggles' widths aren't expected to match each other,
            // just to each be internally state-independent, and to be
            // plausible (a two-glyph icon+word button in a normal UI font is
            // on the order of tens of points, not near zero).
            assert!(mode_width > 20.0, "mode_width = {mode_width}");
            assert!(play_width > 20.0, "play_width = {play_width}");
        });
    }

    #[test]
    fn point_count_width_covers_every_representative_count() {
        use super::super::toolbar::{POINT_COUNT_K_M_BOUNDARY, point_count_label};

        let ctx = egui::Context::default();
        super::super::install_fonts(&ctx);
        let _ = ctx.run_ui(Default::default(), |ui| {
            // A stand-in device ceiling: tests can't depend on the GPU they
            // happen to run on, but this exercises the same k/M crossing and
            // multi-digit M magnitudes a real `max_point_capacity()` would.
            let max_capacity = 50_000_000u32;
            let width = text_button_width(
                ui,
                &[
                    &point_count_label(POINT_COUNT_K_M_BOUNDARY - 1),
                    &point_count_label(max_capacity),
                ],
            );
            println!("point-count button width (50M ceiling): {width}");

            // A spread across both formats and their digit-count boundaries:
            // the slider's floor, the k/M crossing (including 999_999, where
            // `{:.0}` rounds a k-value up an extra digit to "1000k"), and
            // several M magnitudes up to the simulated ceiling.
            for capacity in
                [100_000, 999_000, 999_999, 1_000_000, 9_900_000, max_capacity]
            {
                let label = point_count_label(capacity);
                let label_width = text_button_width(ui, &[&label]);
                assert!(
                    label_width <= width,
                    "{label:?} ({label_width}) exceeds the computed bound ({width})"
                );
            }
        });
    }
}
