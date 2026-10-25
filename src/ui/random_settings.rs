//! Random settings: the dice profile (`dice::RandomSettings`), and the
//! profile picker every window that rolls or mutates shares.
//!
//! One list, because the profile is one list: the parts of a flame the dice
//! may make, grouped as the form (shape and structure) and the colour, each
//! with its on switch and, right under it, the ranges it is drawn from — so
//! "size" carries the size range and "variations" carries the variation list,
//! and nothing that shapes a part is anywhere but next to it. A part that's
//! off is kept as it was by a roll and by a mutation alike; there is no roll
//! half and mutate half to keep in step. After the parts, the quality check
//! every result passes, and last the one thing only a mutation reads: how big
//! a step it takes.
//!
//! The profile bar is on top (which profile, Save / Save as / Revert), and
//! the foot carries both actions the profile governs — Roll one and Mutate —
//! so an edit can be tried where it was made. Edits that change what a roll
//! makes restart the Random flames list once they stop moving. These are a
//! preference, not scene data: they follow the person across scenes, and
//! nothing here is undoable by Ctrl+Z, which is for the artwork.
//!
//! Every range is two fields, min and max, and each field's own range stops at
//! the other's value, so a range can be narrowed to a point but never
//! inverted under the pointer.

use crate::app::App;
use crate::dice::{
    Bound, Part, Parts, RandomSettings, DIMENSION_LIMITS, LACUNARITY_LIMITS, MAX_ATTEMPTS, MAX_CHANGES,
    MAX_TRANSFORMS, MIN_ODDS, ODDS_LIMIT, SCALE_LIMITS, STRENGTH_LIMITS, WEIGHT_LIMIT,
};
use crate::scene::{NUM_VARIATIONS, VARIATION_NAMES};

use super::hints::hinted;
use super::icons;
use super::num;
use super::table::{self, Col};

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.random_settings_open;
    if !open {
        return;
    }
    let mut s = app.random_settings().clone();
    let mut action = None;
    let mut roll = false;
    let mut mutate = false;

    super::window(ctx, app, super::WindowKey::RandomSettings, "Random settings")
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Panel::top("fracturize_random_settings_profile").show(ui, |ui| {
                action = profile_bar(ui, app, &s);
                ui.add_space(2.0);
            });
            egui::Panel::bottom("fracturize_random_settings_foot").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let resp = ui.button((icons::DICE, "Roll one"));
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Roll a flame with these settings and open it: every part that's on drawn \
                         anew, the rest kept from the scene you have. Ctrl+Z brings it back.",
                        "click: roll a random flame",
                    );
                    roll = resp.clicked();
                    let resp = ui.button((icons::SHUFFLE, "Mutate"));
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Change a few of the parts that are on, a step at a time (U). Ctrl+Z \
                         takes it back.",
                        "click: mutate the scene",
                    );
                    mutate = resp.clicked();
                });
                ui.add_space(2.0);
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                sections(ui, &mut app.ui_state, &mut s, &app.random_list);
            });
        });
    super::remember(ctx, app, super::WindowKey::RandomSettings);

    match action {
        // Loading replaces the settings, so this frame's edits go with them.
        Some(ProfileAction::Load(entry)) => app.load_dice_profile(entry),
        Some(ProfileAction::Revert) => app.revert_dice_profile(),
        Some(ProfileAction::Save) => {
            app.set_random_settings(s);
            app.ui_state.dice_profile_error = app.save_dice_profile().err();
        }
        Some(ProfileAction::SaveAs(name)) => {
            app.set_random_settings(s);
            match app.save_dice_profile_as(&name) {
                Ok(()) => {
                    app.ui_state.dice_save_as = None;
                    app.ui_state.dice_profile_error = None;
                }
                Err(e) => app.ui_state.dice_profile_error = Some(e),
            }
        }
        None => app.set_random_settings(s),
    }
    if roll {
        app.random_flame();
    }
    if mutate {
        app.mutate_scene();
    }
    app.ui_state.panels.random_settings_open = open;
}

enum ProfileAction {
    Load(crate::dice::profile::Entry),
    Revert,
    Save,
    SaveAs(String),
}

