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
    MAX_TRANSFORMS, ODDS_LIMIT, SCALE_LIMITS, STRENGTH_LIMITS, WEIGHT_LIMIT,
};
use crate::scene::{NUM_VARIATIONS, VARIATION_NAMES};

use super::hints::hinted;
use super::icons;
use super::num;

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
                parts(ui, &mut app.ui_state, &mut s);
                ui.add_space(8.0);
                bounds(ui, &mut app.ui_state, &mut s, &app.random_list);
                ui.add_space(8.0);
                mutation(ui, &mut app.ui_state, &mut s);
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

/// Width of a part row's name column, so the numbers line up down the list.
const NAME_W: f32 = 150.0;

/// The parts, grouped the way people think of a flame — its shape, how it's
/// put together, its colour — each followed by the ranges it is drawn from.
/// Indices into [`Parts::NAMES`]. The colour group is exactly what
/// `colour-only` changes and `no-colour` leaves alone.
const SHAPE: [usize; 5] = [0, 1, 2, 3, 4];
const STRUCTURE: [usize; 2] = [5, 6];
const COLOUR: [usize; 3] = [7, 8, 9];

fn parts(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    heading(ui, "What the dice may change");
    ui.label(
        egui::RichText::new(
            "A roll draws every part that's on, a mutation changes a few. Off, both keep the scene's own.",
        )
        .small()
        .weak(),
    );
    ui.add_space(2.0);
    // Column heads, over the numbers only a mutation reads.
    ui.horizontal(|ui| {
        ui.add_space(NAME_W + ui.spacing().item_spacing.x);
        let head = |ui: &mut egui::Ui, st: &mut super::UiState, text: &str, chars: usize, tip: &str| {
            let resp = num::small_cell(ui, text, chars);
            hinted(resp, st, tip, text);
        };
        head(ui, st, "odds", 5, "How often a mutation picks this part, against the other parts' odds. A roll draws every part that's on, whatever its odds.");
        head(ui, st, "share", 5, "This part's share of a mutation's changes, from the odds");
        head(ui, st, "re-roll", 5, "Chance a mutation replaces the part outright, drawn the way a roll draws it, rather than nudging what's there");
    });
    let total: f32 = s.parts.all().iter().map(Part::live_odds).sum();

    group_heading(ui, "Shape");
    for k in SHAPE {
        part_row(ui, st, s, k, total);
        match k {
            2 => ranges(ui, s.parts.scale.on, "size", |ui| size_ranges(ui, st, s)),
            4 => ranges(ui, s.parts.variations.on, "variations", |ui| variation_ranges(ui, st, s)),
            _ => {}
        }
    }
    group_heading(ui, "Structure");
    for k in STRUCTURE {
        part_row(ui, st, s, k, total);
        match k {
            5 => ranges(ui, s.parts.count.on, "count", |ui| count_ranges(ui, st, s)),
            6 => ranges(ui, s.parts.symmetry.on, "symmetry", |ui| symmetry_ranges(ui, st, s)),
            _ => {}
        }
    }
    group_heading(ui, "Colour");
    for k in COLOUR {
        part_row(ui, st, s, k, total);
    }
}

fn group_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text).small().strong());
}

/// A part's ranges, indented under it, and greyed while it's off — a part
/// that's off is kept from the scene, so nothing about drawing it applies.
fn ranges(ui: &mut egui::Ui, on: bool, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.indent(("fracturize_part_ranges", id), |ui| {
        ui.add_enabled_ui(on, add);
    });
}

