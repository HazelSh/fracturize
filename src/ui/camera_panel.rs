//! Camera window: framing, saved views, the camera path, and the output actions
//! that capture what's on screen (screenshot, render job, save scene).
//!
//! There is exactly one path here, because there is exactly one path (see
//! `App::camera_path`): a scene's own keypoints when it has two or more,
//! otherwise the default full orbit. The panel doesn't distinguish them beyond
//! saying which you're looking at — the same list, the same transport, the same
//! loop and duration controls either way, and editing any of them is what turns
//! the default into scene data.
//!
//! None of the camera controls are history entries — per the Phase 3 design,
//! moving the camera isn't an edit to the artwork. Path keypoints *are* scene
//! data, but they follow the keyboard paths (Y / Shift+Y / Ctrl+Y), which
//! aren't history-wired either; Ctrl+S is what makes them permanent.

use crate::app::App;

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.camera_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Camera, "Camera")
        .open(&mut open)
        .show(ctx, |ui| {
            // The output row is pinned to the bottom so the keypoint list in
            // the middle gets every point of slack when the window is dragged
            // taller. Without this the window is content-sized and resizing it
            // just adds empty space under a 110pt list you still have to
            // scroll — which is the one thing here that's ever long.
            egui::Panel::bottom("fracturize_camera_output")
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    draw_output(ui, app);
                });
            egui::Panel::top("fracturize_camera_framing")
                .show(ui, |ui| {
                    draw_framing(ui, app);
                    ui.separator();
                    draw_views(ui, app);
                    ui.separator();
                });
            draw_path(ui, app);
        });

    super::remember(ctx, app, super::WindowKey::Camera);
    app.ui_state.panels.camera_open = open;
}

fn draw_framing(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let mut invert = app.invert_pitch();
        let resp = ui.checkbox(&mut invert, "invert pitch");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Flightsim-style: drag down to tilt the scene's top toward you. Saved to prefs (I)",
            "click: invert mouse pitch",
        );
        if resp.changed() {
            app.toggle_invert_pitch();
        }
    });

    ui.horizontal(|ui| {
        let mut distance = app.camera.distance;
        let resp = ui.add(
            egui::DragValue::new(&mut distance)
                .speed(0.02)
                .range(0.05..=100.0)
                .prefix("distance: "),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Orbit radius — the same value scroll and Up/Down change",
            "drag: dolly the camera in / out",
        );
        if resp.changed() {
            app.camera.distance = distance.clamp(0.05, 100.0);
            // Setting the framing by hand means taking the camera off the path,
            // the same as dragging in the viewport does.
            app.stop_camera_motion();
        }

        // Roll's home. Right-drag sets it, but a gesture with no readout
        // can't be undone precisely, and "level" is a thing you want back
        // exactly rather than approximately.
        let mut roll_deg = app.camera.roll.to_degrees();
        let resp = ui.add(
            egui::DragValue::new(&mut roll_deg)
                .speed(0.5)
                .suffix("°")
                .prefix("roll: "),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Rotation about the view axis — right-drag the viewport does this too",
            "drag: roll the camera",
        );
        if resp.changed() {
            app.set_camera_roll(roll_deg.to_radians());
        }

        let resp = ui.add_enabled(app.camera.roll != 0.0, egui::Button::new("level"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Put the horizon back exactly level",
            "click: level the camera",
        );
        if resp.clicked() {
            app.set_camera_roll(0.0);
        }
    });

    ui.label(
        egui::RichText::new(format!(
            "yaw {:.2} · pitch {:.2} · focus ({:.2}, {:.2}, {:.2})",
            app.camera.yaw,
            app.camera.pitch,
            app.camera.focus.x,
            app.camera.focus.y,
            app.camera.focus.z,
        ))
        .small()
        .weak(),
    );
}

fn draw_views(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Saved views").strong());
        let resp = ui.button("Save current");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Write the current framing (plus point size, haze and color) to views/ (V)",
            "click: save this view",
        );
        if resp.clicked() {
            app.save_view();
        }
    });

    // Cloned out of the cache so the row bodies below can take `&mut App`
    // (clicking one loads a view). The clone is a handful of small strings
    // and only happens while the panel is open.
    let views: Vec<(String, std::path::PathBuf)> = app.saved_views().to_vec();
    if views.is_empty() {
        ui.label(
            egui::RichText::new("No saved views for this scene yet.")
                .small()
                .weak(),
        );
        return;
    }

    let mut load: Option<std::path::PathBuf> = None;
    egui::ScrollArea::vertical()
        .id_salt("fracturize_saved_views")
        .max_height(110.0)
        .show(ui, |ui| {
            for (name, path) in &views {
                let resp = ui.add(egui::Button::new(name.as_str()).frame(false));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    path.display().to_string(),
                    "click: load this view",
                );
                if resp.clicked() {
                    load = Some(path.clone());
                }
            }
        });
    if let Some(path) = load {
        app.load_saved_view(&path);
    }
}

