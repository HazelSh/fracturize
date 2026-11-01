//! Top toolbar, ordered by task from left to right: where a scene comes from
//! (File, Explore), what I'm working on (Transforms), undo, how it's drawn
//! (the drawing button), the edit tools, and time (the transport and the
//! Timeline). On the right, the document's name, Collaborate and Help.
//!
//! Every item has a caption. When the window is too narrow, a fixed list of
//! things gives way one at a time, always in the same order (`Forms`, the
//! squeeze ladder): see TOPBAR-PLAN.md §4. File, Explore, Transforms, Undo,
//! Play and Help never lose their captions. It used to drop every window
//! toggle's caption at once, which left seven icons nobody could tell apart.
//!
//! Quick controls delegate to the panels' own widgets (`render_panel::
//! draw_popup`, `render_panel::point_count`) rather than reimplementing them,
//! so the rate limiting, hints and persistence can't drift between the two
//! places you can change the same value from.

use crate::app::App;

use super::hints::hinted;
use super::icons;
use super::num;
use super::render_panel;

/// How many captions give way, one at a time, before whole items start
/// moving into the overflow menu.
const CAPTION_STEPS: usize = 5;

/// What moves into the overflow menu when even the captions are gone, first
/// out first. Each keeps its caption there. File, Explore, Play and Help
/// never move, so the bar can always reach everything; the overflow button
/// sits just left of Help.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    /// Undo and redo: Ctrl+Z and the Explore menu have them too.
    Undo,
    Look,
    /// Its count moves onto the overflow button.
    Collaborate,
    Timeline,
    /// The title bar carries it as well.
    Name,
    /// The time readout; Left and Right still step it.
    Time,
    RecordLoop,
    /// Emphasis and the xaos overlay: they go before Edit Mode itself.
    Companions,
    /// The drawing button, as a submenu holding its popup.
    Drawing,
    ToStart,
    Transforms,
    /// Last: the key control, still labelled in the menu.
    EditMode,
}

const OVERFLOW_ORDER: [Item; 12] = [
    Item::Undo,
    Item::Look,
    Item::Collaborate,
    Item::Timeline,
    Item::Name,
    Item::Time,
    Item::RecordLoop,
    Item::Companions,
    Item::Drawing,
    Item::ToStart,
    Item::Transforms,
    Item::EditMode,
];

/// How many items can move into the overflow menu (`UiState::toolbar_item_w`).
pub const OVERFLOW_ITEMS: usize = OVERFLOW_ORDER.len();

/// Every step of the ladder: the captions, then the items.
const STEPS: usize = CAPTION_STEPS + OVERFLOW_ORDER.len();

impl Item {
    fn index(self) -> usize {
        OVERFLOW_ORDER.iter().position(|i| *i == self).unwrap_or(0)
    }
}

/// The width the document's name may take, in characters of the button font:
/// about a 20-character name and a short author. A longer one is cut with `…`.
const NAME_CHARS: usize = 28;

/// Which form each item that gives way is drawn in, at one step of the
/// ladder. Step `n` gives way the first `n` of these, in this order.
#[derive(Clone, Copy)]
struct Forms {
    /// 1: the time readout's frame count.
    frames: bool,
    /// 2: the author after the scene's name.
    author: bool,
    /// 3: Timeline's caption.
    timeline_caption: bool,
    /// 4: Collaborate's caption (its count stays).
    collab_caption: bool,
    /// 5: the renderer's name on the drawing button.
    renderer: bool,
    /// 6 onwards: how many of `OVERFLOW_ORDER` are in the overflow menu.
    overflowed: usize,
}

impl Forms {
    fn at(level: u8) -> Self {
        Self {
            frames: level < 1,
            author: level < 2,
            timeline_caption: level < 3,
            collab_caption: level < 4,
            renderer: level < 5,
            overflowed: (level as usize).saturating_sub(CAPTION_STEPS),
        }
    }

    /// Whether `item` is on the bar, rather than in the overflow menu.
    fn shows(self, item: Item) -> bool {
        item.index() >= self.overflowed
    }
}

