//! Top toolbar: a thin strip of icon toggle buttons for the six panel
//! windows, then the quick controls — the handful of settings you reach for
//! often enough that opening a panel to get at them is friction.
//!
//! Quick controls delegate to the panels' own widgets (`render_panel::
//! render_mode`, `render_panel::point_count`) rather than reimplementing
//! them, so the rate limiting, hints and persistence can't drift between the
//! two places you can change the same value from.

use crate::app::App;

use super::hints::hinted;
use super::icons;
use super::render_panel;

pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("fracturize_toolbar").show(ui, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);

            draw_file_menu(ui, app);
            ui.separator();

            // Ordered by task, not by module: what am I working on, then how am
            // I looking at it, then help. Gizmos sits with Transforms because
            // it's a *view of the same object*, not a peer of the panel
            // toggles; Keybinds is pushed to the right end, where help lives in
            // every toolbar ever made.
            let resp = ui.toggle_value(&mut app.ui_state.panels.transforms_open, (icons::SHAPES, "Transforms"));
            hinted(
                resp,
                &mut app.ui_state,
                "Transforms — list + inspector",
                "toggle the Transforms window",
            );

            // Not a window toggle, but the same kind of thing: "show me this
            // layer of the interface or don't". It was labelled "Edit", which
            // next to a row of window toggles reads as an Edit *menu*, and to
            // anyone arriving from Blender reads as Edit *Mode* — a large
            // concept that doesn't exist here. It shows gizmos. Call it that.
            let resp = ui.selectable_label(app.show_gizmos, (icons::CUBE, "Gizmos"));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Show transform gizmos + name labels (G)",
                "toggle the transform gizmos",
            );
            if resp.clicked() {
                app.toggle_gizmos();
            }

            ui.separator();

            let resp = ui.toggle_value(&mut app.ui_state.panels.explore_open, (icons::FLASK, "Explore"));
            hinted(
                resp,
                &mut app.ui_state,
                "Explore — mutate, undo, strength",
                "toggle the Explore window",
            );

            ui.separator();

            let resp = ui.toggle_value(&mut app.ui_state.panels.camera_open, (icons::VIDEO_CAMERA, "Camera"));
            hinted(
                resp,
                &mut app.ui_state,
                "Camera — framing, saved views, the camera path",
                "toggle the Camera window",
            );

            let resp = ui.toggle_value(&mut app.ui_state.panels.render_open, (icons::SLIDERS, "Render"));
            hinted(
                resp,
                &mut app.ui_state,
                "Render — renderer, point size + count, color, haze, infinite zoom",
                "toggle the Render window",
            );

            ui.separator();
            draw_undo_redo(ui, app);
            ui.separator();
            draw_quick_controls(ui, app);

            // Right-aligned, and therefore built right-to-left: help at the far
            // end, then the scene identity beside it.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let resp = ui.selectable_label(app.show_help, (icons::KEYBOARD, "Help"));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Every keybind, in a clickable list — the rows run their binding (H or F1)",
                    "toggle the keybind reference",
                );
                if resp.clicked() {
                    app.toggle_help();
                }
                ui.separator();
                draw_scene_identity(ui, app);
            });
        });
        ui.add_space(2.0);
    });
}

/// The File menu.
///
/// A menu, and the only one — not the first entry of a File/Edit/View/Help bar.
/// The scene is a **document**, and the whole value of that abstraction is that
/// people can port intuition from every file-editing program they have used;
/// but intuition needs furniture to attach to, and this is the furniture that
/// says so. What it deliberately isn't is a menu *bar*: Edit and View would be
/// near-empty duplicates of the toolbar toggles and the Explore window, and a
/// menu bar that is mostly empty teaches people that menus here aren't worth
/// opening.
///
/// File operations are numerous, conventional and infrequent — exactly the
/// trade a menu is right for. Undo and redo are the opposite trade, so they get
/// visible buttons instead (`draw_undo_redo`).
///
/// These four used to live in a row at the bottom of the *Camera* window, where
/// three of them had nothing to do with the camera. They ended up there because
/// Camera was the window with a bottom panel free, which is an implementation
/// reason rather than a task one.
fn draw_file_menu(ui: &mut egui::Ui, app: &mut App) {
    let resp = ui.button((icons::LIST, "File"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "New, open, save, and what leaves the app",
        "click: the File menu",
    );

    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(210.0);

        if menu_item(ui, app, "New", "Ctrl+N", "Start over on an empty canvas. An edit like any other — one Ctrl+Z brings back what was on screen.") {
            app.new_blank_scene();
        }
        if menu_item(
            ui,
            app,
            &format!("{} Open…", icons::FOLDER_OPEN),
            "Ctrl+O",
            "Browse scenes/ and load one. Asks first if this scene has unsaved edits.",
        ) {
            app.toggle_browser();
        }
        if menu_item(ui, app, "Save", "Ctrl+S", "Write the scene — transforms, colours, camera framing, path — back to its TOML file.") {
            app.save_scene();
        }
        if menu_item(ui, app, "Save as…", "Ctrl+Shift+S", "Fork the scene: write it under a new name and keep working on the copy, leaving the original as it was.") {
            super::save_as::open(app);
        }

        ui.separator();

        if menu_item(ui, app, "Screenshot", "S", "Capture the viewport as it is, to screenshots/.") {
            app.request_screenshot();
        }
        if menu_item(ui, app, "Render job…", "P", "Set up a batch render — still or animation, its own quality settings — with a cost estimate before you agree to it, progress, and a way to stop it.") {
            super::render_job::open(app);
        }

        ui.separator();

        if menu_item(ui, app, "Quit", "Ctrl+Q", "Leave. Asks first if this scene has unsaved edits.") {
            app.request_quit();
        }
    });
}