/// The dice profile picker: one drop-down, the same in every window that rolls
/// or mutates, so the profile can be changed where it's used. Built-in
/// profiles first, then the person's own, then shared ones, each under a
/// heading; hovering one says what it does. The caption carries a `*` while
/// the settings differ from the profile. Returns the profile picked.
pub fn profile_picker(ui: &mut egui::Ui, app: &mut App, width: f32) -> Option<crate::dice::profile::Entry> {
    use crate::dice::profile;
    let mut picked = None;
    let selected = app.dice_profile().clone();
    let modified = selected.modified(app.random_settings());
    let caption = format!("{}{}", selected.entry.name, if modified { " *" } else { "" });
    let combo = egui::ComboBox::from_id_salt(ui.id().with("fracturize_dice_profile"))
        .selected_text(caption)
        .width(width)
        .show_ui(ui, |ui| {
            let entries = profile::list();
            let groups: [(&str, fn(&profile::Entry) -> bool); 3] = [
                ("Built in", |e| e.is_builtin()),
                ("Yours", |e| !e.is_builtin() && !e.shared),
                ("Shared", |e| e.shared),
            ];
            let mut first = true;
            for (heading, member) in groups {
                let group: Vec<_> = entries.iter().filter(|e| member(e)).collect();
                if group.is_empty() {
                    continue;
                }
                if !first {
                    ui.separator();
                }
                first = false;
                ui.label(egui::RichText::new(heading).small().weak());
                for entry in group {
                    let resp = ui.selectable_label(*entry == selected.entry, &entry.name);
                    // Read only for the row under the pointer, not every row
                    // every frame the list is open.
                    let tip = if resp.hovered() {
                        let what = profile::load(entry).map(|l| l.meta.description).unwrap_or_else(|e| e);
                        if what.is_empty() { format!("Load {}", entry.name) } else { what }
                    } else {
                        String::new()
                    };
                    if hinted(resp, &mut app.ui_state, &tip, "click: load this dice profile").clicked() {
                        picked = Some(entry.clone());
                    }
                }
            }
        });
    let tip = format!(
        "{}{}Pick which parts of a flame the dice may change, for rolls and mutations \
         alike. Save as in Random settings makes your own.",
        selected.description,
        if selected.description.is_empty() { "" } else { ". " },
    );
    hinted(combo.response, &mut app.ui_state, &tip, "click: pick a dice profile");
    picked
}

/// Which profile these settings are, and the document verbs for it: pick one,
/// Save, Save as…, Revert. The name carries a `*` while the settings differ
/// from the profile, as a scene's does.
fn profile_bar(ui: &mut egui::Ui, app: &mut App, s: &RandomSettings) -> Option<ProfileAction> {
    let mut action = None;
    let selected = app.dice_profile().clone();
    let modified = selected.modified(s);
    ui.horizontal(|ui| {
        ui.label("Profile");
        if let Some(entry) = profile_picker(ui, app, 200.0) {
            action = Some(ProfileAction::Load(entry));
        }
    });
    if !selected.description.is_empty() {
        ui.label(egui::RichText::new(&selected.description).small().weak());
    }
    // The buttons get a row of their own, so the bar fits the window's
    // narrowest width with nothing clipped.
    ui.horizontal(|ui| {
        let writable = selected.writable();
        let resp = ui.add_enabled(writable && modified, egui::Button::new((icons::SAVE, "Save")));
        let tip = if !writable {
            "Built-in and shared profiles can't be changed. Save as makes one of your own."
        } else if !modified {
            "Nothing to save: the settings are what the profile says"
        } else {
            "Write these settings into the profile, keeping any comments in its file"
        };
        if hinted(resp, &mut app.ui_state, tip, "click: save the profile").clicked() {
            action = Some(ProfileAction::Save);
        }
        let resp = ui.button("Save as…");
        if hinted(resp, &mut app.ui_state, "Save these settings as a new profile of your own", "click: save as a new profile")
            .clicked()
        {
            app.ui_state.dice_save_as = Some(String::new());
            app.ui_state.dice_profile_error = None;
        }
        let resp = ui.add_enabled(modified && selected.baseline.is_some(), egui::Button::new((icons::UNDO, "Revert")));
        if hinted(resp, &mut app.ui_state, "Put the settings back to what the profile says", "click: revert to the profile")
            .clicked()
        {
            action = Some(ProfileAction::Revert);
        }
    });
    if selected.baseline.is_none() {
        ui.label(egui::RichText::new("The profile's file is gone or no longer reads. Save as keeps these settings.").small().weak());
    }

    if let Some(mut name) = app.ui_state.dice_save_as.take() {
        let mut keep = true;
        ui.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text("name").desired_width(140.0));
            let field = hinted(field, &mut app.ui_state, "The new profile's name: letters, digits, '-', '_'", "type: profile name");
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let resp = ui.add_enabled(!name.trim().is_empty(), egui::Button::new((icons::SAVE, "Save")));
            if hinted(resp, &mut app.ui_state, "Save the profile under this name", "click: save as").clicked() || enter {
                action = Some(ProfileAction::SaveAs(name.clone()));
            }
            let resp = ui.button("Cancel");
            if hinted(resp, &mut app.ui_state, "Don't save a new profile", "click: cancel").clicked() {
                keep = false;
                app.ui_state.dice_profile_error = None;
            }
        });
        if keep {
            app.ui_state.dice_save_as = Some(name);
        }
    }
    if let Some(e) = &app.ui_state.dice_profile_error {
        ui.colored_label(ui.visuals().warn_fg_color, e);
    }
    action
}

