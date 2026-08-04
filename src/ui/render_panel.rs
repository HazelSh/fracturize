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
            draw_zoom_band(ui, app);
            ui.separator();
            draw_output(ui, app);
        });

    super::remember(ctx, app, super::WindowKey::Render);
    app.ui_state.panels.render_open = open;
}

/// The points/splat segmented radio, shared by this panel and the toolbar.
///
/// A radio rather than two toggles: there are exactly two renderers and the
/// point buffer is always going through one of them, so "neither" isn't a
/// state this control can be in. See `ui::radio`.
pub fn render_mode(ui: &mut egui::Ui, app: &mut App) {
    let chosen = super::radio::radio(&mut app.ui_state, "render_mode", app.render_mode)
        .option(
            RenderMode::Points,
            "points",
            "Opaque depth-tested points — crisp, dusty edges (R)",
            "click: switch to the points renderer",
        )
        .option(
            RenderMode::Splat,
            "splat",
            "Additive log-density accumulation — smoother tonemapping, exposure applies (R)",
            "click: switch to the splat renderer",
        )
        .show(ui);
    if let Some(mode) = chosen {
        app.set_render_mode(mode);
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

/// Colour: the source (stage 2) and the two accumulation knobs around it.
///
/// `color_contrast` is drawn by `gradient::draw` rather than here, directly
/// under the strip showing what it does to the gradient. It used to sit with
/// the other sliders, where a palette compressed into an arc of itself looked
/// like a broken palette rather than a contrast setting.
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

    super::gradient::draw(ui, app);
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

/// The infinite-zoom band: how much of the attractor is rendered, over how
/// many octaves, and what happens at the outer edge.
///
/// Lives here rather than in the Camera window because it is a statement about
/// what gets drawn, not about where the eye is — and because it wants to sit
/// next to haze, which is the other half of whether the band's edge is
/// visible. *Which* map carries the zoom is still chosen in the Transforms
/// window, since that is a property of one map.
///
/// Greyed rather than hidden when the scene has no zoom map, for the reason
/// spelled out on the camera panel's zoom-loop row: a control that vanishes
/// can't tell you the feature exists.
fn draw_zoom_band(ui: &mut egui::Ui, app: &mut App) {
    ui.collapsing("infinite zoom", |ui| {
        if let Some(err) = app.zoom_error.clone() {
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color).small());
            return;
        }
        let (Some(spec), Some(z)) = (app.zoom_spec().cloned(), app.zoom().copied()) else {
            ui.add_enabled_ui(false, |ui| {
                ui.label("no zoom map");
            });
            ui.label(
                egui::RichText::new("Transforms window → right-click a map → Zoom about this")
                    .small()
                    .weak(),
            );
            return;
        };
        let mut next = spec.clone();

        ui.label(egui::RichText::new(format!(
            "transform {} · {:.2} octaves per period · {:.0}° twist",
            z.map,
            z.log_scale / std::f32::consts::LN_2,
            z.twist_degrees()
        )).small().weak());

        // The headline control, and the reason this section exists. See
        // `renorm::DEFAULT_OCTAVE_FADE` for the measurements behind the
        // wording — in particular that it does not make the wrap's total
        // brightness step smaller, it spreads it out so there is no
        // discontinuity in any one place.
        let resp = ui.add(
            egui::Slider::new(&mut next.octave_fade, 0.0..=6.0)
                .fixed_decimals(1)
                .text("edge fade"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Octaves over which the band's outer edge thins out instead of \
             stopping dead. At 0 the outermost octave vanishes in one frame at \
             every wrap, taking a recognisable slab of structure with it. \
             Winding it up spreads that same change across the outer octaves, \
             so nothing cuts — the picture dims a little instead.",
            "drag: widen or narrow the outer fade",
        );
        if resp.changed() {
            app.set_zoom_spec(next.clone());
        }
        ui.label(egui::RichText::new(if z.fade_periods >= 1.0 {
            format!(
                "outer {:.1} octaves faded, x{:.2} per period",
                z.fade_periods * z.log_scale / std::f32::consts::LN_2,
                z.fade_g
            )
        } else {
            "hard edge".to_string()
        }).small().weak());

        ui.collapsing("band size", |ui| {
            let mut next = spec.clone();
            let resp = ui.add(
                egui::Slider::new(&mut next.radius, 1.0..=12.0)
                    .fixed_decimals(2)
                    .text("radius"),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Outer radius of the band, in camera distances. Not a look \
                 control: below 2.42 its edge enters the frustum and material \
                 blinks out at every wrap. Raising it past that only helps a \
                 scene with haze — the rendered set is scale-invariant, so the \
                 outermost octave looks the same however far out you put it.",
                "drag: resize the band's outer edge",
            );
            if resp.changed() {
                app.set_zoom_spec(next.clone());
            }
            if !z.band_covers_the_view() {
                ui.label(
                    egui::RichText::new(format!(
                        "below {:.2} — the edge is inside the picture",
                        crate::renorm::MIN_RADIUS
                    ))
                    .color(ui.visuals().error_fg_color)
                    .small(),
                );
            }

            let mut next = spec.clone();
            let resp = ui.add(
                egui::Slider::new(&mut next.levels, 4.0..=24.0)
                    .fixed_decimals(1)
                    .text("levels"),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Octaves of scale rendered below the outer radius. Deeper means \
                 you can zoom further before the core empties out, and every \
                 octave thinner for the same point budget.",
                "drag: change how deep the band goes",
            );
            if resp.changed() {
                app.set_zoom_spec(next.clone());
            }
            ui.label(
                egui::RichText::new(format!("{:.0} zoom periods of this map", z.periods))
                    .small()
                    .weak(),
            );

            let mut next = spec.clone();
            let resp = ui.add(
                egui::Slider::new(&mut next.octave_falloff, 0.0..=3.0)
                    .fixed_decimals(1)
                    .text("octave falloff"),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Point budget falling off toward the fixed point — the opposite \
                 end of the band from the edge fade above, and it thins the \
                 middle of the picture rather than its rim. Leave at 0 for \
                 anything that will be flown: it makes neighbouring octaves hold \
                 different numbers of points, so the density jumps every time the \
                 camera wraps. Useful for a still, which never wraps.",
                "drag: bias the point budget inward",
            );
            if resp.changed() {
                app.set_zoom_spec(next);
            }
        });
    });
}