/// One File-menu row: label on the left, its accelerator greyed on the right.
fn menu_item(ui: &mut egui::Ui, app: &mut App, label: &str, key: &str, tip: &str) -> bool {
    let resp = ui
        .horizontal(|ui| {
            let resp = ui.button(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(key).weak().small());
            });
            resp
        })
        .inner;
    let resp = hinted(resp, &mut app.ui_state, tip, "click: run this command");
    if resp.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

/// Undo and redo, as visible buttons.
///
/// Undo is the one control people reach for with the mouse *even when they know
/// the shortcut*, because it gets used at exactly the moment confidence is low
/// and something has just gone wrong. A creative tool with no undo button in its
/// toolbar is unusual. Disabled rather than hidden when there's nothing to
/// undo, with the tooltip saying so — the house convention.
fn draw_undo_redo(ui: &mut egui::Ui, app: &mut App) {
    let (undo_n, redo_n) = (app.history.undo_len(), app.history.redo_len());
    let next_undo = app.history.undo_display().next().map(str::to_string);
    let next_redo = app.history.redo_display().last().map(str::to_string);

    let resp = ui.add_enabled(undo_n > 0, egui::Button::new("Undo"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        match &next_undo {
            Some(l) => format!("Undo “{}” (Ctrl+Z)", l),
            None => "Nothing to undo yet — edits appear here as you make them".to_string(),
        },
        "click: undo the last edit",
    );
    if resp.clicked() {
        app.undo();
    }

    let resp = ui.add_enabled(redo_n > 0, egui::Button::new("Redo"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        match &next_redo {
            Some(l) => format!("Redo “{}” (Ctrl+Shift+Z or Ctrl+Y)", l),
            None => "Nothing to redo — undo something first".to_string(),
        },
        "click: redo the last undone edit",
    );
    if resp.clicked() {
        app.redo();
    }
}

/// The scene's name and author: a readout that opens an editor for itself.
fn draw_scene_identity(ui: &mut egui::Ui, app: &mut App) {
    let author = app.scene.author.trim().to_string();
    // The same `*` the window title carries, for the same reason: whether
    // there is work on screen that isn't on disk is worth being able to see
    // without opening anything.
    let dirty = if app.is_dirty() { "*" } else { "" };
    let label = if author.is_empty() {
        format!("{}{}", dirty, app.scene.name)
    } else {
        format!("{}{} — {}", dirty, app.scene.name, author)
    };
    let resp = ui.add(
        egui::Button::new(egui::RichText::new(label).weak())
            .frame(false)
            .truncate(),
    );
    let path = app.scene_path.clone().unwrap_or_else(|| "(unsaved scene)".to_string());
    let resp = hinted(
        resp,
        &mut app.ui_state,
        format!("{}\n\nClick to rename the scene or set its author.", path),
        "click: edit the scene's name and author",
    );

    egui::Popup::menu(&resp)
        // A menu that closed on the first click inside would dismiss itself
        // the moment you put the caret in a field.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(260.0)
        .show(|ui| {
            let mut name = app.scene.name.clone();
            ui.horizontal(|ui| {
                ui.label("Name");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY),
                );
                hinted(
                    resp,
                    &mut app.ui_state,
                    "Shown here, and used for screenshot / render / view filenames",
                    "type: rename the scene",
                );
            });
            app.set_scene_name(&name);

            let mut author = app.scene.author.clone();
            ui.horizontal(|ui| {
                ui.label("By");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut author).desired_width(f32::INFINITY),
                );
                hinted(
                    resp,
                    &mut app.ui_state,
                    "Written into the scene file. Remembered as the default for scenes \
                     you start from here (a blank canvas, a random flame).",
                    "type: set the author",
                );
            });
            app.set_scene_author(&author);

            ui.label(
                egui::RichText::new("Ctrl+S writes both to the scene file.")
                    .small()
                    .weak(),
            );
        });
}

/// Renderer mode, point count and camera transport — the three settings that
/// get changed most often mid-exploration.
fn draw_quick_controls(ui: &mut egui::Ui, app: &mut App) {
    render_panel::render_mode(ui, app);

    // A readout, not a slider: the toolbar has no room for a log slider wide
    // enough to be draggable, and a cramped one would be worse than none. The
    // popup gets the real widget.
    let capacity = app.pending_point_capacity().unwrap_or(app.point_capacity());
    let label = if capacity < 1_000_000 {
        format!("{:.0}k pts", capacity as f32 / 1e3)
    } else {
        format!("{:.1}M pts", capacity as f32 / 1e6)
    };
    let resp = ui.button(label);
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Points the chaos game keeps in flight — click for the slider",
        "click: open the point count slider",
    );
    egui::Popup::menu(&resp)
        // The default menu behaviour closes on any click inside, which would
        // dismiss the popup the instant you grabbed the slider.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(280.0)
        .show(|ui| render_panel::point_count(ui, app));

    let moving = app.camera_moving();
    let resp = ui.button(if moving { (icons::PAUSE, "Pause") } else { (icons::PLAY, "Play") });
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if moving {
            "Stop the camera moving (O, or Z for a path)"
        } else {
            "Start the camera moving (O, or Z for a path)"
        },
        if moving {
            "click: stop the camera"
        } else {
            "click: start the camera moving"
        },
    );
    if resp.clicked() {
        app.toggle_camera_motion();
    }
}
