//! The Subscenes window: the layers of a composite scene.
//!
//! One row per layer — the scene's own maps first, then each subscene — with
//! the eye that shows or hides it, its share of the point budget, and how
//! many maps it has. Clicking a subscene's row selects it *as a whole*: the
//! viewport then shows one gizmo, its placement, and dragging that moves,
//! turns or resizes the entire attractor. Below the list, the selected
//! subscene's placement, colour source and provenance.
//!
//! Import adds any scene file as a subscene. It is recorded as a `file =`
//! reference, so it costs four lines in the saved scene and never copies the
//! artwork; edit its maps and the save writes them inline instead, leaving the
//! original file alone. See `src/subscene.rs` for the model.

use crate::app::App;
use crate::scene::ColorMode;

use super::hints::hinted;
use super::icons;
use super::num;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.subscenes_open;
    if !open {
        return;
    }
    super::window(ctx, app, super::WindowKey::Subscenes, "Subscenes")
        .open(&mut open)
        .show(ctx, |ui| {
            draw_actions(ui, app);
            ui.separator();
            draw_layers(ui, app);
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("fracturize_subscene_detail")
                .auto_shrink([false, true])
                .show(ui, |ui| draw_detail(ui, app));
        });
    super::remember(ctx, app, super::WindowKey::Subscenes);
    app.ui_state.panels.subscenes_open = open;
}

/// Import (a menu of scene files) and Remove, at the top: they act on the list.
fn draw_actions(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let zoomed = app.scene.zoom.is_some();
        let full = app.scene.layer_count() >= crate::subscene::MAX_LAYERS;
        let mut picked = None;
        let resp = ui
            .add_enabled_ui(!zoomed && !full, |ui| {
                ui.menu_button((icons::PLUS, "Import scene"), |ui| {
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
                "Subscenes don't work with infinite zoom yet. Turn infinite zoom off in the Render window first"
            } else if full {
                "A scene can hold at most 15 subscenes"
            } else {
                "Place another scene inside this one. It keeps its own transforms and colours, and gets an equal share of the points"
            },
            "click: choose a scene to place in this one",
        );
        if let Some(f) = picked {
            app.ui_state.import_error = app.import_subscene(&f).err();
        }

        let selected = app.selected_subscene();
        let resp = ui.add_enabled(selected.is_some(), egui::Button::new((icons::TRASH, "Remove")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if selected.is_some() {
                "Remove the selected subscene from this scene. Its own scene file isn't touched"
            } else {
                "Select a subscene to remove it"
            },
            "click: remove the selected subscene",
        );
        if resp.clicked() {
            if let Some(k) = selected {
                app.remove_subscene(k);
            }
        }
    });
    if let Some(e) = app.ui_state.import_error.clone() {
        ui.label(egui::RichText::new(e).small().color(super::theme::warn(ui.visuals())));
    }
}

/// One row per layer: eye, name, share, map count.
fn draw_layers(ui: &mut egui::Ui, app: &mut App) {
    if !app.scene.is_composite() {
        ui.label(
            egui::RichText::new(
                "Import a scene to place it alongside this one. The two stay \
                 separate fractals and never blend into each other.",
            )
            .small()
            .weak(),
        );
    }
    let sizes = app.scene.layer_sizes();
    let total: f32 = (0..app.scene.layer_count())
        .filter(|&k| app.scene.layer_visible(k) && sizes[k] > 0)
        .map(|k| app.scene.layer_share(k))
        .sum();
    let current = app.current_layer();
    let whole = app.selected_subscene();

    egui::Grid::new("fracturize_subscene_rows").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
        for k in 0..app.scene.layer_count() {
            let visible = app.scene.layer_visible(k);

            // Eye: show/hide; alt+click solos, as a transform's eye does.
            let icon = if visible { icons::EYE } else { icons::EYE_SLASH };
            let can = !visible || app.can_hide_layer(k);
            let resp = ui.add_enabled(can, egui::Button::new(icon).small().frame(false));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if can {
                    "Show or hide this. Hidden, its points go to the others.\n\nAlt+click to show only this one; alt+click again to show them all"
                } else {
                    "Something has to stay visible. Show another one first"
                },
                "click: show or hide · alt+click: show only this one",
            );
            if resp.clicked() {
                if ui.input(|i| i.modifiers.alt) {
                    app.solo_layer(k);
                } else {
                    app.set_layer_visible(k, !visible);
                }
            }

            // Name: selects. A subscene is selected as a whole (its gizmo
            // moves all of it); the scene's own layer just lists its maps.
            let name = if k == 0 {
                format!("{} (this scene)", app.scene.layer_name(0))
            } else {
                app.scene.layer_name(k).to_string()
            };
            let is_sel = whole == Some(k) || (whole.is_none() && k == 0 && current == 0 && app.scene.is_composite());
            let text = if visible { egui::RichText::new(name) } else { egui::RichText::new(name).weak() };
            let resp = ui.selectable_label(is_sel, text);
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if k == 0 {
                    "This scene's own transforms. Click to list them in the Transforms window"
                } else {
                    "Select this subscene. Its gizmo in the viewport moves, turns and resizes the whole thing, and the Transforms window lists its transforms"
                },
                "click: select this",
            );
            if resp.clicked() {
                app.select_layer(k);
            }

            // Share, with what it comes to.
            let mut share = app.scene.layer_share(k);
            let resp = num::drag(
                ui,
                5,
                2,
                num::Drag::new(&mut share).range(0.0..=100.0).speed(0.01),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "How many of the points this one gets, compared with the others. A small subscene needs fewer to look as dense",
                "drag: change its share of the points",
            );
            if resp.changed() {
                app.set_layer_share(k, share);
            }

            let pct = if visible && sizes[k] > 0 && total > 0.0 { app.scene.layer_share(k) / total * 100.0 } else { 0.0 };
            num::small_cell(ui, &format!("{:>5.1}%  {:>3} transforms", pct, sizes[k]), 22);
            ui.end_row();
        }
    });
}

