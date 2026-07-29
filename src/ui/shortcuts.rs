//! Keybinds window: the reference table that used to be a 50-line block-glyph
//! overlay painted through the legacy text shim, with hand-rolled row
//! hit-testing (`build_help_entries` / `try_click_help` in app.rs).
//!
//! It outgrew the window — 47 rows at a fixed line height overflowed a 720px
//! viewport with no way to reach the bottom — so it's a real `egui::Window`
//! now: scrollable, grouped into sections, with clickable rows that run the
//! binding (shift+click = the second-listed variant) and tooltips saying so.
//!
//! Pulled forward from Phase 6 of the GUI upgrade plan, which is where the
//! rest of the shim's callers get retired.

use crate::app::{App, HelpAction};

use super::hints::hinted;

/// One row: key label, description, and what clicking it does (`None` =
/// informational, e.g. mouse gestures that have no keyboard equivalent).
type Row = (&'static str, &'static str, Option<HelpAction>);

/// Every binding, grouped by the task it belongs to. Order within a section
/// is roughly "most reached-for first". Keep this in sync with the keyboard
/// dispatch in `main.rs::window_event` and the tables in AGENTS.md.
const SECTIONS: &[(&str, &[Row])] = &[
    (
        "Camera",
        &[
            ("drag", "orbit camera (pauses auto-orbit)", None),
            ("shift+drag", "pan focus (middle-drag works too)", None),
            ("scroll", "zoom", None),
            ("Up / Down", "zoom in / out", Some(HelpAction::Zoom)),
            ("O", "pause / resume camera orbit", Some(HelpAction::ToggleOrbit)),
            ("I", "invert mouse pitch (saved to prefs)", Some(HelpAction::InvertPitch)),
            ("V", "save current view to views/", Some(HelpAction::SaveView)),
        ],
    ),
    (
        "Camera paths",
        &[
            ("Y / Shift+Y", "add / remove camera path keypoint", Some(HelpAction::PathKey)),
            ("Z", "play / stop camera path flythrough", Some(HelpAction::PathPlay)),
            ("Ctrl+Y", "toggle camera path loop", None),
        ],
    ),
    (
        "Transforms",
        &[
            ("drag gizmo dot", "select + move in view plane", None),
            ("drag on-axis edge", "move along that axis", None),
            ("drag outer edge", "rotate around third axis", None),
            ("ctrl+drag gizmo", "uniform scale", None),
            ("scroll on gizmo", "adjust that transform's weight", None),
            ("A / Shift+A", "duplicate selected / add new transform", Some(HelpAction::AddTransform)),
            ("Del", "delete selected transform", Some(HelpAction::DeleteTransform)),
            ("Enter", "enable / disable selected transform", Some(HelpAction::ToggleSelected)),
            (". / ,", "selected weight up / down", Some(HelpAction::Weight)),
        ],
    ),
    (
        "Variations",
        &[
            ("E / Shift+E", "next / prev variation slot", Some(HelpAction::CycleVariation)),
            ("= / -", "variation weight up / down", Some(HelpAction::VariationWeight)),
        ],
    ),
    (
        "Color",
        &[
            ("J / Shift+J", "color hue up / down", Some(HelpAction::Hue)),
            ("K / Shift+K", "color saturation up / down", Some(HelpAction::Sat)),
            ("L / Shift+L", "color value up / down", Some(HelpAction::Val)),
            ("D / Shift+D", "finer / coarser color detail (falloff)", Some(HelpAction::ColorFalloff)),
            ("C / Shift+C", "less / more color contrast", Some(HelpAction::ColorContrast)),
        ],
    ),
    (
        "Renderer",
        &[
            ("R", "renderer: points / splat (log-density)", Some(HelpAction::RenderMode)),
            ("W / Shift+W", "splat exposure up / down", Some(HelpAction::Exposure)),
            ("] / [", "grow / shrink point size", Some(HelpAction::PointSize)),
            ("F / Shift+F", "more / less fog", Some(HelpAction::FogIntensity)),
            ("N / Shift+N", "fog start closer / farther", Some(HelpAction::FogNear)),
            ("M / Shift+M", "fog end closer / farther", Some(HelpAction::FogFar)),
            ("Space", "re-seed points (reset)", Some(HelpAction::Reset)),
        ],
    ),
    (
        "Explore",
        &[
            ("U / Shift+U", "random mutation / undo it", Some(HelpAction::Mutate)),
            ("Ctrl+Z / Ctrl+Shift+Z", "undo / redo any edit", Some(HelpAction::Undo)),
            ("X / Shift+X", "chaos traces: show+re-roll / hide", Some(HelpAction::Traces)),
        ],
    ),
    (
        "Files",
        &[
            ("B", "browse + load scenes", Some(HelpAction::Browse)),
            ("Ctrl+S", "save scene TOML", Some(HelpAction::SaveScene)),
            ("S", "save screenshot to screenshots/", Some(HelpAction::Screenshot)),
            ("P", "HQ render current view (background)", Some(HelpAction::HqRender)),
        ],
    ),
    (
        "Overlays",
        &[
            ("H / ?", "toggle this window", Some(HelpAction::ToggleHelp)),
            ("G", "toggle transform gizmos + labels", Some(HelpAction::ToggleGizmos)),
            ("Esc", "quit", None),
        ],
    ),
];

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.show_help;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Keybinds, "Keybinds")
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("Rows with a key binding are clickable — shift+click runs the second variant.")
                    .small()
                    .weak(),
            );
            ui.separator();

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let shift = ui.input(|i| i.modifiers.shift);
                let mut pending: Option<(HelpAction, bool)> = None;

                for (title, rows) in SECTIONS {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(*title).strong());
                    for (key, desc, action) in *rows {
                        if let Some(a) = draw_row(ui, app, key, desc, *action, shift) {
                            pending = Some((a, shift));
                        }
                    }
                    ui.add_space(4.0);
                }

                // Run the action after the loop: `run_help_action` can load a
                // scene or delete a transform, and doing that mid-iteration
                // would edit state the remaining rows still read.
                if let Some((action, shift)) = pending {
                    app.run_help_action(action, shift);
                }
            });
        });

    super::remember(ctx, app, super::WindowKey::Keybinds);

    // Only the window's own close button writes back. A row action can be
    // `ToggleHelp`, which already cleared `show_help` inside the closure —
    // assigning our stale `open` (still true) would undo it.
    if !open {
        app.show_help = false;
    }
}

