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

use super::camera_panel::draw_input_prefs;
use super::hints::hinted;

/// One row: key label, description, and what clicking it does (`None` =
/// informational, e.g. mouse gestures that have no keyboard equivalent).
type Row = (&'static str, &'static str, Option<HelpAction>);

use crate::version::BUILD;

/// Side of the reserved logo square, in points.
const LOGO_SIDE: f32 = 44.0;

/// The logo we don't have yet: a faint outlined square holding its place.
/// Outlined rather than left blank because an empty gap in a header reads as a
/// layout bug. When there's an asset, replace the paint with an `egui::Image`
/// of the same size and the rest of the row keeps its geometry.
fn logo_slot(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(LOGO_SIDE, LOGO_SIDE), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.painter().rect_stroke(
        rect,
        6.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
}

/// Name, version, repo, and the two things a stranger most needs warning
/// about. This is the window someone opens first, so the "what am I even
/// running" answer lives at the top of it.
fn draw_about(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        logo_slot(ui);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Fracturize").heading());
                ui.label(egui::RichText::new(BUILD).small().weak());
            });
            ui.label(egui::RichText::new("3D IFS fractal renderer").small().weak());
            // `CARGO_PKG_REPOSITORY` rather than a literal: the URL is
            // metadata about the crate, and Cargo.toml is where the rest of it
            // lives. egui-winit's link handling opens this in a real browser.
            let resp = ui.hyperlink_to(
                egui::RichText::new(env!("CARGO_PKG_REPOSITORY")).small(),
                env!("CARGO_PKG_REPOSITORY"),
            );
            hinted(
                resp,
                &mut app.ui_state,
                "Source code, scenes and issues. Opens in your browser.",
                "click: open the repository in a browser",
            );
        });
    });

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Pre-release: the scene format, the GUI and CLI, the renderer and the \
             math itself can all change without notice, so scenes and old renders \
             may not survive an update. Roughly 95% vibecoded, mostly by Claudes, \
             with minimal human involvement in the code.",
        )
        .small()
        .weak(),
    );
}