/// One part: its switch, then the numbers a mutation reads.
fn part_row(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings, k: usize, total: f32) {
    let name = Parts::NAMES[k];
    let part = &mut *s.parts.all_mut()[k];
    ui.horizontal(|ui| {
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(egui::vec2(NAME_W, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(NAME_W);
            let resp = ui.checkbox(&mut part.on, part_label(name));
            let tip = format!(
                "{} Off, the dice leave it as the scene has it, in a roll and a mutation alike.",
                part_tip(name)
            );
            hinted(resp, st, &tip, if part.on { "click: keep this as it is" } else { "click: let the dice change this" });
        });
        ui.add_enabled_ui(part.on, |ui| {
            let resp = num::drag(ui, 5, 2, egui::DragValue::new(&mut part.odds).range(0.0..=ODDS_LIMIT).speed(0.005));
            hinted(
                resp,
                st,
                "How often a mutation picks this, against the other parts' odds. At 0 a mutation \
                 never does, though a roll still draws it.",
                "drag: odds a mutation changes this",
            );
            let text = match (part.on, part.odds > 0.0 && total > 0.0) {
                (false, _) => String::new(),
                (true, false) => "rolls".to_string(),
                (true, true) => format!("{}%", num::fixed(100.0 * part.odds / total, 0)),
            };
            let resp = num::small_cell(ui, &text, 5);
            hinted(
                resp,
                st,
                "Its share of a mutation's changes. \"rolls\": a roll draws it, a mutation never picks it.",
                "share of changes",
            );
            chance(
                ui,
                st,
                &mut part.reroll,
                "Chance a mutation replaces this outright, drawn the way a roll draws it (from \
                 the ranges here), rather than nudging what's there",
                "drag: chance of a re-roll",
            );
        });
    });
}

/// How big a step a mutation takes — the only settings a roll doesn't read.
fn mutation(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    heading(ui, "Each mutation");
    let m = &mut s.mutation;
    egui::Grid::new("fracturize_random_mutate").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("strength");
        let resp = num::drag(
            ui,
            5,
            2,
            egui::DragValue::new(&mut m.strength).range(STRENGTH_LIMITS[0]..=STRENGTH_LIMITS[1]).speed(0.01),
        );
        hinted(
            resp,
            st,
            "How far each nudge moves the scene: 1 is a noticeable but usually safe step. The \
             Mutate window's slider is this same setting.",
            "drag: mutation strength",
        );
        ui.end_row();

        ui.label("changes");
        count_range(
            ui,
            st,
            &mut m.changes,
            1,
            MAX_CHANGES,
            "How many parts one mutation changes, each picked by the odds above",
            "drag: changes per mutation",
        );
        ui.end_row();

        ui.label("ease off after");
        ui.horizontal(|ui| {
            let resp = num::drag(ui, 5, 0, egui::DragValue::new(&mut m.backoff).range(0..=MAX_ATTEMPTS).speed(0.1));
            hinted(
                resp,
                st,
                "Halve the strength after this many failed tries in a row, down to a sixteenth, \
                 so a delicate scene still gets something close. 0 never eases off.",
                "drag: failed tries before the strength halves",
            );
            ui.label("failed tries");
        });
        ui.end_row();

        ui.label("");
        let resp = ui.checkbox(&mut m.steer_dimension, "steer dimension");
        hinted(
            resp,
            st,
            "Resize a mutation onto the dimension bound, as a roll is. Off, a mutation only has \
             to stay as close to the bounds as the scene it came from.",
            "click: steer mutations onto the dimension bound",
        );
        ui.end_row();
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
        "scale" => "Each transform's size, from the range under it.",
        "weight" => "How often each transform is picked by the chaos game.",
        "variations" => "Each transform's variations, from the list under it.",
        "count" => "How many transforms: a roll draws it from the range under it, a mutation adds or removes one within it.",
        "symmetry" => "The symmetry group, from the settings under it. A roll with no chance of one draws none.",
        "color" => "The colours: each transform's, the colour source and the gradient.",
        "color_value" => "Where each transform sits on the gradient.",
        "background" => "The background colour, drawn from the flame's own colours.",
        _ => "",
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong());
}