/// One keybind row: fixed-width key column, description, whole row clickable
/// when it has an action. Returns the action if it was clicked this frame.
fn draw_row(
    ui: &mut egui::Ui,
    app: &mut App,
    key: &str,
    desc: &str,
    action: Option<HelpAction>,
    shift: bool,
) -> Option<HelpAction> {
    let clickable = action.is_some();
    let sense = if clickable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };

    let resp = ui
        .scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
            ui.set_width(ui.available_width());
            // Reserved now, sized after layout: `max_rect()` is all the
            // remaining space, not this row (see ui/transforms.rs::draw_row).
            let bg_idx = ui.painter().add(egui::Shape::Noop);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.add_sized(
                    egui::vec2(140.0, ui.spacing().interact_size.y),
                    egui::Label::new(
                        egui::RichText::new(key)
                            .monospace()
                            .color(if clickable {
                                ui.visuals().strong_text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            }),
                    )
                    .selectable(false),
                );
                ui.label(egui::RichText::new(desc).color(ui.visuals().text_color()));
            });

            if clickable && ui.response().hovered() {
                let rect = ui.min_rect();
                ui.painter().set(
                    bg_idx,
                    egui::Shape::rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill),
                );
            }
        })
        .response;

    if !clickable {
        return None;
    }

    let resp = hinted(
        resp,
        &mut app.ui_state,
        if shift {
            "Click to run the shift variant of this binding"
        } else {
            "Click to run this binding · shift+click for the second variant"
        },
        "click: run this binding · shift+click: second variant",
    );

    resp.clicked().then_some(action).flatten()
}