fn draw_path(ui: &mut egui::Ui, app: &mut App) {
    // Which path this is looking at. With no keys of its own the scene is on
    // the default orbit, and that's shown read-only: it isn't authored content,
    // it's derived from the framing, and it re-derives itself as you move. Your
    // first "+ Add key" starts a list of your own, which the ✕s can edit.
    let own: Option<Vec<(f32, f32, f32)>> = app.scene.camera_path.as_ref().map(|p| {
        p.keys.iter().map(|k| (k.distance, k.yaw, k.pitch)).collect()
    });
    let authored = own.is_some();
    let flying_default = app.path_is_default();

    let keys: Vec<(f32, f32, f32)> = own.unwrap_or_else(|| {
        app.camera_path()
            .keys
            .iter()
            .map(|k| (k.distance, k.yaw, k.pitch))
            .collect()
    });
    // Pinned below the list, so the list gets the slack from a taller window.
    egui::Panel::bottom("fracturize_camera_path_controls").show(ui, |ui| {
        ui.add_space(2.0);
        draw_path_controls(ui, app, flying_default, authored);
    });

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Camera path").strong());
        ui.label(
            egui::RichText::new(if authored { "(this scene's)" } else { "(default orbit)" })
                .small()
                .weak(),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.add_enabled(authored, egui::Button::new("Reset"));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if authored {
                    "Throw away this scene's keypoints and go back to the default full orbit"
                } else {
                    "Already on the default orbit"
                },
                "click: reset to the default orbit",
            );
            if resp.clicked() {
                app.reset_path_to_default();
            }

            let resp = ui.button("+ Add key");
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if authored {
                    "Append the current framing to this scene's path (Y)"
                } else {
                    "Start this scene's own path with the current framing (Y). \
                     The default orbit keeps flying until there are two keypoints."
                },
                "click: add a keypoint here",
            );
            if resp.clicked() {
                app.add_path_key();
            }
        });
    });

    if !authored {
        ui.label(
            egui::RichText::new(
                "A full turn around the current framing — every scene has this \
                 until it authors keypoints of its own.",
            )
            .small()
            .weak(),
        );
    }

    // Every remaining point of the window goes to the list — see `draw`'s
    // panel layout. `auto_shrink` off on the vertical axis is what makes the
    // scroll area claim that space rather than hugging its rows.
    //
    // The scrollbar is pinned into the layout rather than left floating: a
    // floating bar draws *over* the right edge of the content, which is
    // exactly where each row's ✕ is, so removing a keypoint from a list long
    // enough to scroll meant threading the pointer between the two.
    ui.spacing_mut().scroll.floating = false;
    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("fracturize_path_keys")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, (d, yaw, pitch)) in keys.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}. d={:.2} yaw={:.2} pitch={:.2}",
                            i + 1,
                            d,
                            yaw,
                            pitch
                        ))
                        .small(),
                    );
                    if !authored {
                        return;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // A deliberate hit target rather than a glyph's own
                        // ink: this is a destructive button in a scrolling
                        // list, and it should be easy to hit on purpose.
                        let resp = ui.add(
                            egui::Button::new(super::icons::X)
                                .frame(false)
                                .min_size(egui::vec2(22.0, 18.0)),
                        );
                        let resp = hinted(
                            resp,
                            &mut app.ui_state,
                            "Remove this keypoint",
                            "click: remove this keypoint",
                        );
                        if resp.clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
        });
    if let Some(i) = remove {
        app.remove_path_key_at(i);
    }
}