fn count_ranges(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    egui::Grid::new("fracturize_random_count").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("per flame");
        count_range(
            ui,
            st,
            &mut s.transforms,
            1,
            MAX_TRANSFORMS,
            "How many transforms a flame has. Three and four are where structure stays \
             legible; past six it tends toward fog. A mutation stays inside this too.",
            "drag: transforms per flame",
        );
        ui.end_row();
    });
}

fn size_ranges(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    egui::Grid::new("fracturize_random_size").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("range");
        float_range(
            ui,
            st,
            &mut s.scale,
            SCALE_LIMITS,
            0.005,
            2,
            "Each transform's size, drawn evenly on a log scale. A dimension bound, when on, \
             resizes all the transforms together and wins over this.",
            "drag: transform size range",
        );
        ui.end_row();

        ui.label("squashed");
        chance(
            ui,
            st,
            &mut s.squash,
            "Chance a transform has one axis squashed. That's what turns blobs into sheets and ribbons.",
            "drag: chance of a squashed axis",
        );
        ui.end_row();
    });
}

fn variation_ranges(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    egui::Grid::new("fracturize_random_kinds").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("kinds");
        count_range(
            ui,
            st,
            &mut s.kinds,
            1,
            NUM_VARIATIONS as u32 - 1,
            "How many different variations one flame draws from. One kind across several \
             transforms reads as a single form; many read as unrelated distortions.",
            "drag: variation kinds per flame",
        );
        ui.end_row();

        ui.label("per transform");
        count_range(
            ui,
            st,
            &mut s.per_map,
            0,
            NUM_VARIATIONS as u32 - 1,
            "Variations on each transform, from the flame's kinds. At 0 some stay plain \
             affine; at least one transform always gets one.",
            "drag: variations per transform",
        );
        ui.end_row();

        ui.label("affine only");
        chance(
            ui,
            st,
            &mut s.affine,
            "Chance a flame has no variations at all",
            "drag: chance of a pure affine flame",
        );
        ui.end_row();

        ui.label("negative");
        chance(
            ui,
            st,
            &mut s.negative,
            "Chance a variation's weight comes out negative, Apophysis-style",
            "drag: chance of a negative weight",
        );
        ui.end_row();
    });

    ui.add_space(4.0);
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
        let resp = hinted(
            resp,
            st,
            "Switch every variation off: every flame comes out affine",
            "click: roll no variations",
        );
        if resp.clicked() {
            s.variations.values_mut().for_each(|v| v.enabled = false);
        }
    });

    // The long list folds away; its count says what's in it while folded.
    let enabled = s.variations.values().filter(|v| v.enabled).count();
    let title = format!("which variations ({} of {})", enabled, s.variations.len());
    egui::CollapsingHeader::new(title).id_salt("fracturize_random_variation_list").show(ui, |ui| {
    egui::Grid::new("fracturize_random_variations")
        .num_columns(2)
        .spacing([6.0, 2.0])
        .striped(true)
        .show(ui, |ui| {
            for name in VARIATION_NAMES.iter().skip(1) {
                let Some(v) = s.variations.get_mut(*name) else { continue };
                let resp = ui.checkbox(&mut v.enabled, *name);
                hinted(
                    resp,
                    st,
                    variation_tip(name),
                    if v.enabled { "click: stop rolling this variation" } else { "click: roll this variation" },
                );
                ui.add_enabled_ui(v.enabled, |ui| {
                    float_range(
                        ui,
                        st,
                        &mut v.weight,
                        [0.0, WEIGHT_LIMIT],
                        0.005,
                        2,
                        "This variation's weight when it is rolled, as a magnitude — the \
                         sign is the negative chance's. Linear takes up what's left of 1.",
                        "drag: weight range",
                    );
                });
                ui.end_row();
            }
        });
    });
}

