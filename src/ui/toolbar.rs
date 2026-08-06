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

            let resp = ui.toggle_value(&mut app.ui_state.panels.transforms_open, (icons::SHAPES, "Transforms"));
            hinted(
                resp,
                &mut app.ui_state,
                "Transforms — list + inspector",
                "toggle the Transforms window",
            );

            let resp = ui.toggle_value(&mut app.ui_state.panels.explore_open, (icons::FLASK, "Explore"));
            hinted(
                resp,
                &mut app.ui_state,
                "Explore — mutate, undo, strength",
                "toggle the Explore window",
            );

            let resp = ui.toggle_value(&mut app.ui_state.panels.camera_open, (icons::VIDEO_CAMERA, "Camera"));
            hinted(
                resp,
                &mut app.ui_state,
                "Camera — framing, saved views, paths, render output",
                "toggle the Camera window",
            );

            let resp = ui.toggle_value(&mut app.ui_state.panels.render_open, (icons::SLIDERS, "Render"));
            hinted(
                resp,
                &mut app.ui_state,
                "Render — renderer, point size + count, color, haze",
                "toggle the Render window",
            );

            ui.separator();

            // Not a window toggle, but it belongs with them: it's the same kind
            // of thing — "show me this layer of the interface or don't" — and
            // it's the one such toggle with no home in a panel, since every
            // panel it might live in is a thing you'd have to open to reach it.
            let resp = ui.selectable_label(app.show_gizmos, (icons::CUBE, "Edit"));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Show transform gizmos + name labels (G)",
                "toggle the transform gizmos",
            );
            if resp.clicked() {
                app.toggle_gizmos();
            }

            let resp = ui.selectable_label(app.show_help, (icons::KEYBOARD, "Keybind Help"));
            let resp = hinted(resp, &mut app.ui_state, "Keybind reference (H)", "toggle the keybind help panel");
            if resp.clicked() {
                app.toggle_help();
            }

            let resp = ui.selectable_label(app.show_browser, (icons::FOLDER_OPEN, "Scene Browser"));
            let resp = hinted(resp, &mut app.ui_state, "Scene browser (B)", "toggle the scene browser");
            if resp.clicked() {
                app.toggle_browser();
            }

            ui.separator();
            draw_quick_controls(ui, app);

            // Scene identity, right-aligned — this used to be the HUD's
            // first line, which sat underneath this very panel. It's also the
            // editor for those two fields: the readout is the obvious place to
            // change what it reads, and there was nowhere else for the author
            // to live that wasn't a settings page for two strings.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                draw_scene_identity(ui, app);
            });
        });
        ui.add_space(2.0);
    });
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