/// The parts, grouped the way people think of a flame: its shape, how it's
/// put together, its colour. Indices into [`Parts::NAMES`]. The colour group
/// is exactly what `colour-only` changes and `no-colour` leaves alone.
const PART_GROUPS: [(&str, &[usize]); 3] = [("Shape", &[0, 1, 2, 3, 4]), ("Structure", &[5, 6]), ("Colour", &[7, 8, 9])];

/// Indices into [`Parts::NAMES`] of the parts the range and chance rows serve,
/// so each row greys while its part is off — nothing about drawing a part
/// applies while the dice keep the scene's own.
mod part {
    pub const SCALE: usize = 2;
    pub const VARIATIONS: usize = 4;
    pub const COUNT: usize = 5;
    pub const SYMMETRY: usize = 6;
}

/// Every section of the window, each one table holding one kind of value:
/// odds, then low–high ranges, then chances, then which groups and which
/// variations may be drawn, then the bounds every result is checked against,
/// then a mutation's step.
fn sections(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings, list: &crate::random_list::RandomList) {
    section(ui, "Parts", |ui| parts(ui, st, s));
    section(ui, "Ranges", |ui| ranges(ui, st, s));
    section(ui, "Chances", |ui| chances(ui, st, s));
    section(ui, "Symmetry groups", |ui| groups(ui, st, s));
    let on = s.variations.values().filter(|v| v.enabled).count();
    let title = format!("Variations ({on} of {})", s.variations.len());
    section(ui, &title, |ui| variations(ui, st, s));
    section(ui, "Quality check", |ui| bounds(ui, st, s, list));
    section(ui, "Mutation step", |ui| mutation(ui, st, s));
}

/// A section: a heading that folds its table away. Open to start with, and
/// remembered once folded.
fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    // Salted by the first word, so a title carrying a count keeps its state.
    let salt = title.split(' ').next().unwrap_or(title).to_string();
    egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .id_salt(("fracturize_dice_section", salt))
        .default_open(true)
        .show(ui, add);
    ui.add_space(4.0);
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().weak());
}

fn parts(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    caption(
        ui,
        "A roll draws every part that's on; a mutation changes a few, picked by their odds. \
         Off (odds 0), both keep the scene's own.",
    );
    let total: f32 = s.parts.all().iter().map(|p| p.odds).sum();
    let cols = [
        Col::name("Part", "What the dice may change"),
        Col::new("Odds", "How often a mutation picks this part, against the others. 0 is off; dragging up from 0 switches it on.", 64.0),
        Col::new("Share", "This part's share of a mutation's changes, from the odds", 48.0),
        Col::new("Re-roll", "Chance a mutation replaces the part outright, drawn the way a roll draws it, rather than nudging what's there", 58.0),
        Col::tick("On", "Ticked, a roll draws this part anew and a mutation may change it. Unticked (odds 0), both keep the scene's own. Ticking it on starts its odds at the least, 0.0001."),
    ];
    table::show(ui, st, "fracturize_dice_parts", &cols, |body, st| {
        let h = table::row_height(body.ui_mut());
        for (group, members) in PART_GROUPS {
            table::group_row(body, group);
            for &k in members {
                let name = Parts::NAMES[k];
                let part = &mut *s.parts.all_mut()[k];
                body.row(h, |mut row| {
                    name_cell(&mut row, st, part.on(), part_label(name), part_tip(name));
                    row.col(|ui| odds(ui, st, part));
                    row.col(|ui| {
                        let text = match part.odds {
                            0.0 => String::new(),
                            o if 100.0 * o / total < 0.5 => "<1%".to_string(),
                            o => format!("{}%", num::fixed(100.0 * o / total, 0)),
                        };
                        num::small_cell(ui, &text, 4);
                    });
                    row.col(|ui| {
                        ui.add_enabled_ui(part.on(), |ui| {
                            percent(
                                ui,
                                st,
                                &mut part.reroll,
                                "Chance a mutation replaces this outright, drawn the way a roll draws it, \
                                 rather than nudging what's there",
                                "drag: chance of a re-roll",
                            );
                        });
                    });
                    // The switch and the odds are one state: ticking sets the
                    // least odds or none, and odds dragged to 0 untick it.
                    row.col(|ui| {
                        let mut on = part.on();
                        let resp = table::tick(ui, &mut on);
                        if resp.changed() {
                            part.set_on(on);
                        }
                        let tip = format!(
                            "{} Unticked, the dice keep the scene's own, in a roll and a mutation alike; \
                             ticked, it starts at the least odds.",
                            part_tip(name)
                        );
                        hinted(resp, st, &tip, if part.on() { "click: keep this as it is" } else { "click: let the dice change this" });
                    });
                });
            }
        }
    });
}