/// Every binding, grouped by the task it belongs to. Order within a section
/// is roughly "most reached-for first". Keep this in sync with the keyboard
/// dispatch in `main.rs::window_event` and the tables in AGENTS.md.
const SECTIONS: &[(&str, &[Row])] = &[
    (
        "Camera",
        &[
            ("drag", "orbit the camera (stops it flying its path)", None),
            ("shift+drag", "pan (middle-drag works too)", None),
            ("right-drag", "roll the camera", None),
            ("right-click gizmo", "that transform's menu", None),
            ("scroll", "zoom", None),
            ("Up / Down", "zoom in / out (or step through transforms, if one is selected)", Some(HelpAction::Zoom)),
            ("Home", "centre the camera on the selected transform", Some(HelpAction::FrameSelected)),
            ("axis cross", "click a ball (bottom right) to look down that axis", None),
            ("T / Shift+T", "one view, four views, or one big and three small", Some(HelpAction::ViewLayout)),
            ("drag in an axis view", "pan (an axis view looks one way only)", None),
            ("O / Z / Space", "start / stop the camera flying its path", Some(HelpAction::CameraMotion)),
            ("I", "invert mouse pitch", Some(HelpAction::InvertPitch)),
            ("V", "save the current view", Some(HelpAction::SaveView)),
        ],
    ),
    (
        "Camera paths",
        &[
            ("Y / Shift+Y", "add a keypoint here / remove the last one", Some(HelpAction::PathKey)),
            ("", "set how the path loops in the Camera window", None),
            ("", "with no keypoints, the path is one full orbit", None),
        ],
    ),
    (
        "Timeline",
        &[
            ("Left / Right", "a frame back / on (Shift: the key before / after)", Some(HelpAction::StepFrame)),
            ("ctrl+scroll on lanes", "zoom the timeline in or out", None),
            ("shift+scroll, middle-drag", "move along the timeline", None),
            ("right-click a lane", "key here; on a key, copy / paste / duplicate / delete", None),
            ("ctrl while dragging a key", "snap the other way from the magnet", None),
            ("", "record (toolbar, the red ring): every edit keys the playhead", None),
        ],
    ),
    (
        "Transforms",
        &[
            ("click gizmo dot", "select that transform", None),
            ("drag its dot", "move it across the screen", None),
            ("drag an axis", "move along that axis", None),
            ("drag outer edge", "rotate", None),
            ("ctrl+drag dot or axis", "scale evenly", None),
            ("hold shift while dragging", "fine adjustment", None),
            ("ctrl+drag outer edge", "rotate in 15° steps", None),
            ("alt+scroll on gizmo", "change that transform's weight", None),
            ("A / Shift+A", "duplicate selected / add a new transform", Some(HelpAction::AddTransform)),
            ("Del", "delete selected transform", Some(HelpAction::DeleteTransform)),
            ("Enter", "enable / disable selected transform", Some(HelpAction::ToggleSelected)),
            (". / ,", "selected transform's weight up / down", Some(HelpAction::Weight)),
        ],
    ),
    (
        "Variations",
        &[
            ("E / Shift+E", "choose the next / previous variation", Some(HelpAction::CycleVariation)),
            ("= / -", "that variation's weight up / down", Some(HelpAction::VariationWeight)),
        ],
    ),
    (
        "Colour",
        &[
            ("J / Shift+J", "hue up / down", Some(HelpAction::Hue)),
            ("K / Shift+K", "saturation up / down", Some(HelpAction::Sat)),
            ("L / Shift+L", "brightness up / down", Some(HelpAction::Val)),
            ("D / Shift+D", "finer / coarser colour detail", Some(HelpAction::ColorFalloff)),
            ("C / Shift+C", "less / more colour contrast", Some(HelpAction::ColorContrast)),
        ],
    ),
    (
        "Renderer",
        &[
            ("R", "switch renderer: points / splat / volume", Some(HelpAction::RenderMode)),
            ("W / Shift+W", "splat exposure up / down", Some(HelpAction::Exposure)),
            ("] / [", "grow / shrink point size", Some(HelpAction::PointSize)),
            ("F / Shift+F", "more / less haze", Some(HelpAction::HazeIntensity)),
            // No key bound any more — Space now plays/stops the camera path
            // (Camera section, above). Left clickable here: re-seeding points
            // is rarely needed now that the renderers are more stable, but
            // still occasionally useful, and this row is its only way in.
            ("(click)", "start the points over from scratch", Some(HelpAction::Reset)),
        ],
    ),
    (
        "Explore",
        &[
            ("U / Shift+U", "random mutation / undo", Some(HelpAction::Mutate)),
            ("Ctrl+U", "mutation grid in the view / back to one", Some(HelpAction::MutationGrid)),
            ("Ctrl+Z / Ctrl+Shift+Z", "undo / redo any edit", Some(HelpAction::Undo)),
            ("Ctrl+Y", "redo", None),
            ("X / Shift+X", "show new traces / hide them", Some(HelpAction::Traces)),
        ],
    ),
    (
        "Files",
        &[
            ("Ctrl+O / B", "open a scene", Some(HelpAction::Browse)),
            ("drop a file", "open a scene, or the scene inside a render", None),
            ("Ctrl+N", "new blank scene (undoable)", Some(HelpAction::NewScene)),
            ("Ctrl+S", "save the scene", Some(HelpAction::SaveScene)),
            ("Ctrl+Shift+S", "save a copy under a new name", None),
            ("S", "save a screenshot", Some(HelpAction::Screenshot)),
            ("P", "render a still or an animation", None),
            ("Ctrl+Q", "quit (asks first if there are unsaved edits)", Some(HelpAction::Quit)),
        ],
    ),
    (
        "Overlays",
        &[
            ("H / ? / F1", "toggle this window", Some(HelpAction::ToggleHelp)),
            ("G / Tab", "switch between edit and view mode", Some(HelpAction::ToggleGizmos)),
            ("M", "light up what the selected transform moves", Some(HelpAction::ToggleEmphasis)),
            ("Esc", "close a menu or dialog, or deselect", None),
        ],
    ),
];

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.show_help;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Keybinds, "Controls")
        .open(&mut open)
        .show(ctx, |ui| {
            draw_about(ui, app);
            ui.separator();

            // These three are settings, and they now look like enough of a
            // settings dialog to want saying so. Left here rather than given a
            // window of their own: three controls don't fill one, and this is
            // the window a stranger opens first.
            ui.label(egui::RichText::new("Settings").strong().small());
            draw_input_prefs(ui, app);

            ui.horizontal(|ui| {
                // Applied on *release*, not while dragging.
                //
                // This is a slider that sets how big sliders are, which makes
                // live application a feedback loop: every frame it re-scales
                // the widget you are holding, so the handle moves out from
                // under the pointer, which moves the handle again. It flickers,
                // it jumps, and it is close to unusable. Dragging shows the
                // pending number and the panel only re-lays-out once you let
                // go — the one control in the app where live application is
                // actively wrong.
                let mut scale = app.ui_state.pending_ui_scale.unwrap_or(app.ui_scale());
                let resp = ui.add(
                    egui::Slider::new(&mut scale, 0.7..=2.0)
                        .fixed_decimals(2)
                        .text("panel scale"),
                );
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "How big the panels and text are. Takes effect when you let \
                     go. The picture itself isn't affected.",
                    "drag: resize the panels (applies on release)",
                );
                if resp.dragged() {
                    app.ui_state.pending_ui_scale = Some(scale);
                } else if resp.changed() {
                    // A click on the track, or an arrow key: no drag to wait for.
                    app.ui_state.pending_ui_scale = None;
                    app.set_ui_scale(scale);
                }
                if resp.drag_stopped() {
                    app.ui_state.pending_ui_scale = None;
                    app.set_ui_scale(scale);
                }
            });

            ui.separator();

            ui.label(
                egui::RichText::new("Click a row to use it. Shift+click does the second action, where there is one.")
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
            "Click to do the second action"
        } else {
            "Click to do this. Shift+click does the second action, where there is one"
        },
        "click: do this · shift+click: the second action",
    );

    resp.clicked().then_some(action).flatten()
}