/// What each step saves, measured this frame. Every item that gives way has
/// both of its forms pinned to a fixed width, so the saving is exact, and
/// last frame's measured width plus or minus these says which step fits.
///
/// An item moving into the overflow menu saves the width it measured the
/// last time it was on the bar (`UiState::toolbar_item_w`), so the overflow
/// steps are as exact as the caption steps; the first one also pays for the
/// overflow button.
fn savings(ui: &egui::Ui, app: &App) -> [f32; STEPS] {
    let mut s = [0.0; STEPS];
    s[0] = super::transport::frames_width(ui);
    s[1] = name_width(ui, app, true) - name_width(ui, app, false);
    s[2] = timeline_width(ui, true) - timeline_width(ui, false);
    s[3] = collab_width(ui, true) - collab_width(ui, false);
    s[4] = drawing_button_width(ui, app, true) - drawing_button_width(ui, app, false);
    for k in 0..OVERFLOW_ORDER.len() {
        s[CAPTION_STEPS + k] = app.ui_state.toolbar_item_w[k];
    }
    s[CAPTION_STEPS] -= overflow_button_width(ui);
    s
}

/// Draw `add` and remember how wide it came out, item spacing included, so
/// the ladder knows what moving `item` into the overflow menu saves.
fn measured(ui: &mut egui::Ui, app: &mut App, item: Item, add: impl FnOnce(&mut egui::Ui, &mut App)) {
    let spacing = ui.spacing().item_spacing.x;
    let w = ui.scope(|ui| add(ui, app)).response.rect.width() + spacing;
    app.ui_state.toolbar_item_w[item.index()] = w;
}

/// The step that fits `avail`, given the bar measured `measured` wide at step
/// `level`. Going back to a roomier step needs `margin` to spare, so a window
/// edge sitting on a boundary doesn't flicker between the two.
fn choose_level(level: u8, measured: f32, savings: &[f32; STEPS], avail: f32, margin: f32) -> u8 {
    let level = level as usize;
    let width_at = |j: usize| {
        if j < level {
            measured + savings[j..level].iter().sum::<f32>()
        } else {
            measured - savings[level..j].iter().sum::<f32>()
        }
    };
    (0..=STEPS)
        .find(|&j| width_at(j) + if j < level { margin } else { 0.0 } <= avail)
        .unwrap_or(STEPS) as u8
}

pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("fracturize_toolbar").show(ui, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let row_left = ui.cursor().min.x;
            let row_right = ui.max_rect().right();
            let level = app.ui_state.toolbar_level;
            let forms = Forms::at(level);
            ui.add_space(4.0);

            draw_file_menu(ui, app);
            draw_explore_menu(ui, app);
            ui.separator();

            if forms.shows(Item::Transforms) {
                measured(ui, app, Item::Transforms, |ui, app| {
                    let resp = ui.toggle_value(&mut app.ui_state.panels.transforms_open, (icons::SHAPES, "Transforms"));
                    hinted(
                        resp,
                        &mut app.ui_state,
                        "The scene's transforms, and their settings",
                        "toggle the Transforms window",
                    );
                    ui.separator();
                });
            }

            if forms.shows(Item::Undo) {
                measured(ui, app, Item::Undo, |ui, app| {
                    draw_undo_redo(ui, app);
                    ui.separator();
                });
            }

            // A group carries its own separator, so whatever moves into the
            // overflow menu takes its rule with it. Look leaves before the
            // drawing button, and the companions before Edit Mode, so each
            // inner item is gone by the time its group is measured alone.
            if forms.shows(Item::Drawing) {
                measured(ui, app, Item::Drawing, |ui, app| {
                    draw_drawing_button(ui, app, forms.renderer);
                    if forms.shows(Item::Look) {
                        measured(ui, app, Item::Look, draw_look_toggle);
                    }
                    ui.separator();
                });
            }

            if forms.shows(Item::EditMode) {
                measured(ui, app, Item::EditMode, |ui, app| {
                    let spacing = ui.spacing().item_spacing.x;
                    // Zero spacing: the companions are joined to Edit Mode.
                    ui.spacing_mut().item_spacing.x = 0.0;
                    draw_edit_mode(ui, app);
                    if forms.shows(Item::Companions) {
                        measured(ui, app, Item::Companions, draw_companions);
                    }
                    ui.spacing_mut().item_spacing.x = spacing;
                    ui.separator();
                });
            }

            if forms.shows(Item::ToStart) {
                measured(ui, app, Item::ToStart, super::transport::draw_to_start);
            }
            super::transport::draw_play(ui, app);
            if forms.shows(Item::RecordLoop) {
                measured(ui, app, Item::RecordLoop, super::transport::draw_record_loop);
            }
            if forms.shows(Item::Time) {
                measured(ui, app, Item::Time, |ui, app| super::transport::draw_time(ui, app, forms.frames));
            }
            if forms.shows(Item::Timeline) {
                measured(ui, app, Item::Timeline, |ui, app| draw_timeline_toggle(ui, app, forms.timeline_caption));
            }
            let left_end = ui.cursor().min.x;

            // The right-hand block: laid out from the right edge when it fits
            // beside the left, and pushed past the window's edge when even the
            // last step doesn't fit, rather than drawn over the left side.
            let gap = 2.0 * ui.spacing().item_spacing.x;
            let fits = left_end + gap + app.ui_state.toolbar_right_w <= row_right;
            let right_w = if fits {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    draw_right_block(ui, app, forms, true)
                })
                .response
                .rect
                .width()
            } else {
                ui.add_space(gap);
                let start = ui.cursor().min.x;
                draw_right_block(ui, app, forms, false);
                ui.cursor().min.x - start
            };
            app.ui_state.toolbar_right_w = right_w;

            let measured = (left_end - row_left) + gap + right_w;
            let saved = savings(ui, app);
            app.ui_state.toolbar_level = choose_level(
                level,
                measured,
                &saved,
                row_right - row_left,
                ui.spacing().item_spacing.x,
            );
        });
        ui.add_space(2.0);
    });
}

