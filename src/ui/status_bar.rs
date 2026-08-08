//! Bottom status bar: Inkscape-style persistent context hints on the left,
//! FPS/p99/sparkline/point stats on the right. Must draw *after* the
//! toolbar and every window so `hinted()` hover hints set this frame are
//! already in `app.ui_state.status_hint` by the time we read it.

use crate::app::App;
use crate::scene::similarity_dimension;

use super::hints::HINT_VIEWPORT;
use super::icons;
use super::num;

const BAR_HEIGHT: f32 = 22.0;
const SPARKLINE_SIZE: egui::Vec2 = egui::vec2(100.0, 16.0);

/// Character budget for the `FPS · ms · p99 · ui · wait` cluster: the fixed
/// text plus five numeric cells of 3+5+5+5+5.
const FPS_CLUSTER_CHARS: usize = 46;
/// Budget for `X.XM/Y.YM pts (warming)` — sized for the parenthetical, which
/// otherwise appears and disappears during every warmup and reflows the rest
/// of the bar with it.
const POINTS_CHARS: usize = 26;
/// Budget for `zoom +NN`.
const ZOOM_CHARS: usize = 8;
/// Budget for `(drawing X.XM)`.
const DRAWING_CHARS: usize = 17;
/// Budget for `render <phase>: NNN% (paused)`.
const JOB_CHARS: usize = 34;
/// Gap between the `d ... (class)` and `Λ ...` readouts — same width as the
/// sparkline-to-FPS-text gap below, since both are two sub-readouts inside
/// one cluster rather than a boundary between clusters (those use 10.0).
const DIM_LACUNARITY_GAP: f32 = 6.0;
/// Shared colour for the dimension-lock toggle and the `d` readout it
/// governs — both tint to this and both go `.weak()` together, so the lock
/// and the number it locks visibly belong to each other.
const DIMENSION_LOCK_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 200, 255);

pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    // Resolve the left-hand hint per the plan's three-tier precedence:
    // (1) a `hinted()` widget hover already stashed a hint this frame;
    // (2) else, gizmo-part hover hint, but only when the pointer isn't over
    //     any egui area at all (so it never fights a panel-hover hint);
    // (3) else the bare-viewport default.
    let pointer_over_egui = ui.ctx().is_pointer_over_egui();
    let hint = match app.ui_state.status_hint.take() {
        Some(h) => h,
        None if !pointer_over_egui => app.hovered_hint().unwrap_or(HINT_VIEWPORT).to_string(),
        None => HINT_VIEWPORT.to_string(),
    };

    egui::Panel::bottom("fracturize_status_bar")
        .exact_size(BAR_HEIGHT)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(hint);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (valid, capacity, warming) = app.point_stats();
                    let resp = num::text_cell(
                        ui,
                        &format!(
                            "{}M/{}M pts{}",
                            num::cell(valid as f32 / 1e6, 1, 5),
                            num::cell(capacity as f32 / 1e6, 1, 5),
                            if warming { " (warming)" } else { "" },
                        ),
                        POINTS_CHARS,
                    );
                    // When the attractor has collapsed the renderer draws a
                    // fraction of the buffer (see `App::drawn_points`). Say so
                    // — a point count that doesn't match what's on screen is
                    // the sort of thing you'd otherwise chase for an hour.
                    let height = app.surface_size().1 as f32;
                    if let Some((drawn, _)) = app.withheld_points(height) {
                        resp.on_hover_text(format!(
                            "Drawing {:.1}M of them.\n\nThis scene's attractor is small \
                             enough on screen that the rest would land on pixels already \
                             covered — they'd cost frame time and change nothing. The \
                             chaos game still runs on the whole buffer, and brightness is \
                             normalized, so this is invisible except in the frame rate.",
                            drawn as f32 / 1e6,
                        ));
                        num::text_cell(
                            ui,
                            &format!("(drawing {}M)", num::cell(drawn as f32 / 1e6, 1, 5)),
                            DRAWING_CHARS,
                        );
                    }

                    // Under infinite zoom the picture is the same at every
                    // depth, so the level counter is the only thing that says
                    // how far in you are — and if the scene asked for zoom and
                    // couldn't have it, this is where that gets said.
                    if let Some(err) = app.zoom_error.clone() {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("zoom off")
                                .color(ui.visuals().error_fg_color),
                        )
                        .on_hover_text(err);
                    } else if let Some(z) = app.zoom() {
                        ui.add_space(10.0);
                        num::text_cell(
                            ui,
                            &format!(
                                "{:>7}",
                                format!(
                                    "zoom {}{}",
                                    if app.zoom_level >= 0 { "+" } else { "" },
                                    app.zoom_level
                                ),
                            ),
                            ZOOM_CHARS,
                        )
                        .on_hover_text(format!(
                            "Infinite zoom about transform {} — {:.2} octaves per \
                             period, {:.0}x total.\n\n\
                             The attractor is rendered as the set invariant under \
                             that map, so it has no largest or smallest feature. \
                             Scrolling in past the bottom of the band folds the \
                             camera back to the top of it; the picture is \
                             identical, so this counter is the only thing that \
                             moves.",
                            z.map,
                            z.log_scale / std::f32::consts::LN_2,
                            (1.0f32 / z.scale).powi(app.zoom_level.abs().min(30)),
                        ));
                    }

                    // A running job is worth seeing without the dialog open —
                    // it's using the GPU this readout is measuring.
                    if let Some(job) = app.job() {
                        let text = match job.fraction() {
                            Some(f) => format!("render {}: {}%", job.phase, num::cell(f * 100.0, 0, 3)),
                            None => format!("render {}", job.phase),
                        };
                        let text = if job.paused() { format!("{} (paused)", text) } else { text };
                        ui.add_space(10.0);
                        // A percentage climbing 9% -> 10% -> 100% used to widen
                        // this label twice during every render, shoving the
                        // sparkline and the whole performance cluster leftwards
                        // while you watched them.
                        num::text_cell(ui, &text, JOB_CHARS);
                        ui.spinner();
                    }

                    ui.add_space(6.0);
                    let (_, _, p99_ms) = app.fps_stats();
                    draw_sparkline(ui, &app.frametime_sparkline(), p99_ms);

                    ui.add_space(6.0);
                    let (fps, avg_ms, p99_ms) = app.fps_stats();
                    let wait_ms = app.present_wait_ms();
                    // Five live numbers, several times a second, inside a
                    // `right_to_left` layout — where widgets are placed from
                    // the right edge, so a width change in *any* of them shifts
                    // everything to its left: the sparkline, the zoom counter,
                    // the point stats, the variation readout. This was the
                    // continuous source of the whole bar's vibration. Each
                    // number now sits in a declared character budget, in a
                    // monospace face, in a widget sized to the budget rather
                    // than to the string.
                    let text = format!(
                        "{} FPS · {}ms · p99 {}ms · ui {}ms · wait {}ms",
                        num::cell(fps, 0, 3),
                        num::cell(avg_ms, 1, 5),
                        num::cell(p99_ms, 1, 5),
                        num::cell(app.ui_ms(), 1, 5),
                        num::cell(wait_ms, 1, 5),
                    );
                    num::text_cell(ui, &text, FPS_CLUSTER_CHARS)
                    .on_hover_text(
                        "ui: CPU cost of building this frame's panels.\n\
                         wait: time parked waiting for the display to take the \
                         last frame.\n\n\
                         With vsync on, most of the frame budget *should* be \
                         wait — that's headroom. If the frame time climbs while \
                         wait stays high, the bottleneck is outside the app \
                         (compositor, driver, another GPU client), not the UI.",
                    );

                    // Similarity dimension + lacunarity: the two numbers that
                    // describe what kind of structure the attractor has. Dimension
                    // is cheap (computed every frame); lacunarity is cached and
                    // updates once per second.
                    ui.add_space(10.0);

                    // Dimension lock toggle — clickable, with visual feedback.
                    // Phosphor glyph via the icon system (icons.rs), not a
                    // literal emoji: emoji renders through whatever font the
                    // host system happens to supply, at whatever size, which
                    // is why the locked glyph used to be unreadable.
                    //
                    // This one sets its own size instead of inheriting the
                    // bar's usual small-icon size like its neighbours do:
                    // LOCK and LOCK_OPEN are a two-state toggle whose glyphs
                    // differ only by a single pixel of shackle-gap at the
                    // inherited size, so geometry alone can't carry the
                    // state at rest — it needs both this size bump and the
                    // colour cue below. Don't "tidy" the size back off; that
                    // is exactly how it became unreadable last time.
                    //
                    // 16px is the largest size that fits without growing the
                    // bar: BAR_HEIGHT (22) minus the status panel's default
                    // frame margin (2px top + 2px bottom, from
                    // egui::Frame::side_top_panel) leaves an 18px content
                    // budget — the same budget the sparkline (16px tall) and
                    // every text readout in this row already live inside.
                    // Measured against egui's actual font/layout system
                    // (Phosphor is a fallback font here, so row height is
                    // still set by the primary proportional font's larger
                    // ascent+descent, not Phosphor's own metrics): a FontId
                    // size of 18 — this glyph's ideal legibility size — paints
                    // ~21px tall, 3px over budget. Size 16 paints exactly
                    // 18px at 1x UI scale (18.0–18.7px across the 1.0–2.0x
                    // range `ui_scale` allows), the largest size that still
                    // matches everyone else's budget.
                    const LOCK_ICON_SIZE: f32 = 16.0;
                    let lock_on = app.ui_state.dimension_lock;
                    let lock_icon = if lock_on { icons::LOCK } else { icons::LOCK_OPEN };
                    let lock_rich = egui::RichText::new(lock_icon).size(LOCK_ICON_SIZE);
                    let lock_rich = if lock_on {
                        lock_rich.color(DIMENSION_LOCK_COLOR)
                    } else {
                        lock_rich.weak()
                    };
                    if ui
                        .add(egui::Button::new(lock_rich).small().frame(false))
                        .on_hover_text(if lock_on {
                            "Dimension lock ON — editing one scale adjusts all others to hold d fixed.\n\
                             Click to unlock."
                        } else {
                            "Dimension lock OFF — scales are independent.\n\
                             Click to lock: editing one scale will adjust all others to preserve d."
                        })
                        .clicked()
                    {
                        app.ui_state.dimension_lock = !lock_on;
                    }

                    if let Some(d) = similarity_dimension(&app.scene.transforms) {
                        let class = match d {
                            d if d < 1.7 => "dust",
                            d if d < 2.1 => "lace",
                            d if d < 2.6 => "shell",
                            d if d < 3.0 => "solid",
                            _ => "overfull",
                        };
                        let lambda_str = app
                            .lacunarity()
                            .map(|l| format!("Λ {:.1}", l))
                            .unwrap_or_default();
                        // Only `d` takes the lock's highlight — the lock holds
                        // similarity dimension fixed and does nothing to
                        // lacunarity, so colouring Λ blue too would claim a
                        // guarantee this control doesn't make. Two labels
                        // rather than one string, with a fixed add_space
                        // between them: constant regardless of the values, so
                        // it can't add to the bar's jitter (see the fps
                        // cluster comment above and `num.rs`).
                        let dim_text = format!("d {:.2} ({})", d, class);
                        let dim_rich = if lock_on {
                            egui::RichText::new(dim_text).color(DIMENSION_LOCK_COLOR)
                        } else {
                            egui::RichText::new(dim_text).weak()
                        };
                        let resp = ui
                            .horizontal(|ui| {
                                // Zero out the default item spacing so the
                                // only gap between the two labels is the
                                // explicit add_space below — otherwise egui's
                                // own inter-widget spacing would stack with
                                // it and the gap would no longer match the
                                // rest of the bar's groupings.
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label(dim_rich);
                                ui.add_space(DIM_LACUNARITY_GAP);
                                ui.label(egui::RichText::new(lambda_str).weak());
                            })
                            .response;
                        resp.on_hover_text(
                                "Similarity dimension (d) and lacunarity (Λ).\n\n\
                                 Dimension predicts visual density:\n\
                                 • dust (<1.7): sparse, may disappear at low point counts\n\
                                 • lace (1.7–2.1): intricate surface structure\n\
                                 • shell (2.1–2.6): volumetric but hollow\n\
                                 • solid (2.6–3.0): fills space\n\
                                 • overfull (>3.0): overlapping copies, uniform measure\n\n\
                                 Lacunarity is how much clumpier than a random scatter\n\
                                 the measure is, averaged over four scales:\n\
                                 • Λ ≈ 1: no gaps to read — a smooth, structureless measure\n\
                                 • Λ ≈ 2: menger, flat by construction\n\
                                 • Λ ≈ 10: wellspiral — structure at every scale\n\
                                 • Λ > 40: glasshouse, blossom\n\n\
                                 The pair is what locates you: low d with high Λ is lace,\n\
                                 high d with Λ near 1 is the mush wall — it fills space\n\
                                 and has nothing in it.",
                            );
                    } else {
                        ui.label(egui::RichText::new("d n/a").weak())
                            .on_hover_text("No valid contracting transforms — dimension undefined.");
                    }
                });
            });
        });
}

