//! Random settings: what the dice may roll (`dice::RandomSettings`).
//!
//! Three groups — the maps, the variations they carry, and bounds on the
//! attractor — then the budget. Edits apply to the next roll of the dice and
//! restart the Random flames list once they stop moving. They are a
//! preference, not scene data: they follow the person across scenes, and
//! nothing here is undoable by Ctrl+Z, which is for the artwork.
//!
//! Every range is two fields, min and max, and each field's own range stops at
//! the other's value, so a range can be narrowed to a point but never
//! inverted under the pointer.

use crate::app::App;
use crate::dice::{
    Bound, RandomSettings, DIMENSION_LIMITS, LACUNARITY_LIMITS, MAX_ATTEMPTS, MAX_OPERATORS, ODDS_LIMIT,
    MAX_TRANSFORMS, SCALE_LIMITS, STRENGTH_LIMITS, WEIGHT_LIMIT,
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
    let mut reset = false;
    let mut action = None;

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
                    let arm = app.ui_state.random_reset_arm;
                    let (arm, fired) = super::confirm::danger_button(
                        ui,
                        &mut app.ui_state,
                        arm,
                        icons::RESET,
                        "Defaults",
                        "Reset all settings",
                        "Put every setting here back to how it shipped",
                        "Every setting in this window goes back to how it shipped. Ctrl+Z \
                         won't bring them back — these are preferences, not the scene.",
                    );
                    app.ui_state.random_reset_arm = arm;
                    reset = fired;
                    let resp = ui.button((icons::DICE, "Roll one"));
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Roll a flame with these settings and open it — one Ctrl+Z away \
                         from what you have now",
                        "click: roll a random flame",
                    );
                    if resp.clicked() {
                        app.random_flame();
                    }
                });
                ui.add_space(2.0);
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                maps(ui, &mut app.ui_state, &mut s);
                ui.add_space(6.0);
                variations(ui, &mut app.ui_state, &mut s);
                ui.add_space(6.0);
                symmetry(ui, &mut app.ui_state, &mut s);
                ui.add_space(6.0);
                bounds(ui, &mut app.ui_state, &mut s, &app.random_list);
                ui.add_space(6.0);
                mutation(ui, &mut app.ui_state, &mut s);
            });
        });
    super::remember(ctx, app, super::WindowKey::RandomSettings);

    if reset {
        s = RandomSettings::default();
    }
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
    app.ui_state.panels.random_settings_open = open;
}

enum ProfileAction {
    Load(crate::dice::profile::Entry),
    Revert,
    Save,
    SaveAs(String),
}

/// Which profile these settings are, and the document verbs for it: pick one,
/// Save, Save as…, Revert. The name carries a `*` while the settings differ
/// from the profile, as a scene's does.
fn profile_bar(ui: &mut egui::Ui, app: &mut App, s: &RandomSettings) -> Option<ProfileAction> {
    use crate::dice::profile;
    let mut action = None;
    let selected = app.dice_profile().clone();
    let modified = selected.modified(s);
    let caption = format!("{}{}", selected.entry.name, if modified { " *" } else { "" });
    ui.horizontal(|ui| {
        ui.label("Profile");
        let combo = egui::ComboBox::from_id_salt("fracturize_dice_profile")
            .selected_text(caption)
            .show_ui(ui, |ui| {
                for entry in profile::list() {
                    let label = if entry.shared { format!("{}  (shared)", entry.name) } else { entry.name.clone() };
                    let current = entry == selected.entry;
                    if ui.selectable_label(current, label).clicked() {
                        action = Some(ProfileAction::Load(entry));
                    }
                }
            });
        hinted(
            combo.response,
            &mut app.ui_state,
            "Load a dice profile: what the dice may roll and a mutation may change, saved. \
             Yours live in ~/.config/fracturize/dice, shared ones in scenes/dice; `default` \
             is the shipped settings. --random-config takes the same names.",
            "click: load a dice profile",
        );

        let writable = selected.writable();
        let resp = ui.add_enabled(writable && modified, egui::Button::new((icons::SAVE, "Save")));
        let tip = if !writable {
            "The built-in and shared profiles can't be saved over — Save as makes one of your own"
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
        ui.label(egui::RichText::new("The profile's file is gone or no longer reads; Save as keeps these settings.").small().weak());
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

/// What a mutation may change: how hard, how much, and the odds of each part.
fn mutation(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    heading(ui, "Mutation");
    let m = &mut s.mutate;
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
            "How much each nudge changes the scene: 1 is a noticeable but usually safe step. \
             The Mutate window's slider is this same setting.",
            "drag: mutation strength",
        );
        ui.end_row();

        ui.label("parts");
        count_range(
            ui,
            st,
            &mut m.operators,
            1,
            MAX_OPERATORS,
            "How many parts one mutation changes",
            "drag: parts changed per mutation",
        );
        ui.end_row();

        ui.label("back off");
        let resp = num::drag(ui, 5, 0, egui::DragValue::new(&mut m.backoff).range(0..=MAX_ATTEMPTS).speed(0.1));
        hinted(
            resp,
            st,
            "Halve the strength after this many rejected tries in a row, down to a sixteenth, \
             so a delicate scene still gives something close. 0 never backs off.",
            "drag: rejections before the strength halves",
        );
        ui.end_row();

        ui.label("");
        let resp = ui.checkbox(&mut m.steer_dimension, "steer dimension");
        hinted(
            resp,
            st,
            "Rescale a mutation onto the dimension bound, as a fresh roll is. Off, a mutation \
             is judged against the scene it came from instead: no further out of bounds than \
             its parent was.",
            "click: steer mutations onto the dimension bound",
        );
        ui.end_row();
    });

    ui.add_space(4.0);
    egui::Grid::new("fracturize_random_parts").num_columns(3).spacing([6.0, 2.0]).striped(true).show(ui, |ui| {
        ui.label(egui::RichText::new("part").small().weak());
        ui.label(egui::RichText::new("odds").small().weak());
        ui.label(egui::RichText::new("re-roll").small().weak());
        ui.end_row();
        for (name, part) in crate::dice::Parts::NAMES.iter().zip(m.parts.all_mut()) {
            ui.label(name.replace('_', " "));
            let resp = num::drag(ui, 5, 2, egui::DragValue::new(&mut part.odds).range(0.0..=ODDS_LIMIT).speed(0.005));
            hinted(
                resp,
                st,
                &format!("{} Relative to the other parts' odds; 0 never touches it.", part_tip(name)),
                "drag: odds of changing this part",
            );
            chance(
                ui,
                st,
                &mut part.reroll,
                "Chance a change re-rolls this part the way a fresh roll would, under the \
                 settings above, rather than nudging what's there",
                "drag: chance of a re-roll",
            );
            ui.end_row();
        }
    });
}

fn part_tip(name: &str) -> &'static str {
    match name {
        "translate" => "A map's position.",
        "rotate" => "A map's turn about its own origin.",
        "scale" => "A map's size; re-rolled from the scale range above.",
        "weight" => "A map's chaos-game weight.",
        "variations" => "A map's variation blend; new ones only from the enabled list above.",
        "color" => "A map's colour, or the gradient in palette mode.",
        "color_value" => "Where a map sits on the gradient.",
        "add" => "A new map: a nudged copy, or (re-rolled) a fresh one. Never past the most maps above.",
        "remove" => "A map deleted. Never below the fewest maps above.",
        "background" => "The background colour.",
        "symmetry" => "The symmetry group: its axis tilts, or another is drawn from the settings above.",
        _ => "",
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong());
}

