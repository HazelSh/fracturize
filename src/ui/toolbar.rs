//! Top toolbar: the File and Explore menus, a thin strip of icon toggle
//! buttons for the panel windows, then the quick controls — the handful of settings you reach for
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

/// The window toggles' captions, for measuring what dropping them saves.
const WINDOW_CAPTIONS: [&str; 7] =
    ["Transforms", "Subscenes", "Xaos", "Collaborate", "Camera", "Timeline", "Render"];

/// A window's toolbar toggle: icon and caption, or the icon alone when the
/// toolbar is too narrow for every caption (`UiState::toolbar_compact`). The
/// tooltip names the window either way.
fn window_toggle(ui: &mut egui::Ui, open: &mut bool, icon: &str, caption: impl Into<String>, compact: bool) -> egui::Response {
    if compact {
        ui.toggle_value(open, icon)
    } else {
        ui.toggle_value(open, (icon, caption.into()))
    }
}

pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("fracturize_toolbar").show(ui, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            // Decided from last frame's fit: a toolbar that ran into its own
            // right-hand block drops the window toggles' captions, and gets
            // them back once there is room for all of them again, with a
            // margin so a window edge sitting on the boundary doesn't flicker
            // between the two.
            let compact = app.ui_state.toolbar_compact;

            draw_file_menu(ui, app);
            draw_explore_menu(ui, app);
            ui.separator();

            // Then the windows, ordered by task rather than by module: what am
            // I working on, then how am I looking at it. Help is pushed to the
            // right end, where it lives in every toolbar ever made.
            let resp = window_toggle(ui, &mut app.ui_state.panels.transforms_open, icons::SHAPES, "Transforms", compact);
            hinted(
                resp,
                &mut app.ui_state,
                "The scene's transforms, and their settings",
                "toggle the Transforms window",
            );

            let resp = window_toggle(ui, &mut app.ui_state.panels.subscenes_open, icons::UNITE, "Subscenes", compact);
            hinted(
                resp,
                &mut app.ui_state,
                "Other scenes placed inside this one",
                "toggle the Subscenes window",
            );

            let resp = window_toggle(ui, &mut app.ui_state.panels.xaos_open, icons::GRAPH, "Xaos", compact);
            hinted(
                resp,
                &mut app.ui_state,
                "Which transform may follow which: the whole graph as a grid",
                "toggle the Xaos window",
            );

            // The badge counts what's waiting on the person. A badge and
            // nothing more: a message never opens a window over the work.
            let waiting = app.collab_waiting();
            let caption = if waiting > 0 { format!("Collaborate ({waiting})") } else { "Collaborate".to_string() };
            let resp = window_toggle(ui, &mut app.ui_state.panels.collab_open, icons::CHATS, caption, compact);
            hinted(
                resp,
                &mut app.ui_state,
                if waiting > 0 {
                    "This scene's thread with an agent: something is waiting for your answer"
                } else {
                    "This scene's thread with an agent: ask for changes, look at what comes back, \
                     apply it or send it back"
                },
                "toggle the Collaborate window",
            );

            ui.separator();

            let resp = window_toggle(ui, &mut app.ui_state.panels.views_open, icons::VIDEO_CAMERA, "Views", compact);
            hinted(
                resp,
                &mut app.ui_state,
                "This framing as numbers, and the views this scene has saved",
                "toggle the Views window",
            );

            let resp = window_toggle(ui, &mut app.ui_state.panels.timeline_open, icons::FILM_STRIP, "Timeline", compact);
            hinted(
                resp,
                &mut app.ui_state,
                "The scene over time: the camera's path and every animated value on one \
                 clock, with a playhead to scrub",
                "toggle the Timeline window",
            );

            let resp = window_toggle(ui, &mut app.ui_state.panels.look_open, icons::SLIDERS, "Look", compact);
            hinted(
                resp,
                &mut app.ui_state,
                "How the picture looks: exposure, point size, colour, haze and infinite zoom",
                "toggle the Look window",
            );

            ui.separator();
            draw_undo_redo(ui, app);

            ui.separator();
            draw_quick_controls(ui, app);
            let left_end = ui.cursor().min.x;

            // Right-aligned, and therefore built right-to-left: help at the far
            // end, then the scene identity beside it.
            let right = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                draw_help_menu(ui, app);
                ui.separator();
                draw_scene_identity(ui, app);
            });
            let gap = right.response.rect.left() - left_end;
            let font = egui::TextStyle::Button.resolve(ui.style());
            let captions: f32 = WINDOW_CAPTIONS
                .iter()
                .map(|c| {
                    ui.ctx().fonts_mut(|f| f.layout_no_wrap(c.to_string(), font.clone(), egui::Color32::WHITE).size().x)
                        + ui.spacing().icon_spacing
                })
                .sum();
            if !compact && gap < 4.0 {
                app.ui_state.toolbar_compact = true;
            } else if compact && gap > captions + 24.0 {
                app.ui_state.toolbar_compact = false;
            }
        });
        ui.add_space(2.0);
    });
}