/// The document's name, Collaborate, the overflow menu and Help. `rtl` says
/// the layout runs right to left, so the items are added in reverse.
fn draw_right_block(ui: &mut egui::Ui, app: &mut App, forms: Forms, rtl: bool) {
    let name = |ui: &mut egui::Ui, app: &mut App| {
        if forms.shows(Item::Name) {
            measured(ui, app, Item::Name, |ui, app| {
                if rtl {
                    ui.separator();
                    draw_scene_identity(ui, app, forms.author, rtl);
                } else {
                    draw_scene_identity(ui, app, forms.author, rtl);
                    ui.separator();
                }
            });
        }
    };
    let collab = |ui: &mut egui::Ui, app: &mut App| {
        if forms.shows(Item::Collaborate) {
            measured(ui, app, Item::Collaborate, |ui, app| draw_collab_toggle(ui, app, forms.collab_caption));
        }
    };
    if rtl {
        ui.add_space(6.0);
        draw_help_menu(ui, app);
        draw_overflow_menu(ui, app, forms);
        collab(ui, app);
        name(ui, app);
    } else {
        name(ui, app);
        collab(ui, app);
        draw_overflow_menu(ui, app, forms);
        draw_help_menu(ui, app);
        ui.add_space(6.0);
    }
}

const OVERFLOW_STATE: [(&str, &str); 1] = [(icons::DOTS_THREE, "")];

/// The overflow button's width on the bar, item spacing included.
fn overflow_button_width(ui: &egui::Ui) -> f32 {
    num::Pinned::width(ui, &OVERFLOW_STATE) + ui.spacing().item_spacing.x
}