fn symmetry_ranges(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    let sym = &mut s.symmetry;
    egui::Grid::new("fracturize_random_symmetry").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("chance");
        chance(
            ui,
            st,
            &mut sym.chance,
            "Chance a roll gives the flame a symmetry group. At 0 none do. Its transforms \
             shrink as they join, so sixty copies under Icosa don't fill space into a lump.",
            "drag: chance of a symmetric flame",
        );
        ui.end_row();
    });

    // Which groups, and how they're applied: elaboration on the chance above,
    // so it folds away and the chance doesn't.
    let kinds = [sym.cyclic, sym.dihedral, sym.tetra, sym.octa, sym.icosa, sym.repeat];
    let title = format!("which groups ({} of {})", kinds.iter().filter(|&&k| k).count(), kinds.len());
    egui::CollapsingHeader::new(title).id_salt("fracturize_random_symmetry_kinds").show(ui, |ui| {
    egui::Grid::new("fracturize_random_symmetry_kinds_grid").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        let kind = |ui: &mut egui::Ui, st: &mut super::UiState, on: &mut bool, name: &str, tip: &str| {
            let resp = ui.checkbox(on, name);
            hinted(resp, st, tip, "click: roll this kind of symmetry");
        };

        ui.label("rotations");
        ui.horizontal(|ui| {
            kind(ui, st, &mut sym.cyclic, "Cyc", "n-fold about a random axis — the mandala");
            kind(ui, st, &mut sym.dihedral, "Dih", "n-fold plus the half-turn flip, so it has a front and a back");
        });
        ui.end_row();

        ui.label("fold");
        ui.add_enabled_ui(sym.cyclic || sym.dihedral, |ui| {
            count_range(ui, st, &mut sym.fold, 2, crate::symmetry::MAX_FOLD, "The n of Cyc and Dih", "drag: fold range");
        });
        ui.end_row();

        ui.label("polyhedra");
        ui.horizontal(|ui| {
            kind(ui, st, &mut sym.tetra, "Tetra", "The tetrahedron's 12 rotations");
            kind(ui, st, &mut sym.octa, "Octa", "The cube's 24 rotations");
            kind(ui, st, &mut sym.icosa, "Icosa", "The icosahedron's 60 rotations");
        });
        ui.end_row();

        ui.label("repeat");
        ui.horizontal(|ui| {
            kind(
                ui,
                st,
                &mut sym.repeat,
                "",
                "Copies stepped up a tapering helix. Not a group — it repeats without being \
                 symmetric — but the same machinery",
            );
            ui.add_enabled_ui(sym.repeat, |ui| {
                count_range(
                    ui,
                    st,
                    &mut sym.repeat_count,
                    2,
                    crate::symmetry::MAX_REPEAT,
                    "Copies in a repeat, counting the motif itself",
                    "drag: repeat copies",
                );
            });
        });
        ui.end_row();

        ui.label("outside");
        count_range(
            ui,
            st,
            &mut sym.free,
            0,
            MAX_TRANSFORMS,
            "Transforms kept out of the group. One is the useful default: a wholly \
             symmetric flame reads as wallpaper, and one transform outside the group gives \
             it somewhere to look. At least one transform always joins.",
            "drag: transforms outside the group",
        );
        ui.end_row();

        ui.label("mirror");
        chance(
            ui,
            st,
            &mut sym.mirror,
            "Chance a group is extended by the central inversion, doubling its copies",
            "drag: chance of a mirrored group",
        );
        ui.end_row();

        ui.label("orbit colour");
        chance(
            ui,
            st,
            &mut sym.orbit_color,
            "Chance the colour is offset by which group element was drawn — an \
             interference pattern through the form, rather than every copy alike",
            "drag: chance of orbit colouring",
        );
        ui.end_row();
    });
    });
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
    heading(ui, "Quality check");
    ui.label(
        egui::RichText::new("Every roll and mutation passes these before you see it.").small().weak(),
    );
    egui::Grid::new("fracturize_random_bounds").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
        bound(
            ui,
            st,
            "dimension",
            &mut s.dimension,
            DIMENSION_LIMITS,
            0.01,
            2,
            "The similarity dimension (the status bar's d): about 1 is wiry, 2 a sheet, \
             toward 3 a volume. Met by resizing all the transforms together, so it costs \
             almost nothing, but the size range stops applying. Only when size is on. \
             Variations aren't counted.",
        );
        ui.end_row();
        bound(
            ui,
            st,
            "lacunarity",
            &mut s.lacunarity,
            LACUNARITY_LIMITS,
            0.1,
            1,
            "How gappy the attractor is across scales (the status bar's Λ): 1 is as \
             even as random dust, higher is clumpier. Default rolls run about 7 to 80, \
             median 16. Met by rolling again until one lands inside, so a narrow bound \
             costs tries — watch the count below.",
        );
        ui.end_row();

        ui.label("tries");
        let resp = num::drag(
            ui,
            5,
            0,
            egui::DragValue::new(&mut s.attempts).range(1..=MAX_ATTEMPTS).speed(1.0),
        );
        hinted(
            resp,
            st,
            "Candidates tried before a roll or mutation comes up empty. About a \
             millisecond each.",
            "drag: tries per flame",
        );
        ui.end_row();
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
        ui.label(egui::RichText::new(text).small().weak());
    }
}