/// The File menu.
///
/// Not the first entry of a File/Edit/View/Help bar.
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
        "New, open, save, subscenes, views, render, settings and quit",
        "click: the File menu",
    );

    egui::Popup::menu(&resp).show(|ui| {
        // Just wide enough for the longest row ("Save as…  Ctrl+Shift+S"), so
        // the accelerators have a column to line up in without the menu being
        // a slab of empty space.
        ui.set_min_width(168.0);

        if menu_item(ui, app, &format!("{} New blank scene", icons::FILE), "Ctrl+N", "Start over on an empty canvas with two plain transforms to build on. Ctrl+Z brings back what you had.") {
            app.new_blank_scene();
        }
        if menu_item(
            ui,
            app,
            &format!("{} Open…", icons::FOLDER_OPEN),
            "Ctrl+O",
            "Open a scene from the scenes folder. Asks first if there are unsaved changes. You can also drop a scene or a render on the window.",
        ) {
            app.toggle_browser();
        }
        if menu_item(ui, app, &format!("{} Save", icons::SAVE), "Ctrl+S", "Save the scene to its file, including the camera framing and path.") {
            app.save_scene();
        }
        if menu_item(ui, app, &format!("{} Save as…", icons::SAVE), "Ctrl+Shift+S", "Save a copy under a new name and keep working on the copy. The original stays as it was.") {
            super::save_as::open(app);
        }

        ui.separator();

        draw_import_menu(ui, app);
        menu_toggle(
            ui,
            app,
            |p| &mut p.subscenes_open,
            &format!("{} Subscenes…", icons::UNITE),
            "The scenes placed inside this one: show or hide each, its share of the points, where it sits and its colour.",
        );

        ui.separator();

        if menu_item(ui, app, &format!("{} Screenshot", icons::CAMERA), "S", "Save the picture as it is now to the screenshots folder.") {
            app.request_screenshot();
        }
        menu_toggle(
            ui,
            app,
            |p| &mut p.views_open,
            &format!("{} Views…", icons::VIDEO_CAMERA),
            "This framing as numbers, and the views this scene has saved (V saves one). Click a saved view to go back to it.",
        );
        if menu_item(ui, app, &format!("{} Render job…", icons::STACK), "P", "Render a high-quality still or animation in the background. Shows how long it'll take before you start, and you can pause or stop it.") {
            super::render_job::open(app);
        }

        ui.separator();

        menu_toggle(
            ui,
            app,
            |p| &mut p.settings_open,
            &format!("{} Settings…", icons::GEAR),
            "Your own settings, for every scene: how the mouse orbits, the lens, and how big the panels are.",
        );

        ui.separator();

        if menu_item(ui, app, &format!("{} Quit", icons::QUIT), "Ctrl+Q", "Quit Fracturize. Asks first if there are unsaved changes.") {
            app.request_quit();
        }
    });
}