/// A part's odds. 0 reads "off", the least that's on is [`MIN_ODDS`], and
/// nothing in between can be set — so on and off are said one way only.
fn odds(ui: &mut egui::Ui, st: &mut super::UiState, part: &mut Part) {
    let mut v = part.odds;
    let resp = ui.add_sized(
        cell_size(ui, 6),
        egui::DragValue::new(&mut v)
            .range(0.0..=ODDS_LIMIT)
            .speed(0.005)
            .custom_formatter(|v, _| odds_text(v as f32))
            .custom_parser(|t| match t.trim() {
                "off" => Some(0.0),
                t => t.parse().ok(),
            }),
    );
    if resp.changed() {
        part.odds = if v > 0.0 { v.max(MIN_ODDS) } else { 0.0 };
    }
    hinted(
        resp,
        st,
        "How often a mutation picks this, against the other parts. Drag to 0 to switch it off; \
         the least it can be while on is 0.0001.",
        "drag: odds a mutation changes this · 0 is off",
    );
}

fn odds_text(v: f32) -> String {
    if v <= 0.0 {
        "off".into()
    } else if v < 0.01 {
        format!("{v:.4}")
    } else {
        format!("{v:.2}")
    }
}

/// Size of a cell holding `chars` characters of a number, as `num::drag`
/// sizes it, so every number column is as wide as its widest value.
fn cell_size(ui: &egui::Ui, chars: usize) -> [f32; 2] {
    [num::mono_width(ui, chars) + 2.0 * ui.spacing().button_padding.x + 4.0, ui.spacing().interact_size.y]
}

/// A chance, shown and typed as a percentage.
fn percent(ui: &mut egui::Ui, st: &mut super::UiState, v: &mut f32, tip: &str, hint: &str) {
    let resp = ui.add_sized(
        cell_size(ui, 4),
        egui::DragValue::new(v)
            .range(0.0..=1.0)
            .speed(0.005)
            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
            .custom_parser(|t| t.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|p| p / 100.0)),
    );
    hinted(resp, st, tip, hint);
}

/// The low and high cells of a whole-number range, each stopping at the
/// other so the range narrows to a point but never turns inside out.
#[allow(clippy::too_many_arguments)]
fn count_cells(
    row: &mut egui_extras::TableRow<'_, '_>,
    st: &mut super::UiState,
    on: bool,
    r: &mut [u32; 2],
    lo: u32,
    hi: u32,
    tip: &str,
    hint: &str,
) {
    let max = r[1];
    row.col(|ui| {
        ui.add_enabled_ui(on, |ui| {
            let resp = num::drag(ui, 3, 0, egui::DragValue::new(&mut r[0]).range(lo..=max).speed(0.05));
            hinted(resp, st, tip, hint);
        });
    });
    let min = r[0];
    row.col(|ui| {
        ui.add_enabled_ui(on, |ui| {
            let resp = num::drag(ui, 3, 0, egui::DragValue::new(&mut r[1]).range(min..=hi).speed(0.05));
            hinted(resp, st, tip, hint);
        });
    });
}

/// [`count_cells`] for a fractional range.
#[allow(clippy::too_many_arguments)]
fn float_cells(
    row: &mut egui_extras::TableRow<'_, '_>,
    st: &mut super::UiState,
    on: bool,
    r: &mut [f32; 2],
    limits: [f32; 2],
    speed: f64,
    decimals: usize,
    tip: &str,
    hint: &str,
) {
    let chars = if limits[1] >= 100.0 { 7 } else { 5 };
    let max = r[1];
    row.col(|ui| {
        ui.add_enabled_ui(on, |ui| {
            let dv = egui::DragValue::new(&mut r[0]).range(limits[0]..=max).speed(speed);
            hinted(num::drag(ui, chars, decimals, dv), st, tip, hint);
        });
    });
    let min = r[0];
    row.col(|ui| {
        ui.add_enabled_ui(on, |ui| {
            let dv = egui::DragValue::new(&mut r[1]).range(min..=limits[1]).speed(speed);
            hinted(num::drag(ui, chars, decimals, dv), st, tip, hint);
        });
    });
}