/// What didn't fit on the bar, in a menu, each with its caption. Only drawn
/// once something has moved into it, and it never moves anything else: it
/// sits just left of Help, which never leaves.
fn draw_overflow_menu(ui: &mut egui::Ui, app: &mut App, forms: Forms) {
    if forms.overflowed == 0 {
        return;
    }
    let gone = &OVERFLOW_ORDER[..forms.overflowed];
    let resp = ui.add(num::Pinned::new(OVERFLOW_STATE[0], &OVERFLOW_STATE));
    let waiting = app.collab_waiting();
    if gone.contains(&Item::Collaborate) && waiting > 0 {
        paint_badge(ui, resp.rect, icons::DOTS_THREE, waiting);
    }
    let names: Vec<&str> = gone
        .iter()
        .map(|i| match i {
            Item::Undo => "undo and redo",
            Item::Look => "Look",
            Item::Collaborate => "Collaborate",
            Item::Timeline => "Timeline",
            Item::Name => "the scene's name",
            Item::Time => "the time",
            Item::RecordLoop => "record and loop",
            Item::Companions => "the edit-mode lights",
            Item::Drawing => "how it's drawn",
            Item::ToStart => "back to the start",
            Item::Transforms => "Transforms",
            Item::EditMode => "Edit Mode",
        })
        .collect();
    let resp = hinted(
        resp,
        &mut app.ui_state,
        format!(
            "What didn't fit on the toolbar at this width: {}. Widen the window and they \
             come back.{}",
            names.join(", "),
            if gone.contains(&Item::Collaborate) && waiting > 0 {
                format!(" Collaborate has {waiting} waiting for you.")
            } else {
                String::new()
            }
        ),
        "click: what didn't fit",
    );
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        for (k, item) in gone.iter().enumerate() {
            if k > 0 {
                ui.separator();
            }
            match item {
                Item::Undo => {
                    let next_undo = app.history.undo_display().next().map(str::to_string);
                    let next_redo = app.history.redo_display().last().map(str::to_string);
                    if menu_item_enabled(ui, app, next_undo.is_some(), &format!("{} Undo", icons::UNDO), "Ctrl+Z", &next_undo.map_or("Nothing to undo".into(), |l| format!("Undo “{l}”"))) {
                        app.undo();
                    }
                    if menu_item_enabled(ui, app, next_redo.is_some(), &format!("{} Redo", icons::REDO), "Ctrl+Shift+Z", &next_redo.map_or("Nothing to redo".into(), |l| format!("Redo “{l}”"))) {
                        app.redo();
                    }
                }
                Item::Look => menu_toggle(
                    ui,
                    app,
                    |p| &mut p.look_open,
                    &format!("{} Look", icons::SLIDERS),
                    "How the picture looks: exposure, point size, colour, background, haze, infinite zoom and the grade",
                ),
                Item::Collaborate => menu_toggle(
                    ui,
                    app,
                    |p| &mut p.collab_open,
                    &if waiting > 0 {
                        format!("{} Collaborate ({waiting} waiting)", icons::CHATS)
                    } else {
                        format!("{} Collaborate", icons::CHATS)
                    },
                    "This scene's thread with an agent",
                ),
                Item::Timeline => menu_toggle(
                    ui,
                    app,
                    |p| &mut p.timeline_open,
                    &format!("{} Timeline", icons::FILM_STRIP),
                    "The camera's path and every animated value on one clock",
                ),
                Item::Name => {
                    let label = format!("{}{}", if app.is_dirty() { "*" } else { "" }, name_text(app, true));
                    let r = ui.menu_button(label, |ui| identity_fields(ui, app));
                    hinted(r.response, &mut app.ui_state, "The scene's name and author. Hover to change them.", "hover: edit the name and author");
                }
                Item::Time => {
                    ui.horizontal(|ui| {
                        ui.label("Time");
                        super::transport::draw_time(ui, app, true);
                    });
                }
                Item::RecordLoop => {
                    let armed = app.record_armed;
                    let r = ui.add(egui::Button::selectable(armed, format!("{} Record", icons::RECORD)));
                    if hinted(r, &mut app.ui_state, "Arm record: every edit keys what it changes at the playhead", "click: arm or stop recording").clicked() {
                        app.record_armed = !armed;
                    }
                    let looping = app.loop_playback;
                    let r = ui.add(egui::Button::selectable(looping, format!("{} Loop", icons::REPEAT)));
                    if hinted(r, &mut app.ui_state, "Loop playback: round the loop region, or the whole timeline", "click: loop or stop looping").clicked() {
                        app.loop_playback = !looping;
                    }
                }
                Item::Companions => {
                    ui.add_enabled_ui(app.show_gizmos, |ui| {
                        let on = app.show_emphasis;
                        let r = ui.add(egui::Button::selectable(on, format!("{} Light up what moves", icons::LIGHTBULB)));
                        if hinted(r, &mut app.ui_state, "Light up the points the selected transform moves (M). Only in edit mode", "click: light up what moves").clicked() {
                            app.toggle_emphasis();
                        }
                        let on = app.ui_state.xaos_overlay;
                        let r = ui.add(egui::Button::selectable(on, format!("{} Xaos edges", icons::FLOW_ARROW)));
                        if hinted(r, &mut app.ui_state, "Draw which transform may follow which, as arrows between gizmos. Only in edit mode", "click: show or hide the xaos edges").clicked() {
                            app.ui_state.xaos_overlay = !on;
                        }
                    });
                }
                Item::Drawing => {
                    let capacity = app.pending_point_capacity().unwrap_or(app.point_capacity());
                    let r = ui.menu_button(drawing_caption(Some(app.render_mode), capacity), |ui| render_panel::draw_popup(ui, app));
                    hinted(r.response, &mut app.ui_state, "How the picture is drawn: the renderer and the point count", "hover: renderer and point count");
                }
                Item::ToStart => {
                    if menu_item(ui, app, &format!("{} Back to the start", icons::SKIP_BACK), "", "Back to the start of the timeline") {
                        app.return_to_start();
                    }
                }
                Item::Transforms => menu_toggle(
                    ui,
                    app,
                    |p| &mut p.transforms_open,
                    &format!("{} Transforms", icons::SHAPES),
                    "The scene's transforms, and their settings",
                ),
                Item::EditMode => {
                    let on = app.show_gizmos;
                    let r = ui.add(egui::Button::selectable(on, format!("{} Edit Mode", icons::CUBE)).shortcut_text("G"));
                    if hinted(r, &mut app.ui_state, "Edit mode: gizmos and names showing. Off is view mode (G)", "click: switch mode").clicked() {
                        app.toggle_gizmos();
                    }
                }
            }
        }
    });
}

