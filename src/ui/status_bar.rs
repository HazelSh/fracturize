//! Bottom status bar: Inkscape-style persistent context hints on the left,
//! FPS/p99/sparkline/point stats on the right. Must draw *after* the
//! toolbar and every window so `hinted()` hover hints set this frame are
//! already in `app.ui_state.status_hint` by the time we read it.

use crate::app::App;

use super::hints::HINT_VIEWPORT;

const BAR_HEIGHT: f32 = 22.0;
const SPARKLINE_SIZE: egui::Vec2 = egui::vec2(100.0, 16.0);

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
                    ui.label(format!(
                        "{:.1}M/{:.1}M pts{}",
                        valid as f32 / 1e6,
                        capacity as f32 / 1e6,
                        if warming { " (warming)" } else { "" },
                    ));

                    ui.add_space(6.0);
                    let (_, _, p99_ms) = app.fps_stats();
                    draw_sparkline(ui, &app.frametime_sparkline(), p99_ms);

                    ui.add_space(6.0);
                    let (fps, avg_ms, p99_ms) = app.fps_stats();
                    let wait_ms = app.present_wait_ms();
                    ui.label(format!(
                        "{:.0} FPS · {:.1}ms · p99 {:.1}ms · ui {:.1}ms · wait {:.1}ms",
                        fps,
                        avg_ms,
                        p99_ms,
                        app.ui_ms(),
                        wait_ms,
                    ))
                    .on_hover_text(
                        "ui: CPU cost of building this frame's panels.\n\
                         wait: time parked waiting for the display to take the \
                         last frame.\n\n\
                         With vsync on, most of the frame budget *should* be \
                         wait — that's headroom. If the frame time climbs while \
                         wait stays high, the bottleneck is outside the app \
                         (compositor, driver, another GPU client), not the UI.",
                    );

                    // Which variation slot E / - / = are pointed at. The one
                    // piece of the retired HUD with no home in a panel: the
                    // Transforms inspector bolds the targeted row, but that
                    // panel is often closed while the keys are still live.
                    if let Some(idx) = app.selected_transform() {
                        let slot = app.selected_variation();
                        let weight = app.scene.transforms[idx].variations[slot];
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "[E] {} {:+.2}",
                                crate::scene::VARIATION_NAMES[slot],
                                weight
                            ))
                            .weak(),
                        );
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