/// "Import as subscene ▸": the scenes folder as a submenu. The same list and
/// the same refusals as the Subscenes window's Import button, so the two
/// can't disagree about what can be placed.
fn draw_import_menu(ui: &mut egui::Ui, app: &mut App) {
    let zoomed = app.scene.zoom.is_some();
    let full = app.scene.layer_count() >= crate::subscene::MAX_LAYERS;
    let mut picked = None;
    let resp = ui
        .add_enabled_ui(!zoomed && !full, |ui| {
            ui.menu_button(format!("{} Import as subscene", icons::PLUS), |ui| {
                let files = app.importable_scenes();
                if files.is_empty() {
                    ui.label(egui::RichText::new("No scenes found in the scenes folder").weak());
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for f in files {
                        let label = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.button(label).on_hover_text(f.display().to_string()).clicked() {
                            picked = Some(f);
                            ui.close();
                        }
                    }
                });
            })
            .response
        })
        .inner;
    hinted(
        resp,
        &mut app.ui_state,
        if zoomed {
            "Subscenes don't work with infinite zoom yet. Turn infinite zoom off first"
        } else if full {
            "A scene can hold at most 15 subscenes"
        } else {
            "Place another scene inside this one. It keeps its own transforms and colours, and gets an equal share of the points"
        },
        "hover: choose a scene to place in this one",
    );
    if let Some(f) = picked {
        app.ui_state.import_error = app.import_subscene(&f).err();
        // Where the result shows, and where an error is said.
        app.ui_state.panels.subscenes_open = true;
    }
}

/// The Help menu: the keys and the mouse, and what this program is.
fn draw_help_menu(ui: &mut egui::Ui, app: &mut App) {
    let resp = ui.button((icons::QUESTION, "Help"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Keyboard shortcuts and mouse controls, and about Fracturize",
        "click: the Help menu",
    );
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(168.0);
        let open = app.show_help;
        let resp = ui.add(egui::Button::selectable(open, format!("{} Controls", icons::KEYBOARD)).shortcut_text("H"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Every keyboard shortcut and mouse gesture. Click a row there to use it (H or F1)",
            if open { "click: close this window" } else { "click: open this window" },
        );
        if resp.clicked() {
            app.toggle_help();
            ui.close();
        }
        menu_toggle(
            ui,
            app,
            |p| &mut p.about_open,
            &format!("{} About Fracturize", icons::INFO),
            "Which version this is, where the source lives, and what to expect from a pre-release.",
        );
    });
}

/// The Explore menu: the ways of finding a scene by chance — roll one, browse
/// a list of rolls, mutate what's there — and the undo that makes all of them
/// safe to try.
///
/// It used to be a window, holding a random-flame button, a blank-scene
/// button, the mutation slider and button, undo and redo, and the history
/// list — five unrelated jobs in a column. Each is its own thing now: the
/// commands are rows here, and the three that need room (History, Mutate,
/// the Random flames list and its settings) are windows this menu opens. The
/// blank scene went to File, where "start a new document" lives in every
/// program.
fn draw_explore_menu(ui: &mut egui::Ui, app: &mut App) {
    let resp = ui.button((icons::FLASK, "Explore"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Random flames, mutation, undo and history",
        "click: the Explore menu",
    );

    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);

        if menu_item(ui, app, &format!("{} Random flame", icons::DICE), "", "Roll a new flame with the random settings and open it. Quality-checked first, so it renders — and it's one Ctrl+Z away from what you have now.") {
            app.random_flame();
        }
        menu_toggle(
            ui,
            app,
            |p| &mut p.random_open,
            &format!("{} Random flames list…", icons::DICE),
            "A scrolling list of rolls down the left of the screen, each a live thumbnail. Click one to open it.",
        );
        draw_profile_menu(ui, app);
        menu_toggle(
            ui,
            app,
            |p| &mut p.random_settings_open,
            &format!("{} Random settings…", icons::SLIDERS),
            "Edit the dice profile: what a roll may make and what a mutation may change. Save your own there.",
        );

        ui.separator();

        if menu_item(ui, app, &format!("{} Mutate", icons::SHUFFLE), "U", "Perturb the scene at random. One Ctrl+Z takes it back.") {
            app.mutate_scene();
        }
        if menu_item(
            ui,
            app,
            &format!("{} Mutation grid{}", icons::SHUFFLE, if app.grid_view() { " (on)" } else { "" }),
            "Ctrl+U",
            "Fill the view with the scene and its mutations around it, all seen through one camera: orbit to judge them from every side, click one to take it, click the middle for new ones.",
        ) {
            app.toggle_grid_view();
        }
        menu_toggle(
            ui,
            app,
            |p| &mut p.mutate_open,
            &format!("{} Mutate…", icons::SHUFFLE),
            "The dice profile, the mutation strength and the grid's size, and a button to mutate with them.",
        );

        ui.separator();

        let next_undo = app.history.undo_display().next().map(str::to_string);
        let next_redo = app.history.redo_display().last().map(str::to_string);
        let undo_tip = match &next_undo {
            Some(l) => format!("Undo “{}”", l),
            None => "Nothing to undo yet".to_string(),
        };
        let redo_tip = match &next_redo {
            Some(l) => format!("Redo “{}”", l),
            None => "Nothing to redo — undo something first".to_string(),
        };
        if menu_item_enabled(ui, app, next_undo.is_some(), &format!("{} Undo", icons::UNDO), "Ctrl+Z", &undo_tip) {
            app.undo();
        }
        if menu_item_enabled(ui, app, next_redo.is_some(), &format!("{} Redo", icons::REDO), "Ctrl+Shift+Z", &redo_tip) {
            app.redo();
        }
        menu_toggle(
            ui,
            app,
            |p| &mut p.history_open,
            &format!("{} History…", icons::RESET),
            "Every edit, newest first. Click one to go back — or forward — to it.",
        );
    });
}