/// Collaborate's two forms: `Collaborate (2)` and its icon with the count.
fn collab_width(ui: &egui::Ui, caption: bool) -> f32 {
    num::Pinned::width(ui, &[collab_state(caption)])
}

fn collab_state(caption: bool) -> (&'static str, &'static str) {
    (icons::CHATS, if caption { "Collaborate" } else { "" })
}

/// Collaborate, with what's waiting on you. A badge and nothing more: a
/// message never opens a window over the work. Pinned to a two-digit count,
/// so a message arriving doesn't push Help along.
fn draw_collab_toggle(ui: &mut egui::Ui, app: &mut App, caption: bool) {
    let waiting = app.collab_waiting();
    let open = app.ui_state.panels.collab_open;
    let state = [collab_state(caption)];
    let resp = ui.add(num::Pinned::new(state[0], &state).selected(open));
    // The count is a badge on the icon, in the colour for "waiting on you",
    // so a message arriving changes nothing's width. The tooltip says it in
    // words, which is what carries it for anyone who can't tell the colour.
    if waiting > 0 {
        paint_badge(ui, resp.rect, icons::CHATS, waiting);
    }
    let resp = hinted(
        resp,
        &mut app.ui_state,
        match waiting {
            0 => "This scene's thread with an agent: ask for changes, look at what comes back, \
                  apply it or send it back"
                .to_string(),
            1 => "This scene's thread with an agent: one thing is waiting for your answer".to_string(),
            n => format!("This scene's thread with an agent: {n} things are waiting for your answer"),
        },
        "toggle the Collaborate window",
    );
    if resp.clicked() {
        app.ui_state.panels.collab_open = !open;
    }
}

/// A count on a button's icon, in the colour for "waiting on you": painted,
/// so it changes nothing's width. Whoever calls it says the number in words
/// in the tooltip too.
fn paint_badge(ui: &egui::Ui, rect: egui::Rect, icon: &str, n: usize) {
    let pad = ui.spacing().button_padding;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let icon_w = ui.fonts_mut(|f| f.glyph_width(&font, icon.chars().next().unwrap_or(' ')));
    let r = 0.36 * font.size;
    let c = egui::pos2(rect.left() + pad.x + icon_w, rect.top() + r + 1.0);
    let label = if n > 9 { "9+".to_string() } else { n.to_string() };
    let painter = ui.painter();
    painter.circle_filled(c, r, super::theme::warn(ui.visuals()));
    painter.text(c, egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(r * 1.4), egui::Color32::BLACK);
}

/// The Timeline toggle's two forms: with its caption, and the icon alone.
fn timeline_width(ui: &egui::Ui, caption: bool) -> f32 {
    num::Pinned::width(ui, &[timeline_state(caption)])
}

fn timeline_state(caption: bool) -> (&'static str, &'static str) {
    (icons::FILM_STRIP, if caption { "Timeline" } else { "" })
}