/// A row's name, greyed with its row while the part it serves is off.
fn name_cell(row: &mut egui_extras::TableRow<'_, '_>, st: &mut super::UiState, on: bool, name: &str, tip: &str) {
    row.col(|ui| {
        ui.add_enabled_ui(on, |ui| {
            let resp = ui.add(egui::Label::new(name).sense(egui::Sense::hover()));
            hinted(resp, st, tip, name);
        });
    });
}

const LOW_HIGH: [Col<'static>; 3] = [
    Col::name("Setting", "What the range is for. A row greys while the part it draws is off."),
    Col::new("Low", "The least the dice draw", 60.0),
    Col::new("High", "The most the dice draw", 60.0),
];

fn ranges(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    caption(ui, "What each part is drawn between, by a roll and a mutation's re-roll alike.");
    let on = |k: usize| s.parts.all()[k].on();
    let (size, vars, count, sym) = (on(part::SCALE), on(part::VARIATIONS), on(part::COUNT), on(part::SYMMETRY));
    let repeat = sym && s.symmetry.repeat;
    let folds = sym && (s.symmetry.cyclic || s.symmetry.dihedral);
    table::show(ui, st, "fracturize_dice_ranges", &LOW_HIGH, |body, st| {
        let h = table::row_height(body.ui_mut());
        table::group_row(body, "Shape");
        body.row(h, |mut row| {
            let tip = "Each transform's size, drawn evenly on a log scale. A dimension bound, when on, \
                       resizes all the transforms together and wins over this.";
            name_cell(&mut row, st, size, "size", tip);
            float_cells(&mut row, st, size, &mut s.scale, SCALE_LIMITS, 0.005, 2, tip, "drag: size range");
        });
        body.row(h, |mut row| {
            let tip = "How many different variations one flame draws from. One kind across several \
                       transforms reads as a single form; many read as unrelated distortions.";
            name_cell(&mut row, st, vars, "variation kinds", tip);
            count_cells(&mut row, st, vars, &mut s.kinds, 1, NUM_VARIATIONS as u32 - 1, tip, "drag: variation kinds per flame");
        });
        body.row(h, |mut row| {
            let tip = "Variations on each transform, from the flame's kinds. At 0 some stay plain \
                       affine; at least one transform always gets one.";
            name_cell(&mut row, st, vars, "variations per transform", tip);
            count_cells(&mut row, st, vars, &mut s.per_map, 0, NUM_VARIATIONS as u32 - 1, tip, "drag: variations per transform");
        });
        table::group_row(body, "Structure");
        body.row(h, |mut row| {
            let tip = "How many transforms a flame has. Three and four are where structure stays \
                       legible; past six it tends toward fog. A mutation stays inside this too.";
            name_cell(&mut row, st, count, "transforms per flame", tip);
            count_cells(&mut row, st, count, &mut s.transforms, 1, MAX_TRANSFORMS, tip, "drag: transforms per flame");
        });
        body.row(h, |mut row| {
            let tip = "The n of Cyc and Dih: how many copies go round the axis";
            name_cell(&mut row, st, folds, "symmetry fold", tip);
            count_cells(&mut row, st, folds, &mut s.symmetry.fold, 2, crate::symmetry::MAX_FOLD, tip, "drag: fold range");
        });
        body.row(h, |mut row| {
            let tip = "Copies in a repeat, counting the first";
            name_cell(&mut row, st, repeat, "repeat copies", tip);
            count_cells(&mut row, st, repeat, &mut s.symmetry.repeat_count, 2, crate::symmetry::MAX_REPEAT, tip, "drag: repeat copies");
        });
        body.row(h, |mut row| {
            let tip = "Transforms kept out of the group. One is the useful default: a wholly symmetric \
                       flame reads as wallpaper, and one transform outside the group gives it somewhere \
                       to look. At least one transform always joins.";
            name_cell(&mut row, st, sym, "outside the group", tip);
            count_cells(&mut row, st, sym, &mut s.symmetry.free, 0, MAX_TRANSFORMS, tip, "drag: transforms outside the group");
        });
        table::group_row(body, "Mutation");
        body.row(h, |mut row| {
            let tip = "How many parts one mutation changes, each picked by the parts' odds";
            name_cell(&mut row, st, true, "changes per mutation", tip);
            count_cells(&mut row, st, true, &mut s.mutation.changes, 1, MAX_CHANGES, tip, "drag: changes per mutation");
        });
    });
}

