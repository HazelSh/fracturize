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

    // Centred rather than tucked in a corner: this is a transient picker you
    // open, choose from and dismiss, and the top-left is where the Explore
    // and Render windows live.
    let screen = ctx.content_rect();
    let default_pos = egui::pos2((screen.center().x - 150.0).max(24.0), 80.0);

    egui::Window::new("Scenes")
        .id(egui::Id::new("fracturize_browser_window"))
        .open(&mut open)
        .default_pos(default_pos)
        .default_width(300.0)
        .default_height(420.0)
        .resizable(true)
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
                        // Keep the keyboard selection visible as Up/Down walks
                        // past the edge of the viewport.
                        if i == selected {
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

    // `browser_load_selected` closes the browser itself; don't let a stale
    // `open` from before the load reopen it.
    if !open {
        app.show_browser = false;
    }
}