/// The Timeline window, at the end of the transport: the transport is how you
/// use it, so that's where you look for it.
fn draw_timeline_toggle(ui: &mut egui::Ui, app: &mut App, caption: bool) {
    let open = app.ui_state.panels.timeline_open;
    let state = [timeline_state(caption)];
    let resp = ui.add(num::Pinned::new(state[0], &state).selected(open));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "The Timeline: the camera's path and every animated value on one clock, with a \
         playhead to scrub",
        "toggle the Timeline window",
    );
    if resp.clicked() {
        app.ui_state.panels.timeline_open = !open;
    }
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

    // No caption: it sits beside Undo, which explains it, and the glyph is
    // the one every program uses.
    let resp = ui.add_enabled(redo_n > 0, egui::Button::new(icons::REDO));
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

/// The name as the toolbar shows it, with or without the author.
fn name_text(app: &App, with_author: bool) -> String {
    let author = app.scene.author.trim();
    // Opened from a render, it says so: there's no .toml behind it yet.
    let from = if app.scene_png.is_some() { " (from PNG)" } else { "" };
    if author.is_empty() || !with_author {
        format!("{}{}", app.scene.name, from)
    } else {
        format!("{} — {}{}", app.scene.name, author, from)
    }
}

/// The most room the name may take: `NAME_CHARS` of the button font.
fn name_cap(ui: &egui::Ui) -> f32 {
    num::text_button_width(ui, &[&"n".repeat(NAME_CHARS)])
}

/// How wide the name is drawn, with or without the author.
fn name_width(ui: &egui::Ui, app: &App, with_author: bool) -> f32 {
    if app.preview_label().is_some() {
        return name_cap(ui);
    }
    num::text_button_width(ui, &[&name_text(app, with_author)]).min(name_cap(ui))
}