fn chances(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    caption(ui, "How often a roll draws each of these.");
    let on = |k: usize| s.parts.all()[k].on();
    let (size, vars, sym) = (on(part::SCALE), on(part::VARIATIONS), on(part::SYMMETRY));
    let cols = [
        Col::name("Setting", "What the chance is of. A row greys while the part it draws is off."),
        Col::new("Chance", "How often a roll draws it", 60.0),
    ];
    table::show(ui, st, "fracturize_dice_chances", &cols, |body, st| {
        let h = table::row_height(body.ui_mut());
        let mut chance_row = |body: &mut egui_extras::TableBody<'_>, on: bool, name: &str, v: &mut f32, tip: &str| {
            body.row(h, |mut row| {
                name_cell(&mut row, st, on, name, tip);
                row.col(|ui| {
                    ui.add_enabled_ui(on, |ui| percent(ui, st, v, tip, "drag: chance"));
                });
            });
        };
        table::group_row(body, "Shape");
        chance_row(body, size, "a squashed axis", &mut s.squash, "Chance a transform has one axis squashed. That's what turns blobs into sheets and ribbons.");
        chance_row(body, vars, "no variations at all", &mut s.affine, "Chance a flame is plain affine, with no variations");
        chance_row(body, vars, "a negative weight", &mut s.negative, "Chance a variation's weight comes out negative, Apophysis-style");
        table::group_row(body, "Structure");
        chance_row(
            body,
            sym,
            "a symmetry group",
            &mut s.symmetry.chance,
            "Chance a roll gives the flame a symmetry group. At 0 none do. Its transforms shrink as \
             they join, so sixty copies under Icosa don't fill space into a lump.",
        );
        chance_row(body, sym, "a mirrored group", &mut s.symmetry.mirror, "Chance a group is extended by the central inversion, doubling its copies");
        chance_row(
            body,
            sym,
            "orbit colouring",
            &mut s.symmetry.orbit_color,
            "Chance the colour is offset by which copy was drawn: an interference pattern through \
             the form, rather than every copy alike",
        );
    });
}

fn groups(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    caption(ui, "Which kinds a roll may draw when it gives a flame a group.");
    let on = s.parts.all()[part::SYMMETRY].on();
    let cols = [
        Col::name("Group", "The kinds of symmetry group"),
        Col::new("Copies", "How many copies of a transform it makes", 70.0),
        Col::tick("On", "Ticked, a roll may draw this kind"),
    ];
    let sym = &mut s.symmetry;
    let rows: [(&mut bool, &str, &str, &str); 6] = [
        (&mut sym.cyclic, "Cyc", "fold", "n-fold about a random axis: the mandala"),
        (&mut sym.dihedral, "Dih", "2 × fold", "n-fold plus the half-turn flip, so it has a front and a back"),
        (&mut sym.tetra, "Tetra", "12", "The tetrahedron's rotations"),
        (&mut sym.octa, "Octa", "24", "The cube's rotations"),
        (&mut sym.icosa, "Icosa", "60", "The icosahedron's rotations"),
        (
            &mut sym.repeat,
            "Repeat",
            "copies",
            "Copies stepped up a tapering helix. Not a group: it repeats without being symmetric.",
        ),
    ];
    table::show(ui, st, "fracturize_dice_groups", &cols, |body, st| {
        let h = table::row_height(body.ui_mut());
        for (flag, name, copies, tip) in rows {
            body.row(h, |mut row| {
                name_cell(&mut row, st, on, name, tip);
                row.col(|ui| {
                    ui.add_enabled_ui(on, |ui| {
                        ui.label(egui::RichText::new(copies).weak());
                    });
                });
                row.col(|ui| {
                    ui.add_enabled_ui(on, |ui| {
                        let resp = table::tick(ui, flag);
                        hinted(resp, st, tip, "click: roll this kind of symmetry");
                    });
                });
            });
        }
    });
}

