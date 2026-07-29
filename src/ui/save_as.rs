//! "Save scene as…" — a small modal for naming a fork of the current scene.
//!
//! Deliberately not a native file dialog. Pulling in `rfd` would add a
//! dependency and a platform surface for one text field, and the app already
//! browses `scenes/` itself (`ui/browser.rs`), so the thing a native picker
//! would be *for* — seeing what's already there — is a keypress away.
//!
//! The one job worth doing carefully is not clobbering an existing scene.
//! Undo only covers the scene you have open, so overwriting a different one is
//! not recoverable from in-app: the dialog checks the path on every keystroke
//! and makes confirming an overwrite a separate, deliberate act.

use crate::app::App;

use super::hints::hinted;

/// Open state and in-progress filename. Lives on `UiState` so the text
/// survives the frame loop; `None` means the dialog is closed.
#[derive(Default)]
pub struct SaveAsState {
    pub path: Option<String>,
    /// Set once the user has acknowledged that the target exists.
    pub allow_overwrite: bool,
    /// Last failure, shown under the field until the path changes.
    pub error: Option<String>,
    /// Set on open so the text field takes focus on its first frame.
    pub focus_field: bool,
}

impl SaveAsState {
    pub fn is_open(&self) -> bool {
        self.path.is_some()
    }
}

/// Open the dialog, prefilled with a sensible fork name.
pub fn open(app: &mut App) {
    let suggested = app.suggested_fork_path();
    app.ui_state.save_as = SaveAsState {
        path: Some(suggested),
        allow_overwrite: false,
        error: None,
        focus_field: true,
    };
}

pub fn draw(ctx: &egui::Context, app: &mut App) {
    if app.ui_state.save_as.path.is_none() {
        return;
    }

    let mut open = true;
    let mut close = false;
    egui::Window::new("Save scene as")
        .id(egui::Id::new("fracturize_save_as"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
        .show(ctx, |ui| {
            let mut path = app.ui_state.save_as.path.clone().unwrap_or_default();
            let field_id = egui::Id::new("fracturize_save_as_field");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut path)
                    .id(field_id)
                    .desired_width(f32::INFINITY)
                    .hint_text("scenes/name.toml"),
            );
            // The dialog exists to take one string; put the caret in it so
            // typing works the instant it opens.
            if app.ui_state.save_as.focus_field {
                app.ui_state.save_as.focus_field = false;
                resp.request_focus();
            }
            let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Where to write the scene. A relative path is relative to where fracturize \
                 was launched; `scenes/` is where the browser (B) looks.",
                "type: the new scene's filename",
            );
            if resp.changed() {
                // Any edit re-arms the overwrite guard: an acknowledgement of
                // one path must not carry over to a different one.
                app.ui_state.save_as.allow_overwrite = false;
                app.ui_state.save_as.error = None;
            }
            app.ui_state.save_as.path = Some(path.clone());

            let trimmed = path.trim();
            let exists = !trimmed.is_empty() && std::path::Path::new(trimmed).exists();
            let valid = !trimmed.is_empty();

            if exists {
                ui.label(
                    egui::RichText::new(format!("{} already exists.", trimmed))
                        .color(ui.visuals().warn_fg_color)
                        .small(),
                );
                let mut ack = app.ui_state.save_as.allow_overwrite;
                let resp = ui.checkbox(&mut ack, "overwrite it");
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Undo only covers the scene you have open — overwriting a different one \
                     can't be taken back from in here.",
                    "click: confirm overwriting that file",
                );
                if resp.changed() {
                    app.ui_state.save_as.allow_overwrite = ack;
                }
            } else if !trimmed.ends_with(".toml") && valid {
                ui.label(
                    egui::RichText::new("Scene files are .toml; the browser won't list this.")
                        .small()
                        .weak(),
                );
            }

            if let Some(err) = &app.ui_state.save_as.error {
                ui.label(
                    egui::RichText::new(err)
                        .color(ui.visuals().error_fg_color)
                        .small(),
                );
            }

            ui.separator();
            ui.horizontal(|ui| {
                let can_save = valid && (!exists || app.ui_state.save_as.allow_overwrite);
                let resp = ui.add_enabled(can_save, egui::Button::new("Save"));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    if can_save {
                        "Write the scene here and keep working on this copy"
                    } else if exists {
                        "Confirm the overwrite first"
                    } else {
                        "Enter a filename"
                    },
                    "click: save the scene under this name",
                );
                // Enter in the field submits, the same as clicking Save —
                // but only when Save would have been available, so hitting it
                // over an existing path can't sidestep the overwrite guard.
                if (resp.clicked() || submitted) && can_save {
                    let overwrite = app.ui_state.save_as.allow_overwrite;
                    let target = trimmed.to_string();
                    match app.save_scene_as(&target, overwrite) {
                        Ok(()) => close = true,
                        Err(e) => app.ui_state.save_as.error = Some(e),
                    }
                } else if submitted {
                    // Enter over a name that needs confirming: put the caret
                    // back rather than silently doing nothing.
                    app.ui_state.save_as.focus_field = true;
                }

                let resp = ui.button("Cancel");
                let resp = hinted(resp, &mut app.ui_state, "Leave the scene where it is", "click: cancel");
                if resp.clicked() {
                    close = true;
                }
            });
        });

    if close || !open {
        app.ui_state.save_as = SaveAsState::default();
    }
}