/// The selected subscene: name, placement, colour, where it came from.
fn draw_detail(ui: &mut egui::Ui, app: &mut App) {
    let Some(k) = app.selected_subscene() else {
        if app.scene.is_composite() {
            ui.label(
                egui::RichText::new(
                    "Select a subscene to move, turn or resize it.",
                )
                .small()
                .weak(),
            );
        }
        return;
    };

    // Name, click to edit.
    ui.horizontal(|ui| {
        let renaming = app.ui_state.renaming_subscene.as_ref().is_some_and(|(j, _)| *j == k);
        if renaming {
            let resp = {
                let (_, buf) = app.ui_state.renaming_subscene.as_mut().unwrap();
                ui.add(egui::TextEdit::singleline(buf).desired_width(160.0))
            };
            resp.request_focus();
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                app.ui_state.renaming_subscene = None;
            } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) || resp.lost_focus() {
                if let Some((j, name)) = app.ui_state.renaming_subscene.take() {
                    app.rename_subscene(j, &name);
                }
            }
        } else {
            let name = app.scene.layer_name(k).to_string();
            let resp = ui.add(egui::Label::new(egui::RichText::new(&name).strong()).sense(egui::Sense::click()));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Click to rename",
                "click: rename this subscene",
            );
            if resp.clicked() {
                app.ui_state.renaming_subscene = Some((k, name));
            }
        }
    });

    // Placement. Position and scale straight from the placement; the
    // rotation as cached XYZ degrees (see `UiState::subscene_euler`).
    let mut p = app.scene.subscenes[k - 1].placement;
    let generation = app.matrix_generation();
    let euler = match app.ui_state.subscene_euler {
        Some((j, g, e)) if j == k && g == generation => e,
        _ => {
            let e = p.rotation.to_xyz_degrees();
            app.ui_state.subscene_euler = Some((k, generation, e));
            e
        }
    };
    let mut t = p.translation.to_array();
    let mut e = euler;
    let mut scale = p.scale;
    let mut changed = false;
    let mut turned = false;

    egui::Grid::new("fracturize_subscene_place").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("at");
        for (axis, v) in ["x", "y", "z"].iter().zip(t.iter_mut()) {
            let resp = num::drag(ui, 7, 3, num::Drag::new(v).speed(0.005).prefix(format!("{} ", axis)));
            let resp = hinted(resp, &mut app.ui_state, "Where the subscene sits in the scene", "drag: move the subscene");
            changed |= resp.changed();
        }
        ui.end_row();
        ui.label("turn");
        for (axis, v) in ["x", "y", "z"].iter().zip(e.iter_mut()) {
            let resp = num::drag(ui, 7, 1, num::Drag::new(v).speed(0.5).suffix("°").prefix(format!("{} ", axis)));
            let resp = hinted(resp, &mut app.ui_state, "The subscene's rotation, in degrees about x, then y, then z", "drag: turn the subscene");
            if resp.changed() {
                changed = true;
                turned = true;
            }
        }
        ui.end_row();
        ui.label("scale");
        let resp = num::drag(ui, 7, 3, num::Drag::new(&mut scale).range(0.001..=1000.0).speed(0.005));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "The subscene's size. It scales evenly in every direction; to stretch it one way, scale its transforms instead",
            "drag: resize the subscene",
        );
        changed |= resp.changed();
        ui.end_row();
    });
    if changed {
        p.translation = glam::Vec3::from(t);
        p.scale = scale;
        if turned {
            p.rotation = crate::rot::Orientation::from_xyz_degrees(e);
        }
        app.set_subscene_placement(k, p);
        // Keep the degrees that were typed, under the generation the edit made.
        app.ui_state.subscene_euler = Some((k, app.matrix_generation(), e));
    }

    // Colour source: this subscene's own.
    ui.horizontal(|ui| {
        ui.label("colour");
        let mode = app.scene.subscenes[k - 1].colour.color_mode;
        let chosen = super::radio::radio(&mut app.ui_state, "subscene_color_mode", mode)
            .option(
                ColorMode::Transforms,
                "transforms",
                "Each of its transforms brings its own colour",
                "click: colour this subscene by its transforms",
            )
            .option(
                ColorMode::Palette,
                "palette",
                "Colour from its own gradient. To edit the gradient, edit the subscene's scene file",
                "click: colour this subscene through a gradient",
            )
            .option(
                ColorMode::Mix,
                "mix",
                "Its transforms' colours mix like paint as points pass through them",
                "click: mix this subscene's colours",
            )
            .show(ui);
        if let Some(m) = chosen {
            app.set_subscene_color_mode(k, m);
        }
    });

    // Where it came from.
    let sub = &app.scene.subscenes[k - 1];
    let from = match (&sub.file, &sub.base) {
        (Some(f), Some(_)) => format!("from {}, linked until you edit it", f),
        _ => match &sub.source {
            Some(src) => format!("a copy of {}, saved in this scene", src),
            None => "saved in this scene".to_string(),
        },
    };
    let by = sub.author.as_deref().map(|a| format!(" · by {}", a)).unwrap_or_default();
    ui.label(egui::RichText::new(format!("{}{}", from, by)).small().weak());
}
