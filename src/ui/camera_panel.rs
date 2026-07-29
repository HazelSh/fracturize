//! Camera window: framing, saved views, camera paths, and the output actions
//! that capture what's on screen (screenshot, HQ render, save scene).
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

    egui::Window::new("Camera")
        .id(egui::Id::new("fracturize_camera_window"))
        .open(&mut open)
        .default_pos(egui::pos2(600.0, 60.0))
        .default_width(300.0)
        .show(ctx, |ui| {
            draw_framing(ui, app);
            ui.separator();
            draw_views(ui, app);
            ui.separator();
            draw_path(ui, app);
            ui.separator();
            draw_output(ui, app);
        });

    app.ui_state.panels.camera_open = open;
}

fn draw_framing(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let mut paused = app.orbit_paused;
        let resp = ui.checkbox(&mut paused, "pause orbit");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Stop the automatic turntable so the framing holds still (O)",
            "click: pause / resume the auto-orbit",
        );
        if resp.changed() {
            app.toggle_orbit();
        }

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
            app.orbit_paused = true;
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

    let views = app.saved_views();
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
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Camera path").strong());
        let resp = ui.button("+ Add key");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Append the current framing as a path keypoint (Y)",
            "click: add a keypoint here",
        );
        if resp.clicked() {
            app.add_path_key();
        }
    });

    let Some(path) = &app.scene.camera_path else {
        ui.label(
            egui::RichText::new("No path yet — add two or more keypoints to fly between them.")
                .small()
                .weak(),
        );
        return;
    };

    // Snapshot what the rows need before the loop, so the row bodies are free
    // to call `&mut App` methods.
    let keys: Vec<(f32, f32, f32)> = path
        .keys
        .iter()
        .map(|k| (k.distance, k.yaw, k.pitch))
        .collect();
    let mut closed = path.closed;
    let seconds = path.duration();
    let explicit_seconds = path.seconds.is_some();
    let playing = app.path_playing();

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

    ui.horizontal(|ui| {
        let can_play = keys.len() >= 2;
        let resp = ui.add_enabled(
            can_play,
            egui::Button::new(if playing { "Stop" } else { "Play" }),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if can_play {
                "Fly the Catmull-Rom spline through the keypoints in real time (Z)"
            } else {
                "Needs at least two keypoints"
            },
            "click: play / stop the flythrough",
        );
        if resp.clicked() {
            app.toggle_path_play();
        }

        let resp = ui.checkbox(&mut closed, "loop");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Close the path back to its first key, for seamless loops (Ctrl+Y)",
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
                "Playback duration for the whole path"
            } else {
                "Playback duration — currently the default of 3s per segment; setting it here pins it"
            },
            "drag: set the flythrough duration",
        );
        if resp.changed() {
            app.set_path_seconds(Some(secs));
        }
    });
}

fn draw_output(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let busy = app.hq_render_busy();
        let resp = ui.add_enabled(!busy, egui::Button::new("HQ render"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if busy {
                "A high-quality render is already running"
            } else {
                "Render this exact view at high quality into renders/, on a background GPU device (P)"
            },
            "click: start a high-quality render",
        );
        if resp.clicked() {
            app.start_hq_render();
        }
        if busy {
            ui.spinner();
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
    });
}