/// The dice profile as a submenu, between the commands it governs: Random
/// flame and the list above it, Mutate below. One click switches it, without
/// opening the settings window. The current one is lit; `*` says it's edited.
fn draw_profile_menu(ui: &mut egui::Ui, app: &mut App) {
    let selected = app.dice_profile().clone();
    let modified = selected.modified(app.random_settings());
    let label = format!("{} Dice profile: {}{}", icons::DICE, selected.entry.name, if modified { " *" } else { "" });
    let mut picked = None;
    let resp = ui.menu_button(label, |ui| {
        for entry in crate::dice::profile::list() {
            let resp = ui.add(egui::Button::selectable(entry == selected.entry, &entry.name));
            let tip = if resp.hovered() {
                crate::dice::profile::load(&entry).map(|l| l.meta.description).unwrap_or_else(|e| e)
            } else {
                String::new()
            };
            if hinted(resp, &mut app.ui_state, &tip, "click: load this dice profile").clicked() {
                picked = Some(entry);
                ui.close();
            }
        }
    });
    hinted(
        resp.response,
        &mut app.ui_state,
        "Pick what random flames are made of and what Mutate may change",
        "hover: the dice profiles",
    );
    if let Some(entry) = picked {
        app.load_dice_profile(entry);
    }
}

/// A menu row that opens or closes a window, lit while it's open.
fn menu_toggle(
    ui: &mut egui::Ui,
    app: &mut App,
    flag: impl Fn(&mut crate::prefs::PanelPrefs) -> &mut bool,
    label: &str,
    tip: &str,
) {
    let open = *flag(&mut app.ui_state.panels);
    let resp = ui.add(egui::Button::selectable(open, label));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        tip,
        if open { "click: close this window" } else { "click: open this window" },
    );
    if resp.clicked() {
        *flag(&mut app.ui_state.panels) = !open;
        ui.close();
    }
}

