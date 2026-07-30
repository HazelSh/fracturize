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
            draw_framing(ui, app);
            ui.separator();
            draw_views(ui, app);
            ui.separator();
            draw_path(ui, app);
            ui.separator();
            draw_output(ui, app);
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
            "Write the current framing (plus point size, fog and color) to views/ (V)",
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
    let mut closed = app.camera_path().closed;
    let seconds = app.camera_path().duration();
    let explicit_seconds = app.camera_path().seconds.is_some();
    let moving = app.camera_moving();

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

    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("fracturize_path_keys")
        .max_height(110.0)
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
                        let resp = ui.add(egui::Button::new(super::icons::X).small().frame(false));
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
        return;
    }

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

        let resp = ui.checkbox(&mut closed, "loop");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Close the path back to its first key, for seamless loops (Ctrl+Y). \
             Setting this on the default orbit makes it this scene's own path.",
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