fn variations(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    let on = s.parts.all()[part::VARIATIONS].on();
    caption(ui, "Which variations may be drawn, and between what weights (the sign is the negative chance's).");
    ui.add_enabled_ui(on, |ui| {
        ui.horizontal(|ui| {
            let resp = ui.small_button("Bounded set");
            let resp = hinted(
                resp,
                st,
                "Switch on the variations that stay bounded in 3D — what the dice roll by \
                 default — and the rest off. Weight ranges are kept.",
                "click: roll only the bounded variations",
            );
            if resp.clicked() {
                let defaults = RandomSettings::default();
                for (name, v) in s.variations.iter_mut() {
                    v.enabled = defaults.variations[name].enabled;
                }
            }
            let resp = ui.small_button("All");
            if hinted(resp, st, "Switch every variation on", "click: roll every variation").clicked() {
                s.variations.values_mut().for_each(|v| v.enabled = true);
            }
            let resp = ui.small_button("None");
            let resp = hinted(resp, st, "Switch every variation off: every flame comes out affine", "click: roll no variations");
            if resp.clicked() {
                s.variations.values_mut().for_each(|v| v.enabled = false);
            }
        });
    });
    let cols = [
        Col::name("Variation", "The variations a transform can carry"),
        Col::new("Low", "Its least weight, as a magnitude. Linear takes up what's left of 1.", 60.0),
        Col::new("High", "Its greatest weight, as a magnitude", 60.0),
        Col::tick("On", "Ticked, a roll or a mutation may bring it in"),
    ];
    table::show(ui, st, "fracturize_dice_variations", &cols, |body, st| {
        let h = table::row_height(body.ui_mut());
        for name in VARIATION_NAMES.iter().skip(1) {
            let Some(v) = s.variations.get_mut(*name) else { continue };
            body.row(h, |mut row| {
                name_cell(&mut row, st, on && v.enabled, name, &variation_tip(name));
                let tip = "This variation's weight when it is drawn, as a magnitude";
                float_cells(&mut row, st, on && v.enabled, &mut v.weight, [0.0, WEIGHT_LIMIT], 0.005, 2, tip, "drag: weight range");
                row.col(|ui| {
                    ui.add_enabled_ui(on, |ui| {
                        let resp = table::tick(ui, &mut v.enabled);
                        hinted(
                            resp,
                            st,
                            &variation_tip(name),
                            if v.enabled { "click: stop rolling this variation" } else { "click: roll this variation" },
                        );
                    });
                });
            });
        }
    });
}

/// A part's name as the interface says it. The file keeps the keys.
fn part_label(name: &str) -> &'static str {
    match name {
        "translate" => "position",
        "rotate" => "turn",
        "scale" => "size",
        "weight" => "weight",
        "variations" => "variations",
        "count" => "how many transforms",
        "symmetry" => "symmetry group",
        "color" => "colours",
        "color_value" => "place on gradient",
        "background" => "background",
        _ => "",
    }
}

fn part_tip(name: &str) -> &'static str {
    match name {
        "translate" => "Where each transform sits.",
        "rotate" => "How each transform is turned, about its own origin.",
        "scale" => "Each transform's size, from the size range.",
        "weight" => "How often each transform is picked by the chaos game.",
        "variations" => "Each transform's variations, from the Variations list.",
        "count" => "How many transforms: a roll draws it from its range, a mutation adds or removes one within it.",
        "symmetry" => "The symmetry group, from the Symmetry groups list. A roll with no chance of one draws none.",
        "color" => "The colours: each transform's, the colour source and the gradient.",
        "color_value" => "Where each transform sits on the gradient.",
        "background" => "The background colour, drawn from the flame's own colours.",
        _ => "",
    }
}

fn variation_tip(name: &str) -> String {
    let note = match name {
        "spherical" => " Its 1/r² inversion sprays far outliers; most rolls with it fail the shape gate.",
        "hyperbolic" | "tangent" => " Unbounded: expect more rejections.",
        "absfold" | "boxfold" | "spherefold" | "bulb" => " A fold or escape-time import — bounded.",
        _ => "",
    };
    format!("Roll {name} on this flame's transforms.{note}")
}

