//! The Views window: the framing you're looking at now, and the views this
//! scene has saved.
//!
//! It is what was left of the Camera window once the Timeline took the
//! camera's path (keys, length, loop) and Settings took the field of view.
//! *This view* is the framing as numbers, for whoever wants it written down;
//! *Saved views* are the files under `views/` that V writes.
//!
//! Moving the camera isn't an edit to the artwork, so nothing here is a
//! history entry: nobody expects Ctrl+Z to un-orbit a camera, and a drag
//! produces thousands of intermediate states, none of which are decisions.

use crate::app::App;

use super::hints::hinted;
use super::icons;
use super::num;

/// Character budget for the `yaw · pitch · focus` readout.
const FRAMING_CHARS: usize = 46;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.views_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Views, "Views")
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("This view").strong());
            draw_framing(ui, app);
            ui.separator();
            draw_views(ui, app);
        });

    super::remember(ctx, app, super::WindowKey::Views);
    app.ui_state.panels.views_open = open;
}

fn draw_framing(ui: &mut egui::Ui, app: &mut App) {

    ui.horizontal(|ui| {
        let mut distance = app.camera.distance;
        // `distance` is 0.05..=100.0, so "distance: 100.00" is the widest it
        // gets — 16 characters, and the field is that wide whatever's in it, so
        // the roll field and the level button beside it hold still.
        let resp = num::drag(
            ui,
            16,
            2,
            num::Drag::new(&mut distance)
                .speed(0.02)
                .range(0.05..=100.0)
                .prefix("distance: "),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "How far the camera is from what it's looking at. Scrolling and \
             the Up and Down keys change it too",
            "drag: move the camera closer or further",
        );
        if resp.changed() {
            app.camera.distance = distance.clamp(0.05, 100.0);
            // Setting the framing by hand means taking the camera off the path,
            // the same as dragging in the viewport does.
            app.stop_camera_motion();
        }

        // Roll's home. Right-drag sets it, but a gesture with no readout
        // can't be undone precisely, and "level" is a thing you want back
        // exactly rather than approximately. Looking straight up or down,
        // yaw and roll are one control and the number here means nothing on
        // its own, so the field greys out rather than scrambling the framing.
        let faithful = app.camera.orientation.chart_is_faithful();
        let mut roll_deg = app.camera.chart().roll.degrees();
        let resp = ui.add_enabled_ui(faithful, |ui| {
            num::drag(
                ui,
                14,
                1,
                num::Drag::new(&mut roll_deg).speed(0.5).suffix("°").prefix("roll: "),
            )
        })
        .inner;
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if faithful {
                "Tilt the view sideways. Right-dragging the viewport does this too"
            } else {
                "Not available when looking straight up or down. Right-drag the \
                 viewport to spin the view instead"
            },
            "drag: roll the camera",
        );
        if resp.changed() {
            app.set_camera_roll(roll_deg.to_radians());
        }

        let resp = ui.add_enabled(!app.camera.is_level(), egui::Button::new((icons::TARGET, "level")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Make the horizon level again",
            "click: level the camera",
        );
        if resp.clicked() {
            app.level_camera();
        }
    });

    // Five signed values that all cross zero while you orbit — the single
    // worst vibration source outside the status bar, because every one of them
    // flicks a minus sign in and out at whatever rate you're dragging.
    let chart = app.camera.chart();
    let focus = app.camera.focus;
    let text = format!(
        "yaw {} · pitch {} · focus ({}, {}, {})",
        num::cell(chart.yaw.radians(), 2, 5),
        num::cell(chart.pitch.radians(), 2, 5),
        num::cell(focus.x, 2, 6),
        num::cell(focus.y, 2, 6),
        num::cell(focus.z, 2, 6),
    );
    num::small_cell(ui, &text, FRAMING_CHARS);
}

fn draw_views(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Saved views").strong());
        let resp = ui.button((icons::SAVE, "Save current"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Save the current framing, with the point size, haze and colour \
             settings, to the views folder (V)",
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

    // An editor left open on a view that is no longer in this list — the scene
    // changed under it — belongs to nothing, so drop it.
    if let Some((p, _)) = &app.ui_state.renaming_view {
        if !views.iter().any(|(_, path)| path == p) {
            app.ui_state.renaming_view = None;
        }
    }

    let mut load: Option<std::path::PathBuf> = None;
    let mut rename: Option<(std::path::PathBuf, String)> = None;
    egui::ScrollArea::vertical()
        .id_salt("fracturize_saved_views")
        // The list takes what the window has left: it's the one thing here
        // that grows.
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (name, path) in &views {
                if app.ui_state.renaming_view.as_ref().is_some_and(|(p, _)| p == path) {
                    // Same in-place editor the Transforms pane header uses:
                    // the name becomes a field where it is read, Enter or
                    // click-away commits, Esc cancels.
                    let resp = {
                        let (_, buf) = app.ui_state.renaming_view.as_mut().unwrap();
                        ui.add(
                            egui::TextEdit::singleline(buf)
                                .id(egui::Id::new(("fracturize_view_name", path)))
                                .hint_text("name this view")
                                .desired_width(f32::INFINITY),
                        )
                    };
                    resp.request_focus();
                    let enter = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                    let escape = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                    if escape {
                        app.ui_state.renaming_view = None;
                        app.ui_state.view_rename_error = None;
                    } else if enter || resp.lost_focus() {
                        if let Some((p, text)) = app.ui_state.renaming_view.take() {
                            rename = Some((p, text));
                        }
                    }
                    continue;
                }

                let shown = crate::app::view_display_name(name);
                let resp = ui.add(egui::Button::new(shown.as_str()).frame(false));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    path.display().to_string(),
                    "click: load this view · right-click: rename",
                );
                resp.context_menu(|ui| {
                    if ui.button((icons::PENCIL, "Rename")).clicked() {
                        // A name that is still the birth timestamp is not a
                        // name, so the field starts empty rather than
                        // prefilled with ten digits to delete first.
                        let seed = if name.parse::<u64>().is_ok() {
                            String::new()
                        } else {
                            name.clone()
                        };
                        app.ui_state.renaming_view = Some((path.clone(), seed));
                        app.ui_state.view_rename_error = None;
                        ui.close();
                    }
                });
                if resp.clicked() {
                    load = Some(path.clone());
                }
            }
        });
    if let Some((path, text)) = rename {
        app.ui_state.view_rename_error = app.rename_saved_view(&path, &text).err();
    }
    if let Some(err) = &app.ui_state.view_rename_error {
        ui.label(
            egui::RichText::new(err.clone())
                .small()
                .color(super::theme::warn(ui.visuals())),
        );
    }
    if let Some(path) = load {
        app.load_saved_view(&path);
    }
}
