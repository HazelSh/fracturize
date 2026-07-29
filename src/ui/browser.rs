//! Scene browser window: the list of `.toml` scenes found under `scenes/`
//! (plus the current scene's own directory), click to load.
//!
//! B still opens and closes it and Up/Down/Enter still walk and load the
//! selection — this window renders the same `browser_files`/`browser_selected`
//! state those keys drive, so the two stay in lockstep, and the row the keys
//! are on scrolls itself into view.

use crate::app::App;

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.show_browser;
    if !open {
        return;
    }

    // Rows to draw, snapshotted before the loop so the bodies can take
    // `&mut App` (clicking one loads a whole new scene).
    let rows: Vec<(String, String)> = app
        .browser_files()
        .iter()
        .map(|p| {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string();
            (name, p.display().to_string())
        })
        .collect();
    let selected = app.browser_selected();
    let current = app.scene_path.clone();
    let scroll_to_selection = app.ui_state.browser_scrolled_to != Some(selected);
    app.ui_state.browser_scrolled_to = Some(selected);

    super::window(ctx, app, super::WindowKey::Scenes, "Scenes")
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("Up/Down to walk, Enter to load — or click a row.")
                    .small()
                    .weak(),
            );
            ui.separator();

            let mut load: Option<usize> = None;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, (name, path)) in rows.iter().enumerate() {
                        let is_current = current.as_deref() == Some(path.as_str());
                        let text = if is_current {
                            egui::RichText::new(name).strong()
                        } else {
                            egui::RichText::new(name)
                        };
                        let resp = ui.selectable_label(i == selected, text);
                        let resp = hinted(
                            resp,
                            &mut app.ui_state,
                            if is_current {
                                format!("{} (currently loaded)", path)
                            } else {
                                path.clone()
                            },
                            "click: load this scene",
                        );
                        // Follow the keyboard selection into view — but only
                        // on the frame it actually moves. Doing it every frame
                        // fights the scrollbar: any scroll that takes the
                        // selection off-screen snaps straight back, which
                        // makes the list unscrollable by hand.
                        if i == selected && scroll_to_selection {
                            resp.scroll_to_me(None);
                        }
                        if resp.clicked() {
                            load = Some(i);
                        }
                    }
                });

            if let Some(i) = load {
                app.set_browser_selected(i);
                app.browser_load_selected();
            }
        });

    super::remember(ctx, app, super::WindowKey::Scenes);

    // `browser_load_selected` closes the browser itself; don't let a stale
    // `open` from before the load reopen it.
    if !open {
        app.show_browser = false;
    }
}
