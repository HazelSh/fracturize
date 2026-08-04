//! Segmented radio: pick one of a small n, where there is no "off".
//!
//! Three settings in this UI are genuinely choose-1-of-n with no unset state —
//! the renderer, the colour source, and how the camera path loops — and all
//! three were drawn as a row of loose `selectable_label`s. That row is a fine
//! *mechanism* and a poor *sign*: a lone toggle button says "this is on or
//! off", and three of them side by side say "three independent toggles that
//! happen to be next to each other". Nothing in the picture rules out turning
//! them all off, or all on, and the widget that rules that out is a radio.
//!
//! So: one connected pill, recessed like a trough, with the chosen segment
//! filled. The group outline is what carries the meaning — these n things are
//! one control, and exactly one of them is lit. Nothing here has an off state
//! to draw, which is precisely why it isn't a checkbox.
//!
//! Each segment is its own [`egui::Response`] and goes through
//! [`hints::hinted`], so every option gets its own tooltip and status-bar hint
//! — the same rule the rest of the panels follow. A segment can also be
//! [disabled](Radio::option_enabled), which greys it *in place* rather than
//! removing it: a mode you can't have yet is still a mode you should be able
//! to find out about. That's the same call the zoom-loop row made when it
//! stopped hiding itself (see `camera_panel::draw_loop`).

use super::hints::hinted;
use super::UiState;

/// Horizontal padding either side of a segment's contents.
const PAD_X: f32 = 7.0;
/// Corner radius of the group's outer ends.
const RADIUS: u8 = 4;
/// Diameter of the ring, and of the dot inside it when chosen. Smaller than
/// egui's own `spacing.icon_width`/`icon_width_inner` (14/8), which are sized
/// for a checkbox sitting alone on a row; four of those in one pill is a lot
/// of furniture. The proportions are kept.
const RING: f32 = 10.0;
const DOT: f32 = 4.4;
/// Gap between the ring and its label.
const RING_GAP: f32 = 5.0;

struct Opt<T> {
    value: T,
    text: String,
    tooltip: String,
    hint: String,
    enabled: bool,
}

/// A segmented one-of-n control. Build it with [`radio`].
pub struct Radio<'a, T> {
    ui_state: &'a mut UiState,
    id_salt: &'static str,
    current: T,
    opts: Vec<Opt<T>>,
}

/// Start a segmented radio showing `current` as the chosen value.
///
/// `id_salt` distinguishes this group from any other in the same panel; the
/// per-segment ids are derived from it, so the segments keep stable ids as
/// options are enabled and disabled.
pub fn radio<'a, T: PartialEq + Copy>(
    ui_state: &'a mut UiState,
    id_salt: &'static str,
    current: T,
) -> Radio<'a, T> {
    Radio { ui_state, id_salt, current, opts: Vec::new() }
}

impl<'a, T: PartialEq + Copy> Radio<'a, T> {
    /// Append a selectable option.
    ///
    /// `tooltip` is what hovering shows; `hint` is the status bar's left-hand
    /// line while hovered, and by convention starts with the gesture
    /// ("click: …").
    pub fn option(
        self,
        value: T,
        text: impl Into<String>,
        tooltip: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        self.push(value, text, tooltip, hint, true)
    }

    /// Append an option that may not be choosable yet. A disabled segment is
    /// greyed *in place* — still hoverable and still hinted, because the
    /// tooltip is where it says why you can't have it and how to get it.
    pub fn option_enabled(
        self,
        enabled: bool,
        value: T,
        text: impl Into<String>,
        tooltip: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        self.push(value, text, tooltip, hint, enabled)
    }

    fn push(
        mut self,
        value: T,
        text: impl Into<String>,
        tooltip: impl Into<String>,
        hint: impl Into<String>,
        enabled: bool,
    ) -> Self {
        self.opts.push(Opt {
            value,
            text: text.into(),
            tooltip: tooltip.into(),
            hint: hint.into(),
            enabled,
        });
        self
    }

    /// Draw it. Returns the newly chosen value, or `None` if nothing changed —
    /// so a caller only calls its setter (and only pushes a history entry) on
    /// an actual change, and clicking the already-chosen segment is a no-op.
    pub fn show(self, ui: &mut egui::Ui) -> Option<T> {
        let Radio { ui_state, id_salt, current, opts } = self;
        if opts.is_empty() {
            return None;
        }

        // Lay the text out first: the group is sized to its contents, so the
        // segments are as wide as they need to be and no wider. Equal-width
        // segments were tried and read worse — "ping-pong" next to "once"
        // leaves the short one swimming in space.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let galleys: Vec<_> = opts
            .iter()
            .map(|o| {
                ui.painter().layout_no_wrap(
                    o.text.clone(),
                    font.clone(),
                    egui::Color32::PLACEHOLDER,
                )
            })
            .collect();

        let height = ui.spacing().interact_size.y;
        let widths: Vec<f32> = galleys
            .iter()
            .map(|g| (g.size().x + RING + RING_GAP + 2.0 * PAD_X).round().max(height))
            .collect();
        let total: f32 = widths.iter().sum();

        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(total, height),
            egui::Sense::hover(),
        );