/// The path's transport and shape controls, pinned below the keypoint list.
fn draw_path_controls(ui: &mut egui::Ui, app: &mut App, flying_default: bool, authored: bool) {
    // One key of your own isn't a path yet, and the list above doesn't show the
    // orbit that's actually flying — so say which is which.
    if authored && flying_default {
        ui.label(
            egui::RichText::new(
                "One more keypoint and this flies instead of the default orbit.",
            )
            .small()
            .color(ui.visuals().warn_fg_color),
        );
    }

    let moving = app.camera_moving();
    let mut closed = app.camera_path().closed;
    let seconds = app.camera_path().duration();
    let explicit_seconds = app.camera_path().seconds.is_some();
    let zoom_loop = app.path_zoom_loop();
    // A zoom loop closes under the scene's scale symmetry, so it only exists
    // when there is one to close under.
    let zoom = app.zoom().map(|z| (z.map, z.log_scale / std::f32::consts::LN_2));

    ui.horizontal(|ui| {
        let resp = ui.button(if moving { "Stop" } else { "Play" });
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Fly the path in real time (O or Z). While it plays the route isn't \
             drawn — the camera is standing on it.",
            "click: play / stop the camera",
        );
        if resp.clicked() {
            app.toggle_camera_motion();
        }

        // Disabled rather than hidden while a zoom loop is on: the two are
        // different loops and only one can be the answer, and a control that
        // vanishes is harder to reason about than one that says why it's grey.
        let resp = ui.add_enabled(zoom_loop.is_none(), egui::Checkbox::new(&mut closed, "loop"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if zoom_loop.is_some() {
                "This path already loops, under the scene's zoom symmetry. \
                 Closing back to the first key would undo the descent that \
                 makes it endless — turn the zoom loop off to use this instead."
            } else {
                "Close the path back to its first key, for seamless loops (Ctrl+Y). \
                 Setting this on the default orbit makes it this scene's own path."
            },
            "click: toggle a closed loop",
        );
        if resp.changed() {
            app.toggle_path_closed();
        }

        let mut secs = seconds;
        let resp = ui.add(
            egui::DragValue::new(&mut secs)
                .speed(0.1)
                .range(0.1..=600.0)
                .suffix("s"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if explicit_seconds {
                "How long the whole path takes"
            } else {
                "How long the whole path takes — currently the default of 3s per segment; setting it here pins it"
            },
            "drag: set the path duration",
        );
        if resp.changed() {
            app.set_path_seconds(Some(secs));
        }
    });

    draw_zoom_loop(ui, app, zoom, zoom_loop);
}

/// The zoom-loop row: greyed, not hidden, when the scene has no scale
/// symmetry to close under.
///
/// It used to vanish, on the grounds that without a zoom map the control is
/// meaningless rather than merely unavailable. That was the wrong call, and
/// for the same reason the `loop` checkbox above is greyed instead of hidden:
/// a control that isn't there can't tell you it exists. Hiding it meant the
/// only way to discover infinite zoom from the Camera window was to already
/// know about it — so the greyed row names the feature and says where to turn
/// it on.
///
/// `S∞` is invariant under the renormalizing map, so a path whose last key is
/// its first carried forward by that map ends on the frame it started —
/// literally, not nearly. That makes an animation loop as an *endless* zoom.
/// See `path::ZoomLoop` and "Infinite Zoom" in AGENTS.md.
fn draw_zoom_loop(
    ui: &mut egui::Ui,
    app: &mut App,
    zoom: Option<(usize, f32)>,
    current: Option<crate::path::ZoomLoop>,
) {
    let Some((map, octaves)) = zoom else {
        ui.horizontal(|ui| {
            let mut off = false;
            let resp = ui.add_enabled(false, egui::Checkbox::new(&mut off, "zoom loop"));
            hinted(
                resp,
                &mut app.ui_state,
                "Loop by descending one zoom period instead of returning to the first \
                 key, so the animation plays as a zoom that never ends.\n\n\
                 Needs a scale symmetry to close under, and this scene has none yet. \
                 Select a transform in the Transforms window and press \"Zoom about \
                 this\" to nominate one.",
                // No arrow: the UI font has no U+2192, and a missing glyph in
                // the one line that tells you where to go is worse than a comma.
                "needs a zoom map — Transforms window, Zoom about this",
            );
        });
        return;
    };

    ui.horizontal(|ui| {
        let mut on = current.is_some();
        let resp = ui.checkbox(&mut on, "zoom loop");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            format!(
                "Close the loop under this scene's zoom symmetry (transform {}, \
                 {:.2} octaves per period) instead of by returning to the first \
                 key.\n\n\
                 One loop descends a whole period and ends on the frame it \
                 started — the same frame, not a similar one — so the animation \
                 plays as a zoom that never ends. One keypoint is enough, and \
                 gives a constant-rate descent with no seam.",
                map, octaves
            ),
            "click: loop by descending one zoom period",
        );
        if resp.changed() {
            app.set_path_zoom_loop(on.then_some(current.map_or(1, |z| z.periods)));
        }

        let mut periods = current.map_or(1, |z| z.periods);
        let suffix = if periods == 1 { " period" } else { " periods" };
        let resp = ui.add_enabled(
            current.is_some(),
            egui::DragValue::new(&mut periods).speed(0.05).range(1..=64).suffix(suffix),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            format!(
                "Zoom periods descended per loop — {:.2} octaves each, so {} of \
                 them is a factor of {:.0}. More makes a longer fall before it \
                 repeats; the loop is seamless either way.",
                octaves,
                periods,
                2f32.powf(octaves * periods as f32),
            ),
            "drag: periods descended per loop",
        );
        if resp.changed() {
            app.set_path_zoom_loop(Some(periods));
        }
    });
}

fn draw_output(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let resp = ui.button("Render job…");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Set up a batch render: still or animation, its own quality settings, \
             with estimates, progress and a way to stop it",
            "click: open the render job dialog",
        );
        if resp.clicked() {
            super::render_job::open(app);
        }

        let resp = ui.button("Screenshot");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Capture the viewport to screenshots/ (S)",
            "click: save a screenshot",
        );
        if resp.clicked() {
            app.request_screenshot();
        }

        let resp = ui.button("Save scene");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Write the scene — transforms, colors, camera framing, path — back to its TOML (Ctrl+S)",
            "click: save the scene file",
        );
        if resp.clicked() {
            app.save_scene();
        }

        let resp = ui.button("Save as…");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Fork the scene: write it under a new name and keep working on that copy, \
             leaving the original as it was (Ctrl+Shift+S)",
            "click: save the scene under a new name",
        );
        if resp.clicked() {
            super::save_as::open(app);
        }
    });
}
