//! Render window: everything that decides how the accumulated points get
//! turned into pixels — renderer mode, exposure, point size, buffer capacity,
//! fog, and color falloff/contrast.
//!
//! Which controls commit history is deliberate (see the matching setters on
//! `App`): parameters Ctrl+S writes back into the scene TOML are edits and
//! are undoable; view-only knobs (mode, exposure, fog) and the performance-
//! only point count are not, exactly matching their keybind counterparts.

use crate::app::{App, RenderMode};

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.render_open;
    if !open {
        return;
    }

    egui::Window::new("Render")
        .id(egui::Id::new("fracturize_render_window"))
        .open(&mut open)
        .default_pos(egui::pos2(20.0, 300.0))
        .default_width(280.0)
        .show(ctx, |ui| {
            draw_renderer(ui, app);
            ui.separator();
            draw_points(ui, app);
            ui.separator();
            draw_color(ui, app);
            ui.separator();
            draw_fog(ui, app);
        });

    app.ui_state.panels.render_open = open;
}

fn draw_renderer(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("Renderer");
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

    // Point count is a *render* property, not scene data: it's edited here in
    // millions, persisted to prefs, and applied explicitly — raising it
    // reallocates the point buffer and restarts the chaos-game warmup, which
    // is not something to do live on every drag frame.
    let applied = app.point_capacity();
    let max_m = app.max_point_capacity() as f32 / 1e6;
    // Held in a local for the duration of the row so the widget closures can
    // still take `&mut App`; written back to `UiState` at the end.
    let mut pending = app
        .ui_state
        .pending_point_count
        .unwrap_or(applied as f32 / 1e6);

    ui.horizontal(|ui| {
        ui.label("points");
        let resp = ui.add(
            egui::DragValue::new(&mut pending)
                .speed(0.05)
                .range(0.1..=max_m as f64)
                .suffix("M"),
        );
        let pending_value = pending;
        let resp = hinted(
            resp,
            &mut app.ui_state,
            format!(
                "Points held in flight by the chaos game (max {:.1}M on this GPU). Applied on demand — it reallocates the buffer and restarts warmup.",
                max_m
            ),
            "drag: choose a point count · click: type it",
        );
        let _ = resp;

        let target = (pending_value * 1e6).round() as u32;
        let dirty = target != applied;
        let resp = ui.add_enabled(dirty, egui::Button::new("Apply"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if dirty {
                "Reallocate the point buffer and restart warmup"
            } else {
                "Already at this count"
            },
            "click: apply the new point count",
        );
        if resp.clicked() {
            app.set_point_capacity(target);
            // Snap back to whatever the clamp actually gave us.
            pending = app.point_capacity() as f32 / 1e6;
        }
    });

    app.ui_state.pending_point_count = Some(pending);
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

fn draw_fog(ui: &mut egui::Ui, app: &mut App) {
    ui.label(egui::RichText::new("Fog").strong());

    ui.horizontal(|ui| {
        let mut near = app.fog_near;
        let resp = ui.add(
            egui::DragValue::new(&mut near)
                .speed(0.1)
                .range(0.1..=(app.fog_far - 1.0) as f64)
                .prefix("near: "),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Distance where fog starts (N / Shift+N)",
            "drag: move the fog's near plane",
        );
        if resp.changed() {
            app.fog_near = near.clamp(0.1, app.fog_far - 1.0);
        }

        let mut far = app.fog_far;
        let resp = ui.add(
            egui::DragValue::new(&mut far)
                .speed(0.1)
                .range((app.fog_near + 1.0) as f64..=30.0)
                .prefix("far: "),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Distance where fog reaches full strength (M / Shift+M)",
            "drag: move the fog's far plane",
        );
        if resp.changed() {
            app.fog_far = far.clamp(app.fog_near + 1.0, 30.0);
        }
    });

    let mut brightness = app.fog_brightness;
    let resp = ui.add(egui::Slider::new(&mut brightness, 0.05..=1.0).text("brightness"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "How much brightness survives at the fog's far plane — 1.0 is no fog (F / Shift+F moves both)",
        "drag: adjust fog brightness falloff",
    );
    if resp.changed() {
        app.fog_brightness = brightness.clamp(0.05, 1.0);
    }

    let mut saturation = app.fog_saturation;
    let resp = ui.add(egui::Slider::new(&mut saturation, 0.05..=1.0).text("saturation"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "How much color survives at the fog's far plane — 1.0 is no desaturation (F / Shift+F moves both)",
        "drag: adjust fog saturation falloff",
    );
    if resp.changed() {
        app.fog_saturation = saturation.clamp(0.05, 1.0);
    }
}
