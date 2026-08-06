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
            // Grouped by *persistence class*, with headings that say so. This
            // panel stacks three different kinds of value with nothing between
            // them admitting it: the renderer mode evaporates at exit, the
            // point count follows the person across every scene they open, and
            // the rest goes in the file. Nothing on screen used to distinguish
            // a slider whose value will be in your scene tomorrow from one that
            // won't — and that gap is what let `exposure` go years without
            // being saved at all.
            //
            // Grouped within the window rather than split across windows:
            // splitting would scatter things that belong together by task, and
            // the task is why you opened this panel.
            class_heading(ui, "Scene", "Saved with the scene by Ctrl+S, and undoable.");
            draw_renderer(ui, app);
            draw_point_size(ui, app);
            draw_color(ui, app);
            draw_haze(ui, app);
            draw_zoom_band(ui, app);
            draw_output(ui, app);

            ui.add_space(6.0);
            class_heading(
                ui,
                "Preference",
                "Follows you across scenes, saved to prefs — never written to a scene file.",
            );
            point_count(ui, app);
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
                "Splat exposure — the log-density renderer's brightness (W / Shift+W).\n\n\
                 Saved with the scene, and undoable."
            } else {
                "Only applies to the splat renderer"
            },
            "drag: adjust splat exposure",
        );
        if resp.changed() {
            app.set_exposure(exposure);
        }
    });
}

/// A persistence-class heading: the one line that answers "will this be in my
/// file tomorrow?", which the UI previously gave no way to ask.
fn class_heading(ui: &mut egui::Ui, title: &str, tip: &str) {
    ui.separator();
    let resp = ui
        .add(egui::Label::new(egui::RichText::new(title).strong().small()).sense(egui::Sense::hover()));
    resp.on_hover_text(tip);
}

fn draw_point_size(ui: &mut egui::Ui, app: &mut App) {
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
    // Point count used to follow straight on from point size — two sliders that
    // read as a pair and aren't one. It's a *preference*, so it sits under the
    // Preference heading at the foot of the panel now.
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

        // A pinned band does not survive a zoom wrap — it is nailed to world
        // space while the wrap rescales everything else — so infinite zoom
        // ignores it. Say so, rather than leaving two sliders that move and
        // change nothing.
        let overridden = app.haze_band_overridden();
        if overridden {
            ui.label(
                egui::RichText::new(
                    "auto-ranged: a pinned band can't follow the zoom's wrap",
                )
                .small()
                .weak(),
            );
        }

        // Shown disabled rather than hidden when auto: the resolved band is
        // worth being able to read, and hiding it would leave "pin the band"
        // with nothing to say what it would pin.
        let (mut near, mut far) = app.haze_range();
        ui.add_enabled_ui(pinned && !overridden, |ui| {
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
    // Open by default once the scene actually has a zoom map. A disclosure
    // should hide *elaboration*, never the headline — and with a zoom map in
    // play, the edge guard is the headline. Compare `haze`, which gets this
    // right: the amount is always visible and only the band's pin/near/far are
    // folded away. This section used to hide the entire feature, with the edge
    // guard one level in and the band size two.
    let has_map = app.zoom_spec().is_some();
    egui::CollapsingHeader::new("infinite zoom")
        .default_open(has_map)
        .show(ui, |ui| draw_zoom_body(ui, app));
}

/// Which transform carries the scale symmetry — drawn as the one-of-n choice
/// it actually is.
///
/// Picking the zoom map is a **choose-one-of-n over transforms**: only one map
/// can carry it. It used to be drawn as a lone `Button::selected` on whichever
/// transform you happened to have selected, with the other n−1 options not on
/// screen at all — the exact defect `ui::radio`'s module doc was written to
/// eliminate ("a lone toggle button says 'this is on or off'"), only spread
/// across *time* instead of space. You couldn't see the alternatives, and
/// nothing said only one could be lit.
///
/// And the app has always known which transforms qualify: `zoom_action` runs
/// `Renorm::build` per transform and produces both an enabled flag and a
/// sentence explaining the failure. So this turns "know the theory" into "read
/// the list", out of code that already existed. Non-qualifying rows are greyed
/// with their reason on hover, per the house convention.
fn draw_zoom_map_picker(ui: &mut egui::Ui, app: &mut App) {
    let n = app.scene.transforms.len();
    if n == 0 {
        return;
    }
    let current = app.zoom_map();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("map").strong());
        let resp = ui.add(egui::Button::new("off").selected(current.is_none()));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Render the attractor as itself — bounded, with a largest feature. \
             The ordinary way to render an IFS.",
            "click: turn infinite zoom off",
        );
        if resp.clicked() {
            app.set_zoom_map(None);
        }
    });

    let mut chosen: Option<usize> = None;
    let row_h = ui.spacing().interact_size.y;
    // Virtualized: `zoom_action` builds a `Renorm` per row, and an L-system
    // scene has tens of thousands of transforms. Only the rows actually on
    // screen get built.
    egui::ScrollArea::vertical()
        .id_salt("fracturize_zoom_map_picker")
        .max_height((row_h * 5.0).max(90.0))
        .auto_shrink([false, false])
        .show_rows(ui, row_h, n, |ui, range| {
            for i in range {
                let zoom = super::transforms::zoom_action(app, i);
                let label = super::transforms::transform_label(app, i);
                // Framed when selected — a *frameless* `Button::selected`
                // doesn't paint its selected fill, so the row that actually is
                // the scene's zoom map looked exactly like the rows that
                // aren't. Which is the whole thing this picker is for.
                let is_current = current == Some(i);
                let resp = ui.add_enabled(
                    zoom.enabled,
                    egui::Button::new(label).selected(is_current).frame(is_current),
                );
                let resp = hinted(resp, &mut app.ui_state, zoom.tooltip, zoom.hint);
                if resp.clicked() && current != Some(i) {
                    chosen = Some(i);
                }
            }
        });
    if let Some(i) = chosen {
        app.set_zoom_map(Some(i));
    }
    ui.label(
        egui::RichText::new("Greyed maps can't carry the symmetry — hover one to see why.")
            .small()
            .weak(),
    );
}