        let visuals = ui.visuals().clone();
        // The trough. Drawn under everything, across the whole group, so the
        // segments read as divisions of one control rather than as n controls.
        if ui.is_rect_visible(rect) {
            ui.painter().rect_filled(rect, RADIUS, visuals.widgets.inactive.weak_bg_fill);
        }

        let mut chosen = None;
        let mut x = rect.left();
        let last = opts.len() - 1;
        // Segment rects and their selected/hover state, kept for the
        // separator and outline passes below — both of which have to run
        // after every fill or they'd be painted over.
        let mut lit: Vec<bool> = Vec::with_capacity(opts.len());

        for (i, (o, galley)) in opts.iter().zip(&galleys).enumerate() {
            let seg = egui::Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(widths[i], height),
            );
            x += widths[i];

            let id = ui.id().with((id_salt, i));
            let resp = ui.interact(seg, id, egui::Sense::click());
            let selected = o.value == current;
            lit.push(selected);

            let hovered = o.enabled && resp.hovered();
            if ui.is_rect_visible(seg) {
                // Only the group's outer ends are rounded; interior segments
                // are square, so the fills butt up against each other.
                let radius = egui::CornerRadius {
                    nw: if i == 0 { RADIUS } else { 0 },
                    sw: if i == 0 { RADIUS } else { 0 },
                    ne: if i == last { RADIUS } else { 0 },
                    se: if i == last { RADIUS } else { 0 },
                };
                if selected {
                    ui.painter().rect_filled(seg, radius, visuals.selection.bg_fill);
                } else if hovered {
                    ui.painter().rect_filled(seg, radius, visuals.widgets.hovered.weak_bg_fill);
                }

                let color = if !o.enabled {
                    visuals.widgets.noninteractive.fg_stroke.color.gamma_multiply(0.45)
                } else if selected {
                    visuals.selection.stroke.color
                } else if hovered {
                    visuals.widgets.hovered.fg_stroke.color
                } else {
                    visuals.widgets.inactive.fg_stroke.color
                };
                // The ring, and the dot in it when this is the one chosen —
                // the same two circles egui's own `RadioButton` paints, at the
                // same proportions. Drawn rather than typed: the obvious
                // characters for it (U+2B58 ⭘, U+25C9 ◉) are in none of the
                // fonts this app has (Ubuntu-Light, NotoEmoji,
                // emoji-icon-font, Phosphor, Envy Code R), and a missing glyph
                // is a tofu box in the middle of the control. egui draws its
                // own for the same reason.
                let inner = seg.width() - 2.0 * PAD_X;
                let ring_c = egui::pos2(
                    seg.center().x - 0.5 * inner + RING * 0.5,
                    seg.center().y,
                );
                ui.painter().circle_stroke(ring_c, RING * 0.5, egui::Stroke::new(1.0, color));
                if selected {
                    ui.painter().circle_filled(ring_c, DOT * 0.5, color);
                }

                let pos = egui::pos2(
                    ring_c.x + RING * 0.5 + RING_GAP,
                    seg.center().y - 0.5 * galley.size().y,
                );
                ui.painter().galley(pos, galley.clone(), color);
            }

            // `hinted` keys off `contains_pointer`, so a greyed segment still
            // shows its tooltip — which is the whole reason it's greyed rather
            // than gone.
            let resp = hinted(resp, ui_state, o.tooltip.clone(), &o.hint);
            resp.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::RadioButton,
                    o.enabled,
                    selected,
                    &o.text,
                )
            });
            if o.enabled && resp.clicked() && !selected {
                chosen = Some(o.value);
            }
        }

        if ui.is_rect_visible(rect) {
            // Hairlines between adjacent unlit segments. Between a lit segment
            // and its neighbour the fill edge already draws the division, and
            // a line there reads as a border around the selection.
            //
            // Drawn in the *panel's* colour, so it reads as a slot cut through
            // the trough — and, unlike a stroke colour, it can't collide with
            // the trough's own fill. `widgets.noninteractive.bg_stroke` is
            // exactly `widgets.inactive.weak_bg_fill` in egui's dark theme
            // (both gray 60), which drew four segments as one grey slab.
            let mut x = rect.left();
            for i in 0..last {
                x += widths[i];
                if !lit[i] && !lit[i + 1] {
                    ui.painter().line_segment(
                        [
                            egui::pos2(x, rect.top() + 3.0),
                            egui::pos2(x, rect.bottom() - 3.0),
                        ],
                        egui::Stroke::new(1.0, visuals.window_fill),
                    );
                }
            }
        }

        chosen
    }
}
