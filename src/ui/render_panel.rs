//! Render window: everything that decides how the accumulated points get
//! turned into pixels — renderer mode, exposure, point size, buffer capacity,
//! haze, and color falloff/contrast.
//!
//! Which controls commit history is deliberate (see the matching setters on
//! `App`): parameters Ctrl+S writes back into the scene TOML are edits and
//! are undoable; view-only knobs (mode, exposure) and the performance-
//! only point count are not, exactly matching their keybind counterparts.

use crate::app::{App, RenderMode};

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.render_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Render, "Render")
        .open(&mut open)
        .show(ctx, |ui| {
            draw_renderer(ui, app);
            ui.separator();
            draw_points(ui, app);
            ui.separator();
            draw_color(ui, app);
            ui.separator();
            draw_haze(ui, app);
            ui.separator();
            draw_output(ui, app);
        });

    super::remember(ctx, app, super::WindowKey::Render);
    app.ui_state.panels.render_open = open;
}

/// The points/splat segmented toggle, shared by this panel and the toolbar.
pub fn render_mode(ui: &mut egui::Ui, app: &mut App) {
    let mode = app.render_mode;
    let resp = ui.selectable_label(mode == RenderMode::Points, "points");
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "One additive splat per point — crisp, dusty edges (R)",
        "click: switch to the points renderer",
    );
    if resp.clicked() {
        app.set_render_mode(RenderMode::Points);
    }

    let resp = ui.selectable_label(mode == RenderMode::Splat, "splat");
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Log-density accumulation — smoother tonemapping, exposure applies (R)",
        "click: switch to the splat renderer",
    );
    if resp.clicked() {
        app.set_render_mode(RenderMode::Splat);
    }
}

fn draw_renderer(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("Renderer");
        render_mode(ui, app);
    });

    let splat = app.render_mode == RenderMode::Splat;
    ui.add_enabled_ui(splat, |ui| {
        let mut exposure = app.exposure;
        let resp = ui.add(
            egui::Slider::new(&mut exposure, 0.01..=20.0)
                .logarithmic(true)
                .text("exposure"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if splat {
                "Splat exposure — log-density brightness (W / Shift+W)"
            } else {
                "Only applies to the splat renderer"
            },
            "drag: adjust splat exposure",
        );
        if resp.changed() {
            app.exposure = exposure.clamp(0.01, 100.0);
        }
    });
}

fn draw_points(ui: &mut egui::Ui, app: &mut App) {
    let mut size = app.point_size;
    let resp = ui.add(
        egui::Slider::new(&mut size, 0.0001..=0.02)
            .logarithmic(true)
            .text("point size"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "World-space point size — smaller is finer and dustier ([ / ])",
        "drag: adjust point size",
    );
    if resp.changed() {
        app.set_point_size(size);
    }

    point_count(ui, app);
}

/// The point-count slider, shared by this panel and the toolbar's quick
/// control. Deliberately one widget rather than two: the rate limiting,
/// in-flight display and prefs persistence all hang off it, and two copies
/// would drift.
///
/// Point count is a *render* property, not scene data: edited here, persisted
/// to prefs, never written to the scene TOML.
///
/// Logarithmic and live, deliberately. This is the one control that decides
/// whether the machine stays responsive, and the previous value-box-plus-Apply
/// shape was exactly wrong for that: you committed blind to a number and only
/// found out afterwards. Dragging a log slider moves you by a constant *factor*
/// per pixel, so 0.5M to 50M is a short sweep rather than an eternity, and
/// because it applies as you go (rate-limited in `App::apply_pending_capacity`)
/// you watch the FPS and p99 readouts degrade under your hand and can back off
/// without committing.
pub fn point_count(ui: &mut egui::Ui, app: &mut App) {
    let applied = app.point_capacity();
    let max_m = app.max_point_capacity() as f32 / 1e6;
    let pending = app.pending_point_capacity();
    // While a change is in flight, keep showing the value the drag asked for
    // rather than snapping back to the applied one between reallocations.
    let mut millions = pending.unwrap_or(applied) as f32 / 1e6;

    let resp = ui.add(
        egui::Slider::new(&mut millions, 0.1..=max_m)
            .logarithmic(true)
            .custom_formatter(|v, _| {
                if v < 1.0 {
                    format!("{:.0}k", v * 1000.0)
                } else {
                    format!("{:.2}M", v)
                }
            })
            .text("points"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        format!(
            "Points the chaos game keeps in flight — the main quality/performance dial (max {:.0}M on this GPU). \
             Applies as you drag, a few times a second, so watch the FPS and p99 readouts move.",
            max_m
        ),
        "drag: adjust point count — watch the frame stats",
    );
    if resp.changed() {
        app.request_point_capacity((millions * 1e6).round() as u32);
    }

    if pending.is_some() {
        ui.label(
            egui::RichText::new("reallocating…")
                .small()
                .weak(),
        );
    }
}

fn draw_output(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        // `color_edit_button_rgb` takes linear RGB, which is exactly what
        // `Scene::background` stores and what `LoadOp::Clear` wants — no
        // conversion anywhere in the chain.
        let mut rgb = app.scene.background.to_array();
        let resp = ui.color_edit_button_rgb(&mut rgb);
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Background colour. Scene data — saved with the scene, and undoable.",
            "click: pick a background colour",
        );
        if resp.changed() {
            app.set_background(glam::Vec3::from(rgb));
        }
        ui.label("background");

        let mut transparent = app.transparent_render;
        let resp = ui.checkbox(&mut transparent, "transparent");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Write an alpha channel in screenshots (S) and render jobs so they can be \
             composited. The window itself stays opaque — there's nothing behind it to show \
             through. Not available for .avif output.",
            "click: toggle transparent output",
        );
        if resp.changed() {
            app.transparent_render = transparent;
        }
    });
}