fn bounds(
    ui: &mut egui::Ui,
    st: &mut super::UiState,
    s: &mut RandomSettings,
    list: &crate::random_list::RandomList,
) {
    caption(ui, "Every roll and mutation passes these before you see it.");
    let cols = [
        Col::name("Bound", "What's measured"),
        Col::new("Low", "The least it may be", 60.0),
        Col::new("High", "The most it may be", 60.0),
        Col::tick("On", "Ticked, a result must land inside the range"),
    ];
    let bound_row = |body: &mut egui_extras::TableBody<'_>,
                         st: &mut super::UiState,
                         name: &str,
                         b: &mut Bound,
                         limits: [f32; 2],
                         speed: f64,
                         decimals: usize,
                         tip: &str| {
        let h = table::row_height(body.ui_mut());
        body.row(h, |mut row| {
            name_cell(&mut row, st, b.on, name, tip);
            let on = b.on;
            float_cells(&mut row, st, on, &mut b.range, limits, speed, decimals, tip, "drag: bound range");
            row.col(|ui| {
                let resp = table::tick(ui, &mut b.on);
                hinted(resp, st, tip, if b.on { "click: stop bounding this" } else { "click: bound this" });
            });
        });
    };
    table::show(ui, st, "fracturize_dice_bounds", &cols, |body, st| {
        bound_row(
            body,
            st,
            "dimension",
            &mut s.dimension,
            DIMENSION_LIMITS,
            0.01,
            2,
            "The similarity dimension (the status bar's d): about 1 is wiry, 2 a sheet, toward 3 a \
             volume. Met by resizing all the transforms together, so it costs almost nothing, but \
             the size range stops applying. Only while size is on. Variations aren't counted.",
        );
        bound_row(
            body,
            st,
            "lacunarity",
            &mut s.lacunarity,
            LACUNARITY_LIMITS,
            0.1,
            1,
            "How gappy the attractor is across scales (the status bar's Λ): 1 is as even as random \
             dust, higher is clumpier. Default rolls run about 7 to 80, median 16. Met by drawing \
             again until one lands inside, so a narrow bound costs tries.",
        );
    });

    ui.horizontal(|ui| {
        ui.label("tries");
        let resp = num::drag(ui, 5, 0, egui::DragValue::new(&mut s.attempts).range(1..=MAX_ATTEMPTS).speed(1.0));
        hinted(
            resp,
            st,
            "Candidates tried before a roll or mutation comes up empty. About a millisecond each.",
            "drag: tries per roll or mutation",
        );
    });

    // What the list has measured under these settings, so a bound that is
    // too tight shows up as a number rather than as a list that stops growing.
    if list.settings().same_roll(s) && list.candidates > 0 {
        let flames = list.entries.len() as u64;
        let text = format!(
            "The list found {} flame{} in {} candidates{}.",
            flames,
            if flames == 1 { "" } else { "s" },
            list.candidates,
            if list.rejected.total() > 0 {
                format!(" — thrown away: {}", list.rejected.describe())
            } else {
                String::new()
            },
        );
        caption(ui, &text);
    }
}

/// How big a step a mutation takes — settings a roll doesn't read. (How many
/// parts it changes is a range, so it's in Ranges.)
fn mutation(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    let m = &mut s.mutation;
    let cols = [
        Col::name("Setting", "How a mutation steps. A roll draws every part that's on, so none of this applies to it."),
        Col::new("Value", "", 60.0),
    ];
    table::show(ui, st, "fracturize_dice_mutation", &cols, |body, st| {
        let h = table::row_height(body.ui_mut());
        body.row(h, |mut row| {
            let tip = "How far each nudge moves the scene: 1 is a noticeable but usually safe step. \
                       The Mutate window's slider is this same setting.";
            name_cell(&mut row, st, true, "strength", tip);
            row.col(|ui| {
                let dv = egui::DragValue::new(&mut m.strength).range(STRENGTH_LIMITS[0]..=STRENGTH_LIMITS[1]).speed(0.01);
                hinted(num::drag(ui, 5, 2, dv), st, tip, "drag: mutation strength");
            });
        });
        body.row(h, |mut row| {
            let tip = "Halve the strength after this many failed tries in a row, down to a sixteenth, \
                       so a delicate scene still gets something close. 0 never eases off.";
            name_cell(&mut row, st, true, "ease off after (failed tries)", tip);
            row.col(|ui| {
                let dv = egui::DragValue::new(&mut m.backoff).range(0..=MAX_ATTEMPTS).speed(0.1);
                hinted(num::drag(ui, 5, 0, dv), st, tip, "drag: failed tries before the strength halves");
            });
        });
        body.row(h, |mut row| {
            let tip = "Resize a mutation onto the dimension bound, as a roll is. Off, a mutation only \
                       has to stay as close to the bounds as the scene it came from.";
            name_cell(&mut row, st, true, "steer to dimension bound", tip);
            row.col(|ui| {
                let resp = ui.checkbox(&mut m.steer_dimension, "");
                hinted(resp, st, tip, "click: steer mutations onto the dimension bound");
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window edits the map by name; every variation the generator can
    /// roll must be in it, and linear never.
    #[test]
    fn every_rollable_variation_has_a_row() {
        let s = RandomSettings::default();
        for name in VARIATION_NAMES.iter().skip(1) {
            assert!(s.variations.contains_key(*name), "{name} has no row");
        }
        assert!(!s.variations.contains_key("linear"));
        assert_eq!(RandomSettings::default().normalized(), s);
    }
}