fn draw_zoom_body(ui: &mut egui::Ui, app: &mut App) {
    if let Some(err) = app.zoom_error.clone() {
        ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color).small());
        return;
    }

    draw_zoom_map_picker(ui, app);

    let (Some(spec), Some(z)) = (app.zoom_spec().cloned(), app.zoom().copied()) else {
        return;
    };
    let mut next = spec.clone();

    ui.label(egui::RichText::new(format!(
        "{:.2} octaves per period · {:.0}° twist",
        z.log_scale / std::f32::consts::LN_2,
        z.twist_degrees()
    )).small().weak());
    {
        // The headline control, and the reason this section exists. See
        // `renorm::DEFAULT_EDGE_GUARD` — in particular for why this one is a
        // render-time weight on the picture and not a taper on the point deal,
        // which is what it was and which put the whole of its change into the
        // one frame where the camera wraps.
        let resp = ui.add(
            egui::Slider::new(&mut next.edge_guard, 0.0..=4.0)
                .fixed_decimals(1)
                .text("edge guard"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Octaves over which the outer edge of the picture fades to \
             nothing, measured against the camera rather than the world. At 0 \
             the band simply stops, and every wrap drops a recognisable slab \
             of structure between two frames. Above 0 that material leaves \
             gradually and at a steady rate as you zoom, and the wrap itself \
             costs nothing. Wider than the band has room for is clamped.",
            "drag: widen or narrow the outer fade",
        );
        if resp.changed() {
            app.set_zoom_spec(next.clone());
        }
        ui.label(egui::RichText::new(match z.guard_span() {
            Some((start, end)) => format!(
                "outer {:.1} octaves guarded, {:.2}x-{:.2}x the eye distance",
                z.guard_width, start, end
            ),
            None => "hard edge — the wrap will step".to_string(),
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
    }
}
