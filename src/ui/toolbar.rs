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
use super::num;
use super::render_panel;

pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("fracturize_toolbar").show(ui, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);

            draw_file_menu(ui, app);
            ui.separator();

            // Then the windows, ordered by task rather than by module: what am
            // I working on, then how am I looking at it. Help is pushed to the
            // right end, where it lives in every toolbar ever made.
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
        // Just wide enough for the longest row ("Save as…  Ctrl+Shift+S"), so
        // the accelerators have a column to line up in without the menu being
        // a slab of empty space.
        ui.set_min_width(168.0);

        if menu_item(ui, app, &format!("{} New", icons::FILE), "Ctrl+N", "Start over on an empty canvas. An edit like any other — one Ctrl+Z brings back what was on screen.") {
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
        if menu_item(ui, app, &format!("{} Save", icons::SAVE), "Ctrl+S", "Write the scene — transforms, colours, camera framing, path — back to its TOML file.") {
            app.save_scene();
        }
        if menu_item(ui, app, &format!("{} Save as…", icons::SAVE), "Ctrl+Shift+S", "Fork the scene: write it under a new name and keep working on the copy, leaving the original as it was.") {
            super::save_as::open(app);
        }

        ui.separator();

        if menu_item(ui, app, &format!("{} Screenshot", icons::CAMERA), "S", "Capture the viewport as it is, to screenshots/.") {
            app.request_screenshot();
        }
        if menu_item(ui, app, &format!("{} Render job…", icons::STACK), "P", "Set up a batch render — still or animation, its own quality settings — with a cost estimate before you agree to it, progress, and a way to stop it.") {
            super::render_job::open(app);
        }

        ui.separator();

        if menu_item(ui, app, &format!("{} Quit", icons::QUIT), "Ctrl+Q", "Leave. Asks first if this scene has unsaved edits.") {
            app.request_quit();
        }
    });
}

/// One File-menu row: label on the left, its accelerator greyed on the right,
/// and the *whole row* as one hit target that highlights at once.
///
/// `Button::shortcut_text` is the widget for this and egui already has it — it
/// pushes a growing atom between the label and the accelerator, so the row
/// fills the menu's width and the accelerators line up in a column. The first
/// version of this wrapped a button and a label in a `horizontal`, which made
/// the *button* the hit target rather than the row: a menu that behaved like a
/// column of little buttons, with a dead gutter beside each one.
///
/// No frame work needed here — `Popup::menu` applies `egui::menu::menu_style`,
/// which already strips the resting fill and leaves the hover highlight.
fn menu_item(ui: &mut egui::Ui, app: &mut App, label: &str, key: &str, tip: &str) -> bool {
    let resp = ui.add(egui::Button::new(label).shortcut_text(key));
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

    let resp = ui.add_enabled(undo_n > 0, egui::Button::new((icons::UNDO, "Undo")));
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

    let resp = ui.add_enabled(redo_n > 0, egui::Button::new((icons::REDO, "Redo")));
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
            // No "Ctrl+S writes these to the file" footnote: this is a file
            // editing program, so that is the paradigm and not news.
        });
}

/// Below this many points the toolbar readout prints "k", at or above it
/// "M" — the single source of truth for that boundary, shared by
/// `point_count_label` and the width bound computed from it, so the two can
/// never quietly disagree about where the format switches.
///
/// `pub(super)` (not `pub`) so `num.rs`'s width test can drive this exact
/// formatter with boundary values, rather than a hand-copied string that
/// could drift from what this function actually prints.
pub(super) const POINT_COUNT_K_M_BOUNDARY: u32 = 1_000_000;

/// The toolbar's point-count readout, e.g. `"420k pts"` or `"1.5M pts"`.
///
/// Its own function (rather than inlined at the one call site) so the width
/// bound in `draw_quick_controls` can ask it for boundary strings — the
/// actual widest output this formatter can produce, not a separately
/// maintained guess that would drift the moment this function changes.
pub(super) fn point_count_label(capacity: u32) -> String {
    if capacity < POINT_COUNT_K_M_BOUNDARY {
        format!("{:.0}k pts", capacity as f32 / 1e3)
    } else {
        format!("{:.1}M pts", capacity as f32 / 1e6)
    }
}

/// Renderer mode, point count, edit/view mode and camera transport — the
/// four settings that get changed most often mid-exploration.
fn draw_quick_controls(ui: &mut egui::Ui, app: &mut App) {
    render_panel::render_mode(ui, app);

    // A readout, not a slider: the toolbar has no room for a log slider wide
    // enough to be draggable, and a cramped one would be worse than none. The
    // popup gets the real widget.
    //
    // Its width is pinned too — the label alternates between "NNNk pts" and
    // "NN.NM pts" as the count changes, and this button sits immediately
    // left of Edit/View Mode and Play/Pause, so an unpinned width here would
    // still shift both of them every time the count crosses the k/M
    // boundary or gains a digit. The bound comes from `point_count_label`
    // itself, evaluated at the two ends that can actually print the widest
    // string: just under the k/M boundary, and this device's real capacity
    // ceiling — never a guessed string.
    let capacity = app.pending_point_capacity().unwrap_or(app.point_capacity());
    let label = point_count_label(capacity);
    let point_count_width = num::text_button_width(
        ui,
        &[
            &point_count_label(POINT_COUNT_K_M_BOUNDARY - 1),
            &point_count_label(app.max_point_capacity()),
        ],
    );
    let resp = ui.add(
        egui::Button::new((label, egui::Atom::grow()))
            .min_size(egui::vec2(point_count_width, 0.0)),
    );
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

    // Label names the state you're *in*, not the action a click performs —
    // the pressed/unpressed `selectable_label` styling already carries the
    // state, so the text is reinforcement rather than the only signal.
    //
    // Fixed to the wider of its two captions (`num::icon_button_width`) so
    // toggling it — which changes both the icon and the word — doesn't shift
    // the Play/Pause button beside it.
    let mode_width =
        num::icon_button_width(ui, &[(icons::CUBE, "Edit Mode"), (icons::EYE, "View Mode")]);
    let resp = ui.add(
        egui::Button::selectable(
            app.show_gizmos,
            if app.show_gizmos {
                (icons::CUBE, "Edit Mode", egui::Atom::grow())
            } else {
                (icons::EYE, "View Mode", egui::Atom::grow())
            },
        )
        .min_size(egui::vec2(mode_width, 0.0)),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if app.show_gizmos {
            "Transform gizmos + name labels are showing (G) — click to switch to view mode"
        } else {
            "Transform gizmos + name labels are hidden (G) — click to switch to edit mode"
        },
        if app.show_gizmos { "click: switch to view mode" } else { "click: switch to edit mode" },
    );
    if resp.clicked() {
        app.toggle_gizmos();
    }

    // Same defect, same fix: "Play" and "Pause" aren't the same width either,
    // so this button used to shift whatever sat to its right when clicked.
    let moving = app.camera_moving();
    let play_width = num::icon_button_width(ui, &[(icons::PLAY, "Play"), (icons::PAUSE, "Pause")]);
    let resp = ui.add(
        egui::Button::new(if moving {
            (icons::PAUSE, "Pause", egui::Atom::grow())
        } else {
            (icons::PLAY, "Play", egui::Atom::grow())
        })
        .min_size(egui::vec2(play_width, 0.0)),
    );
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