fn maps(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    heading(ui, "Maps");
    egui::Grid::new("fracturize_random_maps").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("per flame");
        count_range(
            ui,
            st,
            &mut s.transforms,
            1,
            MAX_TRANSFORMS,
            "How many maps a flame has. Three and four are where structure stays legible; \
             past six it tends toward fog.",
            "drag: maps per flame",
        );
        ui.end_row();

        ui.label("scale");
        float_range(
            ui,
            st,
            &mut s.scale,
            SCALE_LIMITS,
            0.005,
            2,
            "Each map's size, drawn evenly on a log scale. A dimension bound, when on, \
             rescales all the maps together and wins over this.",
            "drag: map scale range",
        );
        ui.end_row();

        ui.label("squashed");
        chance(
            ui,
            st,
            &mut s.squash,
            "Chance a map has one axis squashed — what turns blobs into sheets and ribbons",
            "drag: chance of a squashed axis",
        );
        ui.end_row();
    });
}

fn variations(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    heading(ui, "Variations");
    egui::Grid::new("fracturize_random_kinds").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("kinds");
        count_range(
            ui,
            st,
            &mut s.kinds,
            1,
            NUM_VARIATIONS as u32 - 1,
            "How many different variations one flame draws from. One kind across several \
             maps reads as a single form; many read as unrelated distortions.",
            "drag: variation kinds per flame",
        );
        ui.end_row();

        ui.label("per map");
        count_range(
            ui,
            st,
            &mut s.per_map,
            0,
            NUM_VARIATIONS as u32 - 1,
            "Variations on each map, from the flame's kinds. At 0 some maps stay plain \
             affine; at least one map always gets one.",
            "drag: variations per map",
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
}

fn symmetry(ui: &mut egui::Ui, st: &mut super::UiState, s: &mut RandomSettings) {
    heading(ui, "Symmetry");
    let sym = &mut s.symmetry;
    egui::Grid::new("fracturize_random_symmetry").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("chance");
        chance(
            ui,
            st,
            &mut sym.chance,
            "Chance a flame takes a symmetry group. At 0 none do. Its motifs shrink as they \
             join, so sixty copies under Icosa don't fill space into a lump.",
            "drag: chance of a symmetric flame",
        );
        ui.end_row();

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
            "Maps kept out of the group. One is the useful default: an orbit spreads \
             measure evenly by construction, so a wholly symmetric flame reads as \
             wallpaper, and one map outside the group gives it somewhere to look. At \
             least one map always joins.",
            "drag: maps outside the group",
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
}

fn variation_tip(name: &str) -> String {
    let note = match name {
        "spherical" => " Its 1/r² inversion sprays far outliers; most rolls with it fail the shape gate.",
        "hyperbolic" | "tangent" => " Unbounded: expect more rejections.",
        "absfold" | "boxfold" | "spherefold" | "bulb" => " A fold or escape-time import — bounded.",
        _ => "",
    };
    format!("Roll {name} on this flame's maps.{note}")
}

fn bounds(
    ui: &mut egui::Ui,
    st: &mut super::UiState,
    s: &mut RandomSettings,
    list: &crate::random_list::RandomList,
) {
    heading(ui, "Bounds");
    egui::Grid::new("fracturize_random_bounds").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
        bound(
            ui,
            st,
            "dimension",
            &mut s.dimension,
            DIMENSION_LIMITS,
            0.01,
            2,
            "The similarity dimension of the maps' affine parts (the status bar's d): \
             about 1 is wiry, 2 a sheet, toward 3 a volume. Met by rescaling the maps \
             together onto a value inside the bound, so it costs almost nothing — but \
             the scale range above stops applying. Variations aren't counted.",
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
            "Candidates tried per flame before the roll comes up empty. About a \
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
