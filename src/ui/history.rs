//! History window: every edit, newest first, clickable to jump to. Rows go
//! through the same `App::jump_undo`/`jump_redo` choke points as Ctrl+Z and
//! Ctrl+Shift+Z (src/history.rs) — clicking a row N deep undoes or redoes N
//! steps in one rebuild. Undo and Redo themselves are in the Explore menu and
//! on the toolbar.

use crate::app::App;

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.history_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::History, "History")
        .open(&mut open)
        .show(ctx, |ui| {
            let redo_labels: Vec<String> = app.history.redo_display().map(str::to_string).collect();
            let undo_labels: Vec<String> = app.history.undo_display().map(str::to_string).collect();
            let redo_len = redo_labels.len();

            if redo_labels.is_empty() && undo_labels.is_empty() {
                ui.label(egui::RichText::new("No edits yet.").small().weak());
            } else {
                let mut jump_redo: Option<usize> = None;
                let mut jump_undo: Option<usize> = None;

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Redo entries, grayed, furthest first — the one
                        // closest to "current" (last in the vec) sits right
                        // above the marker.
                        for (i, label) in redo_labels.iter().enumerate() {
                            let resp = ui.add(
                                egui::Label::new(egui::RichText::new(format!("↷ {}", label)).weak())
                                    .sense(egui::Sense::click()),
                            );
                            // These rows are drawn, and things you can see and
                            // click read as durable — but committing any new
                            // edit clears the redo stack, so nudging one slider
                            // can make twenty visible rows vanish. Universal
                            // behaviour, worth a word here because this app is
                            // unusual in *showing* them.
                            let resp = hinted(
                                resp,
                                &mut app.ui_state,
                                format!(
                                    "Redo up to and including “{}”\n\nThese greyed rows \
                                     disappear as soon as you make a new edit.",
                                    label
                                ),
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
                                format!("Undo back to before “{}”", label),
                                "click: undo to this point",
                            );
                            if resp.clicked() {
                                jump_undo = Some(j + 1);
                            }
                        }

                        // Eviction used to be silent, which is the worst way
                        // for it to happen: on an L-system scene a single
                        // whole-scene snapshot is megabytes, so the byte cap
                        // binds long before the 64-entry one and the bottom of
                        // this list quietly stops being the beginning.
                        let dropped = app.history.dropped();
                        if dropped > 0 {
                            let resp = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "… {} older edit{} dropped",
                                        dropped,
                                        if dropped == 1 { "" } else { "s" },
                                    ))
                                    .small()
                                    .weak(),
                                )
                                .sense(egui::Sense::hover()),
                            );
                            hinted(
                                resp,
                                &mut app.ui_state,
                                "The history has a size limit. Scenes with many transforms \
                                 use more memory per step, so they keep fewer steps (always \
                                 at least ten).",
                                "older edits were dropped to save memory",
                            );
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

    super::remember(ctx, app, super::WindowKey::History);
    app.ui_state.panels.history_open = open;
}