fn draw_color(ui: &mut egui::Ui, app: &mut App) {
    let mut falloff = app.color_falloff;
    let resp = ui.add(egui::Slider::new(&mut falloff, 0.0..=4.0).text("color falloff"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Scale-aware color accumulation exponent — 0 disables it, lower is finer color detail (D / Shift+D)",
        "drag: adjust color falloff",
    );
    if resp.changed() {
        app.set_color_falloff(falloff);
    }

    let mut contrast = app.color_contrast;
    let resp = ui.add(
        egui::Slider::new(&mut contrast, 0.25..=16.0)
            .logarithmic(true)
            .text("color contrast"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Cyclic colormap contrast stretch (C / Shift+C)",
        "drag: adjust color contrast",
    );
    if resp.changed() {
        app.set_color_contrast(contrast);
    }
}

/// One slider, and a disclosure for the band it normally works out itself.
/// The four raw shader knobs this replaced are documented in `src/haze.rs`,
/// along with why exposing them was the mistake — and why this is called haze
/// rather than fog now that it thins distant material instead of darkening it.
fn draw_haze(ui: &mut egui::Ui, app: &mut App) {
    let mut amount = app.haze_amount;
    let resp = ui.add(
        egui::Slider::new(&mut amount, 0.0..=1.0)
            .fixed_decimals(2)
            .text("haze"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Aerial perspective: distant material thins toward the background and \
         loses colour, so you can read which arm is in front. The band it fades \
         across follows the camera distance automatically (F / Shift+F)",
        "drag: adjust haze",
    );
    if resp.changed() {
        app.set_haze_amount(amount);
    }

    ui.collapsing("haze band", |ui| {
        let (auto_near, auto_far) = crate::haze::auto_band(app.camera.distance);
        let mut pinned = app.haze_band.is_some();
        let resp = ui.checkbox(&mut pinned, "pin the band");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Off: the haze band tracks the camera distance, so it keeps working \
             as you zoom. On: hold it at fixed world-space distances.",
            "click: pin or unpin the haze band",
        );
        if resp.changed() {
            // Pinning starts from whatever the auto band currently is, so
            // the picture doesn't jump the moment you take control.
            app.haze_band = pinned.then_some((auto_near, auto_far));
        }

        // Shown disabled rather than hidden when auto: the resolved band is
        // worth being able to read, and hiding it would leave "pin the band"
        // with nothing to say what it would pin.
        let (mut near, mut far) = app.haze_range();
        ui.add_enabled_ui(pinned, |ui| {
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::DragValue::new(&mut near)
                        .speed(0.05)
                        .range(0.01..=(far - 0.05) as f64)
                        .prefix("near: "),
                );
                let changed_near = hinted(
                    resp,
                    &mut app.ui_state,
                    "World distance where the fade starts",
                    "drag: move the haze's near plane",
                )
                .changed();

                let resp = ui.add(
                    egui::DragValue::new(&mut far)
                        .speed(0.05)
                        .range((near + 0.05) as f64..=100.0)
                        .prefix("far: "),
                );
                let changed_far = hinted(
                    resp,
                    &mut app.ui_state,
                    "World distance where the fade reaches full strength",
                    "drag: move the haze's far plane",
                )
                .changed();

                if changed_near || changed_far {
                    app.haze_band = Some((near.min(far - 0.05), far));
                }
            });
        });

    });
}