/// [`menu_item`], greyed when there's nothing for it to do.
fn menu_item_enabled(ui: &mut egui::Ui, app: &mut App, enabled: bool, label: &str, key: &str, tip: &str) -> bool {
    let resp = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(key));
    let resp = hinted(resp, &mut app.ui_state, tip, "click: run this command");
    if resp.clicked() {
        ui.close();
        true
    } else {
        false
    }
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
            None => "Nothing to undo".to_string(),
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
            None => "Nothing to redo".to_string(),
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
    // While an option is previewed the scene on screen isn't yours, and the
    // readout says so where it would say what it is.
    if let Some(label) = app.preview_label() {
        let resp = ui.add(egui::Button::new(egui::RichText::new(label).color(super::theme::warn(ui.visuals()))).frame(false).truncate());
        hinted(
            resp,
            &mut app.ui_state,
            "An option from the Collaborate window is on screen; your scene is set aside, \
             untouched. Esc goes back to it.",
            "previewing an option · Esc: back to your scene",
        );
        return;
    }
    // Opened from a render, it says so: there's no .toml behind it yet.
    let from = if app.scene_png.is_some() { " (from PNG)" } else { "" };
    let label = if author.is_empty() {
        format!("{}{}{}", dirty, app.scene.name, from)
    } else {
        format!("{}{} — {}{}", dirty, app.scene.name, author, from)
    };
    let resp = ui.add(
        egui::Button::new(egui::RichText::new(label).weak())
            .frame(false)
            .truncate(),
    );
    let path = match (&app.scene_path, &app.scene_png) {
        (Some(p), _) => p.clone(),
        (None, Some(png)) => format!(
            "Opened from the render {}.\nCtrl+S saves it as a new scene; the PNG is never changed.",
            png.display()
        ),
        (None, None) => "(unsaved scene)".to_string(),
    };
    let resp = hinted(
        resp,
        &mut app.ui_state,
        format!("{}\n\nClick to change the scene's name or author.", path),
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
                    "Shown here, and used to name screenshots, renders and views",
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
                    "Saved in the scene file, and filled in for you on new scenes you \
                     start here.",
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

/// The drawing button's point count, e.g. `"420k"` or `"1.5M"`.
///
/// Its own function (rather than inlined at the one call site) so the width
/// bound in `draw_quick_controls` can ask it for boundary strings — the
/// actual widest output this formatter can produce, not a separately
/// maintained guess that would drift the moment this function changes.
pub(super) fn point_count_label(capacity: u32) -> String {
    if capacity < POINT_COUNT_K_M_BOUNDARY {
        format!("{:.0}k", capacity as f32 / 1e3)
    } else {
        format!("{:.1}M", capacity as f32 / 1e6)
    }
}

/// How the viewport is divided (`multiview`): one of three, so a segmented
/// radio, and icons because the three pictures say it better than words in a
/// strip with no room for them.
fn view_layout(ui: &mut egui::Ui, app: &mut App) {
    use crate::multiview::Layout;
    let grid = app.grid_view();
    let later = if grid { " Takes effect when you leave the mutation grid." } else { "" };
    let current = app.view_layout();
    let chosen = super::radio::radio(&mut app.ui_state, "view_layout", current)
        .option(
            Layout::Single,
            icons::SQUARE,
            format!("One view (T){later}"),
            "click: one view",
        )
        .option(
            Layout::Quad,
            icons::SQUARES_FOUR,
            format!(
                "Four views: the perspective view top left, with top, front and right \
                 around it (T). \
                 Drag in an axis view to pan, and click a ball on its axis cross to \
                 look down another axis.{later}"
            ),
            "click: four views",
        )
        .option(
            Layout::OneAndThree,
            icons::LAYOUT,
            format!(
                "The perspective view large, with top, front and right down the side \
                 (T).{later}"
            ),
            "click: one big view and three small",
        )
        .show(ui);
    if let Some(layout) = chosen {
        app.set_view_layout(layout);
    }
}

/// The renderer's name as the drawing button shows it.
fn renderer_caption(mode: crate::app::RenderMode) -> &'static str {
    use crate::app::RenderMode;
    match mode {
        RenderMode::Points => "Points",
        RenderMode::Splat => "Splat",
        RenderMode::Volume => "Volume",
    }
}

/// The drawing button's caption: `Splat · 6.0M`, or just `6.0M` once the
/// toolbar has run out of room for the renderer's name.
fn drawing_caption(mode: Option<crate::app::RenderMode>, capacity: u32) -> String {
    match mode {
        Some(m) => format!("{} · {}", renderer_caption(m), point_count_label(capacity)),
        None => point_count_label(capacity),
    }
}

/// The drawing button's width for a caption form: the widest caption it can
/// show in that form, so changing renderer or dragging the count moves
/// nothing beside it. Measured at the two counts that print widest (just under
/// the k/M boundary, and this device's ceiling) across every renderer.
fn drawing_button_width(ui: &egui::Ui, app: &App, with_renderer: bool) -> f32 {
    use crate::app::RenderMode;
    let counts = [POINT_COUNT_K_M_BOUNDARY - 1, app.max_point_capacity()];
    let modes: Vec<Option<RenderMode>> = if with_renderer {
        [RenderMode::Points, RenderMode::Splat, RenderMode::Volume].into_iter().map(Some).collect()
    } else {
        vec![None]
    };
    let captions: Vec<String> =
        modes.iter().flat_map(|m| counts.iter().map(move |c| drawing_caption(*m, *c))).collect();
    let refs: Vec<(&str, &str)> = captions.iter().map(|c| (icons::CARET_DOWN, c.as_str())).collect();
    num::icon_button_width(ui, &refs)
}

