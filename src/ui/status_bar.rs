//! Bottom status bar: Inkscape-style persistent context hints on the left,
//! FPS/p99/sparkline/point stats on the right. Must draw *after* the
//! toolbar and every window so `hinted()` hover hints set this frame are
//! already in `app.ui_state.status_hint` by the time we read it.

use crate::app::App;
use crate::scene::similarity_dimension;

use super::hints::{hinted, HINT_VIEWPORT};
use super::icons;
use super::num;

const BAR_HEIGHT: f32 = 22.0;
/// Widest the open-failure message gets before it truncates; the whole text
/// is in its tooltip.
const NOTICE_WIDTH: f32 = 420.0;
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
        // A file dragged over the window outranks everything: it's the one
        // moment the question on the person's mind is "will this open?".
        _ if app.drop_hover.is_some() => {
            let path = app.drop_hover.as_deref().unwrap_or(std::path::Path::new(""));
            match crate::app::droppable(path) {
                Ok(()) => format!(
                    "drop: open {}",
                    path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ),
                Err(e) => e,
            }
        }
        Some(h) => h,
        None if !pointer_over_egui => app.hovered_hint().unwrap_or(HINT_VIEWPORT).to_string(),
        None => HINT_VIEWPORT.to_string(),
    };

    egui::Panel::bottom("fracturize_status_bar")
        .exact_size(BAR_HEIGHT)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
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
                            "Only {:.1}M of these are being drawn.\n\nThe fractal is so \
                             small on screen right now that more points would only land on \
                             pixels that are already covered. Drawing fewer keeps the frame \
                             rate up and doesn't change the picture.",
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
                            "Infinite zoom into transform {}. The picture repeats \
                             every {:.2} octaves; you're zoomed {:.0}× in.\n\n\
                             Because the picture repeats exactly, this counter is \
                             the only way to tell how deep you are.",
                            z.map,
                            z.log_scale / std::f32::consts::LN_2,
                            (1.0f32 / z.scale).powi(app.zoom_level.abs().min(30)),
                        ));
                    }

                    // A running job is worth seeing without the dialog open —
                    // it's using the GPU this readout is measuring.
                    if let Some(job) = app.job() {
                        // The whole job, not the current phase — the status bar
                        // has room for one number and it should be the one that
                        // answers "how far along is this".
                        let text = format!(
                            "render {}: {}%",
                            job.phase.label(),
                            num::cell(job.overall() * 100.0, 0, 3),
                        );
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
                        "Frames per second, then the average and slowest (p99) \
                         frame times.\n\
                         ui: time spent drawing the panels.\n\
                         wait: time spent waiting for the screen to be ready.\n\n\
                         With vsync on, a high wait is good: it's spare time. If \
                         frame times climb while wait stays high, something \
                         outside Fracturize is slowing things down.",
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
                    // Under xaos the dimension is a spectral radius over the
                    // transition graph, and no one factor on the other maps
                    // holds it — so the lock stands down, and says so.
                    let under_xaos = crate::xaos::layer_has_xaos(&app.measured_maps().0);
                    if ui
                        .add_enabled(!under_xaos, egui::Button::new(lock_rich).small().frame(false))
                        .on_disabled_hover_text(
                            "Dimension lock is unavailable while this layer uses xaos: d then \
                             depends on which transforms may follow which, and scaling the \
                             others can't hold it.",
                        )
                        .on_hover_text(if lock_on {
                            "Dimension lock is on: changing one transform's scale adjusts \
                             the others to keep d the same.\n\
                             Click to turn it off."
                        } else {
                            "Dimension lock is off.\n\
                             Click to turn it on: changing one transform's scale will then \
                             adjust the others to keep d the same."
                        })
                        .clicked()
                    {
                        app.ui_state.dimension_lock = !lock_on;
                    }

                    if let Some(d) = similarity_dimension(&app.measured_maps().0) {
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
                                "Dimension (d) and lacunarity (Λ): two numbers that \
                                 describe the fractal's texture.\n\n\
                                 Dimension says how densely it fills space:\n\
                                 • dust (below 1.7): sparse; may vanish with few points\n\
                                 • lace (1.7 to 2.1): intricate, surface-like\n\
                                 • shell (2.1 to 2.6): has volume, but hollow\n\
                                 • solid (2.6 to 3.0): fills space\n\
                                 • overfull (above 3.0): copies overlap and even out\n\n\
                                 Lacunarity says how clumpy it is, with gaps at many sizes:\n\
                                 • around 1: smooth, no gaps (menger is about 2)\n\
                                 • around 10: detail at every scale (wellspiral)\n\
                                 • above 40: very clumpy (glasshouse, blossom)\n\n\
                                 Low d with high Λ looks like lace. High d with Λ near 1\n\
                                 is a featureless fog.",
                            );
                    } else {
                        ui.label(egui::RichText::new("d n/a").weak())
                            .on_hover_text("Can't work out a dimension: no transform shrinks things");
                    }

                    // The left side goes last, into whatever the readouts
                    // left over, so a long hint or message truncates instead
                    // of running underneath them.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        // Why the last open failed, or what a drop ignored. Stays
                        // until clicked away or the next scene opens.
                        if let Some((text, error)) = app.notice().map(|n| (n.text.clone(), n.error)) {
                            let color = if error {
                                ui.visuals().error_fg_color
                            } else {
                                ui.visuals().warn_fg_color
                            };
                            let first = text.lines().next().unwrap_or_default().to_string();
                            let resp = ui
                                .allocate_ui(egui::vec2(NOTICE_WIDTH, BAR_HEIGHT), |ui| {
                                    ui.add(
                                        egui::Button::new(egui::RichText::new(first).color(color))
                                            .frame(false)
                                            .truncate(),
                                    )
                                })
                                .inner;
                            let resp = hinted(
                                resp,
                                &mut app.ui_state,
                                format!("{}\n\nClick to dismiss.", text),
                                "click: dismiss this message",
                            );
                            if resp.clicked() {
                                app.dismiss_notice();
                            }
                            ui.separator();
                        }
                        ui.add(egui::Label::new(hint).truncate());
                    });
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