/// The scene's name and author: a readout that opens an editor for itself.
///
/// The `*` that says there are unsaved edits is a cell of its own, outside
/// the name, so a long name cut with `…` can't hide it, and so the bar doesn't
/// shift when it comes and goes. `rtl` says which way the row is being laid
/// out, so the `*` lands before the name either way.
fn draw_scene_identity(ui: &mut egui::Ui, app: &mut App, with_author: bool, rtl: bool) {
    let h = ui.spacing().interact_size.y;
    let cap = name_cap(ui);
    let star_w = num::text_button_width(ui, &["*"]) - 2.0 * ui.spacing().button_padding.x;
    // The same `*` the window title carries, for the same reason: whether
    // there is work on screen that isn't on disk is worth being able to see
    // without opening anything.
    let dirty = app.is_dirty();
    let star = |ui: &mut egui::Ui| {
        ui.add_sized(
            egui::vec2(star_w, h),
            egui::Label::new(egui::RichText::new(if dirty { "*" } else { "" }).weak()),
        );
    };
    let layout = if rtl {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    if !rtl {
        star(ui);
    }

    // While an option is previewed the scene on screen isn't yours, and the
    // readout says so where it would say what it is.
    if let Some(label) = app.preview_label() {
        let resp = ui
            .allocate_ui_with_layout(egui::vec2(cap, h), layout, |ui| {
                ui.add(
                    egui::Button::new(egui::RichText::new(label).color(super::theme::warn(ui.visuals())))
                        .frame(false)
                        .truncate(),
                )
            })
            .inner;
        hinted(
            resp,
            &mut app.ui_state,
            "An option from the Collaborate window is on screen; your scene is set aside, \
             untouched. Esc goes back to it.",
            "previewing an option · Esc: back to your scene",
        );
        if rtl {
            star(ui);
        }
        return;
    }

    let label = name_text(app, with_author);
    let resp = ui
        .allocate_ui_with_layout(egui::vec2(cap, h), layout, |ui| {
            ui.add(egui::Button::new(egui::RichText::new(label).weak()).frame(false).truncate())
        })
        .inner;
    if rtl {
        star(ui);
    }
    let path = match (&app.scene_path, &app.scene_png) {
        (Some(p), _) => p.clone(),
        (None, Some(png)) => format!(
            "Opened from the render {}.\nCtrl+S saves it as a new scene; the PNG is never changed.",
            png.display()
        ),
        (None, None) => "(unsaved scene)".to_string(),
    };
    let tip = format!(
        "{}{}\n{}\n\nClick to change the scene's name or author.",
        name_text(app, true),
        if dirty { " (unsaved changes)" } else { "" },
        path
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        tip,
        "click: edit the scene's name and author",
    );

    egui::Popup::menu(&resp)
        // A menu that closed on the first click inside would dismiss itself
        // the moment you put the caret in a field.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(260.0)
        .show(|ui| identity_fields(ui, app));
}

/// The name and author fields: the toolbar name's popup, and the overflow
/// menu's name row. No "Ctrl+S writes these" footnote: this is a file editing
/// program, so that is the paradigm and not news.
fn identity_fields(ui: &mut egui::Ui, app: &mut App) {
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
/// bound in `drawing_button_width` can ask it for boundary strings — the
/// actual widest output this formatter can produce, not a separately
/// maintained guess that would drift the moment this function changes.
pub(super) fn point_count_label(capacity: u32) -> String {
    if capacity < POINT_COUNT_K_M_BOUNDARY {
        format!("{:.0}k", capacity as f32 / 1e3)
    } else {
        format!("{:.1}M", capacity as f32 / 1e6)
    }
}

/// The renderer's name as the drawing button shows it.
fn renderer_caption(mode: crate::app::RenderMode) -> &'static str {
    use crate::app::RenderMode;
    match mode {
        RenderMode::Points => "Points",
        RenderMode::Splat => "Splat",
        RenderMode::Volume => "Volume",
        RenderMode::Glass => "Glass",
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
fn drawing_captions(app: &App, with_renderer: bool) -> Vec<String> {
    use crate::app::RenderMode;
    let counts = [POINT_COUNT_K_M_BOUNDARY - 1, app.max_point_capacity()];
    let modes: Vec<Option<RenderMode>> = if with_renderer {
        [RenderMode::Points, RenderMode::Splat, RenderMode::Volume, RenderMode::Glass].into_iter().map(Some).collect()
    } else {
        vec![None]
    };
    modes.iter().flat_map(|m| counts.iter().map(move |c| drawing_caption(*m, *c))).collect()
}

fn drawing_button_width(ui: &egui::Ui, app: &App, with_renderer: bool) -> f32 {
    let captions = drawing_captions(app, with_renderer);
    let states: Vec<(&str, &str)> = captions.iter().map(|c| (icons::CARET_DOWN, c.as_str())).collect();
    num::Pinned::width(ui, &states)
}

/// One button for how the picture is drawn: it reads the renderer and the
/// point count, and opens a popup holding both, opacity, and the way to the
/// Look window. It replaced the renderer radio and the point-count button,
/// which between them took a fifth of the toolbar.
fn draw_drawing_button(ui: &mut egui::Ui, app: &mut App, with_renderer: bool) {
    let capacity = app.pending_point_capacity().unwrap_or(app.point_capacity());
    let caption = drawing_caption(with_renderer.then_some(app.render_mode), capacity);
    let captions = drawing_captions(app, with_renderer);
    let states: Vec<(&str, &str)> = captions.iter().map(|c| (icons::CARET_DOWN, c.as_str())).collect();
    let resp = ui.add(num::Pinned::new((icons::CARET_DOWN, &caption), &states).icon_after());
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

/// The Look window, beside the drawing button: how the picture is drawn, then
/// how it looks. A window toggle with its caption; it is short, and never on
/// the ladder.
fn draw_look_toggle(ui: &mut egui::Ui, app: &mut App) {
    let resp = ui.toggle_value(&mut app.ui_state.panels.look_open, (icons::SLIDERS, "Look"));
    hinted(
        resp,
        &mut app.ui_state,
        "How the picture looks: exposure, point size, colour, background, haze, infinite \
         zoom and the grade",
        "toggle the Look window",
    );
}

/// Edit Mode / View Mode, edit first. Always captioned: the word is what
/// says which mode you're in and what the two buttons beside it belong to,
/// so it is never on the ladder.
const MODE_STATES: [(&str, &str); 2] = [(icons::CUBE, "Edit Mode"), (icons::EYE, "View Mode")];

/// Edit Mode / View Mode. Its two companions sit to its right, joined to it
/// (`draw_companions`): both do nothing outside edit mode, so they belong to
/// this control, and zero spacing between the three is what says so. Edit
/// Mode comes first because it's the one they depend on.
fn draw_edit_mode(ui: &mut egui::Ui, app: &mut App) {
    // Label names the state you're *in*, not the action a click performs:
    // the pressed/unpressed styling already carries the state, so the text is
    // reinforcement rather than the only signal. Pinned to the wider of its
    // two captions, so toggling it doesn't shift what's to its right.
    let states = MODE_STATES;
    let shown = if app.show_gizmos { states[0] } else { states[1] };
    let resp = ui.add(num::Pinned::new(shown, &states).selected(app.show_gizmos));
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
}

/// Edit Mode's two companions: the emphasis glow and the xaos overlay.
fn draw_companions(ui: &mut egui::Ui, app: &mut App) {
    // Disabled rather than hidden in view mode: a button that vanishes takes
    // the rest of the toolbar's layout with it.
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

    // The xaos edge overlay: it draws between gizmo origins, so it has nothing
    // to show in view mode either.
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
            "Drawing xaos edges: the selected transform's exits, or every edge that isn't 1. \
             Click an edge to edit it, alt+scroll to scale it, ctrl+right-drag to cut several. \
             Click to stop"
        } else {
            "Draw which transform may follow which, as arrows between gizmos"
        },
        if overlay { "click: hide the xaos edges" } else { "click: show the xaos edges" },
    );
    if resp.clicked() {
        app.ui_state.xaos_overlay = !app.ui_state.xaos_overlay;
    }
    // The warp grid's toggle (branch `warp-view`) goes here, as the third
    // companion: it too is about the selected map, so only edit mode has it.
    // It wants a row in the overflow menu's Companions arm too.
}

#[cfg(test)]
mod tests {
    use super::{STEPS, choose_level};

    const SAVES: [f32; STEPS] = [40.0, 60.0, 50.0, 80.0, 45.0, 70.0, 60.0, 110.0, 75.0, 160.0, 90.0, 80.0, 50.0, 120.0, 30.0, 100.0, 110.0];

    #[test]
    fn the_ladder_takes_the_first_step_that_fits() {
        // Measured 1500 with every caption, in 1440: dropping the frame count
        // (40) isn't enough, dropping the author too (100) is.
        assert_eq!(choose_level(0, 1500.0, &SAVES, 1440.0, 20.0), 2);
        // Room for everything: stays at 0.
        assert_eq!(choose_level(0, 1200.0, &SAVES, 1440.0, 20.0), 0);
        // From step 2 (measured 1400), the same answer: the arithmetic runs
        // both ways from wherever the bar is.
        assert_eq!(choose_level(2, 1400.0, &SAVES, 1440.0, 20.0), 2);
    }

    #[test]
    fn going_back_needs_room_to_spare() {
        // At step 1, measured 1400: step 0 would be 1440, which fits exactly
        // but leaves no margin, so it stays put rather than flicker.
        assert_eq!(choose_level(1, 1400.0, &SAVES, 1440.0, 20.0), 1);
        // With the margin's worth of room, it goes back.
        assert_eq!(choose_level(1, 1380.0, &SAVES, 1440.0, 20.0), 0);
    }

    #[test]
    fn items_move_into_the_overflow_menu_in_order_after_the_captions() {
        use super::{CAPTION_STEPS, Forms, Item};
        let all = [Item::Undo, Item::Look, Item::Collaborate, Item::Timeline, Item::Name];
        // Every caption gone, and still everything on the bar.
        let f = Forms::at(CAPTION_STEPS as u8);
        assert!(all.iter().all(|i| f.shows(*i)));
        // Two steps more: undo/redo and Look have gone, the rest stay.
        let f = Forms::at(CAPTION_STEPS as u8 + 2);
        assert!(!f.shows(Item::Undo) && !f.shows(Item::Look));
        assert!(f.shows(Item::Collaborate) && f.shows(Item::Timeline) && f.shows(Item::Name));
        // The last step: everything that can go has gone.
        let f = Forms::at(STEPS as u8);
        assert!(all.iter().all(|i| !f.shows(*i)));
    }

    #[test]
    fn past_the_last_step_it_stays_there() {
        assert_eq!(choose_level(0, 3000.0, &SAVES, 800.0, 20.0), STEPS as u8);
        assert_eq!(choose_level(STEPS as u8, 2000.0, &SAVES, 800.0, 20.0), STEPS as u8);
    }
}