/// One button for how the picture is drawn: it reads the renderer and the
/// point count, and opens a popup holding both, opacity, and the way to the
/// Look window. It replaced the renderer radio and the point-count button,
/// which between them took a fifth of the toolbar.
fn draw_drawing_button(ui: &mut egui::Ui, app: &mut App, with_renderer: bool) {
    let capacity = app.pending_point_capacity().unwrap_or(app.point_capacity());
    let caption = drawing_caption(with_renderer.then_some(app.render_mode), capacity);
    let width = drawing_button_width(ui, app, with_renderer);
    let resp = ui.add(
        egui::Button::new((caption, egui::Atom::grow(), icons::CARET_DOWN))
            .min_size(egui::vec2(width, 0.0)),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        format!(
            "How the picture is drawn: the {} renderer, {} points. Click to change \
             either, or to open the Look window (R switches renderer)",
            app.render_mode.label(),
            point_count_label(capacity),
        ),
        "click: renderer, point count and the Look window",
    );
    egui::Popup::menu(&resp)
        // The default menu behaviour closes on any click inside, which would
        // dismiss the popup the instant you grabbed the slider.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(300.0)
        .show(|ui| render_panel::draw_popup(ui, app));
}

/// The drawing button, edit/view mode and the transport: the settings that
/// get changed most often mid-exploration.
fn draw_quick_controls(ui: &mut egui::Ui, app: &mut App) {
    draw_drawing_button(ui, app, true);
    view_layout(ui, app);
    ui.separator();

    // Label names the state you're *in*, not the action a click performs —
    // the pressed/unpressed `selectable_label` styling already carries the
    // state, so the text is reinforcement rather than the only signal.
    //
    // Fixed to the wider of its two captions (`num::icon_button_width`) so
    // toggling it — which changes both the icon and the word — doesn't shift
    // the Play/Pause button beside it.
    // The emphasis toggle rides *attached* to the mode button rather than
    // sitting loose in the strip. It does nothing outside edit mode — there is
    // no selection to be about — so it belongs to that control rather than
    // beside it, and zero spacing between the two is what says so. Disabled
    // rather than hidden in view mode: a button that vanishes takes the rest
    // of the toolbar's layout with it.
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    let resp = ui.add_enabled(
        app.show_gizmos,
        egui::Button::selectable(app.show_emphasis && app.show_gizmos, icons::LIGHTBULB),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if !app.show_gizmos {
            "Light up the points the selected transform moves (M). Only in edit mode"
        } else if app.show_emphasis {
            "Lighting up the points the selected transform moves (M). Click to stop"
        } else {
            "Light up the points the selected transform moves (M)"
        },
        if app.show_emphasis { "click: stop lighting it up" } else { "click: light up what moves" },
    );
    if resp.clicked() {
        app.toggle_emphasis();
    }
    // The xaos edge overlay: the Gizmos control's other sibling, for the same
    // reason — it draws between gizmo origins, so it has nothing to show in
    // view mode.
    let overlay = app.ui_state.xaos_overlay;
    let resp = ui.add_enabled(
        app.show_gizmos,
        egui::Button::selectable(overlay && app.show_gizmos, icons::FLOW_ARROW),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if !app.show_gizmos {
            "Draw which transform may follow which, as arrows between gizmos. Only in edit mode"
        } else if overlay {
            "Drawing xaos edges: the selected transform's exits, or every edge that isn't 1.              Click an edge to edit it, alt+scroll to scale it, ctrl+right-drag to cut several.              Click to stop"
        } else {
            "Draw which transform may follow which, as arrows between gizmos"
        },
        if overlay { "click: hide the xaos edges" } else { "click: show the xaos edges" },
    );
    if resp.clicked() {
        app.ui_state.xaos_overlay = !app.ui_state.xaos_overlay;
    }

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
            "Edit mode: gizmos and names are showing (G). Click for view mode"
        } else {
            "View mode: gizmos and names are hidden (G). Click for edit mode"
        },
        if app.show_gizmos { "click: switch to view mode" } else { "click: switch to edit mode" },
    );
    if resp.clicked() {
        app.toggle_gizmos();
    }
    ui.spacing_mut().item_spacing.x = spacing;

    // The transport: record, return to start, play, the time, loop.
    ui.separator();
    super::transport::draw(ui, app);
}