/// Last-120-sample frametime sparkline: ~100x16 pt painter line segments (no
/// egui_plot dependency), y-scaled to `max(33ms, p99)`, green under 16.7ms /
/// amber above, with a thin horizontal rule at the p99 level.
fn draw_sparkline(ui: &mut egui::Ui, samples: &[f32], p99_ms: f32) {
    let (resp, painter) = ui.allocate_painter(SPARKLINE_SIZE, egui::Sense::hover());
    let rect = resp.rect;
    let y_scale = p99_ms.max(33.0);

    let y_of = |ms: f32| {
        let t = (ms / y_scale).clamp(0.0, 1.0);
        rect.bottom() - t * rect.height()
    };

    painter.hline(
        rect.x_range(),
        y_of(p99_ms),
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)),
    );

    let n = samples.len();
    if n < 2 {
        return;
    }
    let x_of = |i: usize| rect.left() + (i as f32 / (n - 1) as f32) * rect.width();
    for i in 0..n - 1 {
        let (a, b) = (samples[i], samples[i + 1]);
        let avg = 0.5 * (a + b);
        let color = if avg < 16.7 {
            egui::Color32::from_rgb(90, 220, 120)
        } else {
            egui::Color32::from_rgb(230, 175, 60)
        };
        painter.line_segment(
            [egui::pos2(x_of(i), y_of(a)), egui::pos2(x_of(i + 1), y_of(b))],
            egui::Stroke::new(1.2, color),
        );
    }
}
