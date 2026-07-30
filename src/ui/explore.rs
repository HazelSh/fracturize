//! Explore window: mutate/strength slider + the unified history list.
//! Undo/Redo buttons and the history list all go through the same
//! `App::undo`/`redo`/`jump_undo`/`jump_redo` choke points as the
//! Ctrl+Z/Ctrl+Shift+Z/Shift+U keybinds (src/history.rs) — clicking a row N
//! deep undoes/redoes N steps in one rebuild.

use crate::app::App;

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.explore_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Explore, "Explore")
        .open(&mut open)
        .show(ctx, |ui| {
            // The two ways to start from nothing, side by side: roll one, or
            // build one. Both are history entries, so either is one Ctrl+Z away
            // from whatever was on screen.
            ui.horizontal(|ui| {
                let resp = ui.button(format!("{}  Random flame", super::icons::DICE));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Roll a whole new flame from nothing. Quality-checked on the CPU first, so it always renders — and it's one Ctrl+Z away from what you have now.",
                    "click: generate a random flame",
                );
                if resp.clicked() {
                    app.random_flame();
                }

                let resp = ui.button(format!("{}  Blank scene", super::icons::FILE));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Start from an empty canvas: two plain half-scale transforms and nothing else, to build an IFS up by hand. One Ctrl+Z away from what you have now.",
                    "click: start a blank scene",
                );
                if resp.clicked() {
                    app.new_blank_scene();
                }
            });
            ui.separator();

            let mut strength = app.mutate_strength();
            let resp = ui.add(
                egui::Slider::new(&mut strength, 0.1..=3.0)
                    .text("strength")
                    .clamping(egui::SliderClamping::Always),
            );
            if resp.changed() {
                app.set_mutate_strength(strength);
            }
            // Persist once the drag ends rather than on every changed-frame,
            // so a single drag writes prefs.toml once, not per pixel.
            if resp.drag_stopped() {
                app.save_prefs();
            }
            hinted(
                resp,
                &mut app.ui_state,
                "How strongly U perturbs the scene (persisted)",
                "drag: change mutation strength",
            );

            ui.horizontal(|ui| {
                let resp = ui.button("Mutate");
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Apply a random mutation (U)",
                    "click: mutate the scene",
                );
                if resp.clicked() {
                    app.mutate_scene();
                }

                let resp = ui.add_enabled(app.history.undo_len() > 0, egui::Button::new("Undo"));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Undo the last edit (Ctrl+Z, Shift+U)",
                    "click: undo the last edit",
                );
                if resp.clicked() {
                    app.undo();
                }

                let resp = ui.add_enabled(app.history.redo_len() > 0, egui::Button::new("Redo"));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Redo the last undone edit (Ctrl+Shift+Z)",
                    "click: redo the last undone edit",
                );
                if resp.clicked() {
                    app.redo();
                }
            });

            ui.separator();
            ui.label(egui::RichText::new("History").strong());

            let redo_labels: Vec<String> = app.history.redo_display().map(str::to_string).collect();
            let undo_labels: Vec<String> = app.history.undo_display().map(str::to_string).collect();
            let redo_len = redo_labels.len();

            if redo_labels.is_empty() && undo_labels.is_empty() {
                ui.label(egui::RichText::new("No edits yet.").small().weak());
            } else {
                let mut jump_redo: Option<usize> = None;
                let mut jump_undo: Option<usize> = None;

                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        // Redo entries, grayed, furthest first — the one
                        // closest to "current" (last in the vec) sits right
                        // above the marker.
                        for (i, label) in redo_labels.iter().enumerate() {
                            let resp = ui.add(
                                egui::Label::new(egui::RichText::new(format!("↷ {}", label)).weak())
                                    .sense(egui::Sense::click()),
                            );
                            let resp = hinted(
                                resp,
                                &mut app.ui_state,
                                format!("Redo forward to after \"{}\"", label),
                                "click: redo to this point",
                            );
                            if resp.clicked() {
                                jump_redo = Some(redo_len - i);
                            }
                        }

                        ui.label(
                            egui::RichText::new("— current —").small().italics().weak(),
                        );

                        // Undo entries, newest-first: the top row is what
                        // Ctrl+Z undoes next.
                        for (j, label) in undo_labels.iter().enumerate() {
                            let resp = ui.add(
                                egui::Label::new(format!("↶ {}", label)).sense(egui::Sense::click()),
                            );
                            let resp = hinted(
                                resp,
                                &mut app.ui_state,
                                format!("Undo back to before \"{}\"", label),
                                "click: undo to this point",
                            );
                            if resp.clicked() {
                                jump_undo = Some(j + 1);
                            }
                        }
                    });

                // At most one of these fires per frame (a single click), and
                // they're mutually exclusive sections of the same list.
                if let Some(n) = jump_redo {
                    app.jump_redo(n);
                }
                if let Some(n) = jump_undo {
                    app.jump_undo(n);
                }
            }
        });

    super::remember(ctx, app, super::WindowKey::Explore);
    app.ui_state.panels.explore_open = open;
}