#[allow(clippy::too_many_arguments)]
fn bound(
    ui: &mut egui::Ui,
    st: &mut super::UiState,
    name: &str,
    b: &mut Bound,
    limits: [f32; 2],
    speed: f64,
    decimals: usize,
    tip: &str,
) {
    let resp = ui.checkbox(&mut b.on, name);
    hinted(
        resp,
        st,
        tip,
        if b.on { "click: stop bounding this" } else { "click: bound this" },
    );
    ui.add_enabled_ui(b.on, |ui| {
        float_range(ui, st, &mut b.range, limits, speed, decimals, tip, "drag: bound range");
    });
}

/// `min – max` as two fields that can't cross.
fn count_range(
    ui: &mut egui::Ui,
    st: &mut super::UiState,
    r: &mut [u32; 2],
    lo: u32,
    hi: u32,
    tip: &str,
    hint: &str,
) {
    ui.horizontal(|ui| {
        let max = r[1];
        let resp = num::drag(ui, 3, 0, egui::DragValue::new(&mut r[0]).range(lo..=max).speed(0.05));
        hinted(resp, st, tip, hint);
        ui.label("to");
        let min = r[0];
        let resp = num::drag(ui, 3, 0, egui::DragValue::new(&mut r[1]).range(min..=hi).speed(0.05));
        hinted(resp, st, tip, hint);
    });
}

#[allow(clippy::too_many_arguments)]
fn float_range(
    ui: &mut egui::Ui,
    st: &mut super::UiState,
    r: &mut [f32; 2],
    limits: [f32; 2],
    speed: f64,
    decimals: usize,
    tip: &str,
    hint: &str,
) {
    let chars = if limits[1] >= 100.0 { 7 } else { 5 };
    // One row, so it occupies one grid cell wherever it's put.
    ui.horizontal(|ui| {
        let max = r[1];
        let resp = num::drag(ui, chars, decimals, egui::DragValue::new(&mut r[0]).range(limits[0]..=max).speed(speed));
        hinted(resp, st, tip, hint);
        ui.label("to");
        let min = r[0];
        let resp = num::drag(ui, chars, decimals, egui::DragValue::new(&mut r[1]).range(min..=limits[1]).speed(speed));
        hinted(resp, st, tip, hint);
    });
}

fn chance(ui: &mut egui::Ui, st: &mut super::UiState, v: &mut f32, tip: &str, hint: &str) {
    let resp = num::drag(
        ui,
        5,
        2,
        egui::DragValue::new(v).range(0.0..=1.0).speed(0.005),
    );
    hinted(resp, st, tip, hint);
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
