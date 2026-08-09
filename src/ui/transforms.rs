//! Transforms window: per-transform list (color swatch, eye toggle, weight,
//! click-to-select, right-click context menu) plus a selected-transform
//! inspector implementing the plan's "Inspector: Mat4 <-> TRS" design —
//! position/rotation/scale fields when the matrix decomposes faithfully, a
//! raw matrix grid + "Orthogonalize -> TRS" fallback when it doesn't (shear
//! or a mirrored/det<0 matrix), plus weight/color/variation editing.
//!
//! Every mutation funnels through a matching `App` method (`set_transform_*`,
//! `rename_transform`, `duplicate_transform_at`, ...) that snapshots, edits,
//! syncs to the GPU, and commits through the unified history — this module
//! itself never touches `app.scene` mutably, only reads it for display.

use glam::{Mat4, Vec3, Vec4};

use crate::app::App;
use crate::scene::{NUM_VARIATIONS, VARIATION_NAMES};

use super::hints::hinted;
use super::icons;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.transforms_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Transforms, "Transforms")
        .open(&mut open)
        .show(ctx, |ui| {
            // Master-detail as a vertical tab rail, not a list stacked above
            // fields. Stacking leaves the relationship between the two halves
            // carried by proximity alone — nothing says *these* fields belong
            // to *that* row. A tab whose fill continues into the pane, with no
            // border between them, says it structurally.
            // Reserve the rail's backdrop before the columns are laid out so
            // it paints under them, then size it to the *full* height of the
            // row once both are placed. A rail that stops where its tabs stop
            // breaks the illusion — the strip has to run the height of the
            // pane for the active tab to read as continuous with it.
            let rail_bg_idx = ui.painter().add(egui::Shape::Noop);
            let row = ui.horizontal_top(|ui| {
                draw_rail(ui, app);
                draw_detail(ui, app);
            });
            let full = row.response.rect;
            ui.painter().set(
                rail_bg_idx,
                egui::Shape::rect_filled(
                    egui::Rect::from_min_size(full.min, egui::vec2(RAIL_WIDTH, full.height())),
                    0.0,
                    rail_fill(ui),
                ),
            );
        });

    super::remember(ctx, app, super::WindowKey::Transforms);
    app.ui_state.panels.transforms_open = open;
}

/// Width of the tab rail in points. Narrow on purpose: it carries identity
/// (colour, name, enabled) and relative weight, and nothing else.
const RAIL_WIDTH: f32 = 132.0;
/// Height of one tab, including its weight bar and the gap below it.
///
/// **This is the row pitch the rail's virtualization is told about**, so a tab
/// has to actually occupy it — see `draw_tab`'s `set_min_height`. It didn't,
/// once: the tabs laid out at their content height (~21pt, one `interact_size`
/// row plus spacing) while `show_rows` reserved 30, and every consequence of
/// that mismatch read as a different bug. egui drew only `viewport / 30` tabs
/// and stopped, so nine of a twenty-transform scene's rows were unreachable;
/// the drawn tabs filled 11/30ths less than the viewport, leaving a band of
/// dead rail above the buttons that followed; and the scrollbar sized itself
/// to a content height half again as tall as the content. One number, three
/// symptoms, none of which looks like the same bug from the outside.
const TAB_HEIGHT: f32 = 30.0;
/// The gap between painted tabs: a tab's body is `TAB_HEIGHT - TAB_GAP`, and
/// the rest is the space that keeps two selected-coloured fills apart.
const TAB_GAP: f32 = 4.0;
/// Thickness of the weight bar drawn along a tab's bottom edge.
const BAR_THICKNESS: f32 = 2.0;
/// How far the weight bar is inset from each end of its tab.
const BAR_INSET: f32 = 8.0;
/// Height of the weight bar's *grab* strip, centred on the drawn bar.
///
/// Deliberately much smaller than the tab: this strip is registered after the
/// tab and so takes the pointer from it, and it used to be 8pt of a 21pt row —
/// 40% of every tab, including the whole lower half of the name, was silently
/// not the click target it looked like. Centred on the 2pt bar it is now 20%
/// of a 30pt row, sitting under the thing it adjusts and nothing else.
const BAR_GRAB: f32 = 6.0;
/// Shortest the rail's list is allowed to get when the window is dragged small.
/// Below about three rows a list stops being one.
const MIN_LIST_HEIGHT: f32 = TAB_HEIGHT * 3.0;

/// The rail's backdrop: recessed relative to the detail pane, so the active
/// tab (which is filled with the *pane's* colour) reads as lifted out of it.
fn rail_fill(ui: &egui::Ui) -> egui::Color32 {
    let base = ui.visuals().window_fill;
    egui::Color32::from_rgb(
        (base.r() as f32 * 0.72) as u8,
        (base.g() as f32 * 0.72) as u8,
        (base.b() as f32 * 0.72) as u8,
    )
}

// === Tab rail ===

/// Snapshot of one tab's display data, taken before the loop so the loop body
/// is free to call `&mut App` methods without fighting the borrow checker
/// over `app.scene`.
struct RowData {
    name: String,
    color: Vec3,
    weight: f32,
    enabled: bool,
    /// Hover summary (position/scale/variations) — the info the retired HUD
    /// transform list used to show per row.
    summary: String,
}

fn row_data(app: &App, i: usize) -> RowData {
    let spec = &app.scene.transforms[i];
    let p = spec.matrix.w_axis.truncate();
    RowData {
        name: app
            .scene
            .transform_names
            .get(i)
            .and_then(|n| n.clone())
            .unwrap_or_default(),
        color: app.scene.colors.get(i).copied().unwrap_or(Vec3::ONE),
        weight: spec.weight,
        enabled: app.is_transform_enabled(i),
        summary: format!(
            "p=({:.2},{:.2},{:.2}) s={:.2} [{}]\nclick: select · right-click: menu",
            p.x,
            p.y,
            p.z,
            spec.matrix.x_axis.truncate().length(),
            spec.variation_summary(),
        ),
    }
}

/// Which transforms the rail should show, given the filter box.
///
/// Matches the name *or* the `T<i>` index label, case-insensitively, so both
/// "spine" and "12" find something. An empty filter is every transform, and
/// costs one `is_empty` check — the fast path for the overwhelmingly common
/// case of a scene with four maps.
fn filtered_indices(app: &App) -> Vec<usize> {
    let n = app.scene.transforms.len();
    let needle = app.ui_state.transform_filter.trim().to_lowercase();
    if needle.is_empty() {
        return (0..n).collect();
    }
    (0..n)
        .filter(|&i| {
            let name = app
                .scene
                .transform_names
                .get(i)
                .and_then(|n| n.as_deref())
                .unwrap_or("");
            name.to_lowercase().contains(&needle) || format!("t{}", i).contains(&needle)
        })
        .collect()
}

fn draw_filter(ui: &mut egui::Ui, app: &mut App) {
    // Only worth the row once there are enough tabs that finding one is a
    // problem. Below that it is a control that solves nothing, taking space
    // from the list it would filter.
    if app.scene.transforms.len() < FILTER_THRESHOLD {
        app.ui_state.transform_filter.clear();
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        let mut text = app.ui_state.transform_filter.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(RAIL_WIDTH - 34.0)
                .hint_text("filter"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Show only transforms whose name or T-number contains this",
            "type: filter the transform list",
        );
        if resp.changed() {
            app.ui_state.transform_filter = text;
        }
        if !app.ui_state.transform_filter.is_empty() {
            let resp = ui.add(egui::Button::new(icons::X).small().frame(false));
            let resp = hinted(resp, &mut app.ui_state, "Clear the filter", "click: clear the filter");
            if resp.clicked() {
                app.ui_state.transform_filter.clear();
            }
        }
    });
}

/// How many transforms it takes before the rail grows a filter box.
const FILTER_THRESHOLD: usize = 12;

fn draw_rail(ui: &mut egui::Ui, app: &mut App) {
    let selected = app.selected_transform();
    // The rail is virtualized because L-system scenes reach tens of thousands
    // of transforms — but scrolling ten thousand tabs to find one is not a
    // workflow, and virtualization only pays off once there's a way to *not*
    // scroll. One line of filter is that way.
    let visible = filtered_indices(app);
    let n = visible.len();
    // Weights are shown as bars relative to the largest, so "which transform
    // dominates the chaos game" stays readable at a glance even though the
    // editable field lives over in the detail pane.
    let max_weight = app
        .scene
        .transforms
        .iter()
        .map(|t| t.weight)
        .fold(0.01f32, f32::max);

    ui.vertical(|ui| {
        ui.set_width(RAIL_WIDTH);
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(0, 4))
            .show(ui, |ui| {
                ui.set_width(RAIL_WIDTH);
                // Above the list, not below it. These are the list's own
                // operations, so they belong at one of its ends — and the end
                // that doesn't move is the top. Under the list they sat
                // wherever the list happened to stop, which on a scene with
                // more transforms than fit was nowhere near it.
                draw_list_actions(ui, app, selected);
                draw_filter(ui, app);
                // The list takes the rest of the window. It used to take 320pt
                // whatever the window was, so growing the window grew a band of
                // empty rail instead of showing more transforms.
                let height = ui.available_height().max(MIN_LIST_HEIGHT);
                // Virtualized: L-system scenes reach tens of thousands of
                // transforms, and laying every tab out per frame would cost
                // more than the whole rest of the UI.
                egui::ScrollArea::vertical()
                    .id_salt("fracturize_transform_rail")
                    .max_height(height)
                    // Both axes: the list is the rail's body and should hold
                    // its height whether it has four transforms or four
                    // thousand, so the pane beside it doesn't change size when
                    // the filter box matches fewer rows.
                    .auto_shrink([false, false])
                    .show_rows(ui, TAB_HEIGHT, n, |ui, range| {
                        if n == 0 {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("Nothing matches that filter.")
                                    .small()
                                    .weak(),
                            );
                        }
                        for slot in range {
                            let i = visible[slot];
                            let row = row_data(app, i);
                            draw_tab(ui, app, i, row, selected == Some(i), max_weight);
                        }
                    });
            });
    });
}

/// Add / duplicate / delete: the operations that act on the *list*, at the top
/// of the rail that shows it.
///
/// Duplicate and delete read as list operations even though they need a
/// selection — "one more like that one", "that one goes" — which is why they
/// sit here rather than beside Disable and Rename in the detail pane, where
/// the controls describe the selected transform itself. Both are shown
/// disabled rather than hidden when there's no selection, per the house rule.
fn draw_list_actions(ui: &mut egui::Ui, app: &mut App, selected: Option<usize>) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        let resp = ui.add(egui::Button::new(format!("{} Add", icons::PLUS)).small());
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Add a fresh transform (Shift+A)",
            "click: add a new transform",
        );
        if resp.clicked() {
            app.add_transform(true);
        }

        let resp = ui.add_enabled(
            selected.is_some(),
            egui::Button::new(format!("{} Dup", icons::COPY)).small(),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Duplicate the selected transform (A)",
            "click: duplicate selected transform",
        );
        if resp.clicked() {
            if let Some(i) = selected {
                app.duplicate_transform_at(i);
            }
        }

        // The chaos game needs somewhere to send the point, so the last
        // transform can't go — same rule as the context menu's Delete.
        let can_delete = selected.is_some() && app.scene.transforms.len() > 1;
        let resp = ui.add_enabled(can_delete, egui::Button::new(icons::TRASH).small());
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if selected.is_none() {
                "Delete a transform (Del) — select one first"
            } else if app.scene.transforms.len() > 1 {
                "Delete the selected transform (Del)"
            } else {
                "A scene needs at least one transform"
            },
            "click: delete selected transform",
        );
        if resp.clicked() {
            if let Some(i) = selected {
                app.delete_transform_at(i);
            }
        }
    });
}

/// One tab. The selected tab is filled with the *detail pane's* colour and
/// extends past the rail's right edge, so it reads as continuous with the
/// pane rather than as a highlighted row in a separate list.
fn draw_tab(
    ui: &mut egui::Ui,
    app: &mut App,
    i: usize,
    row: RowData,
    selected: bool,
    max_weight: f32,
) {
    let is_renaming = app
        .ui_state
        .renaming_transform
        .as_ref()
        .is_some_and(|(idx, _)| *idx == i);

    let display_name = if row.name.is_empty() {
        format!("T{}", i)
    } else {
        row.name.clone()
    };

    let sense = if is_renaming {
        egui::Sense::hover()
    } else {
        egui::Sense::click()
    };
    let pane_fill = ui.visuals().window_fill;
    let accent = color32_from_vec3(row.color);

    let tab_resp = ui
        .scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
            ui.set_width(ui.available_width());
            // A tab is exactly one row of the rail's virtualization, whatever
            // its content happens to measure. Without this the row pitch and
            // `TAB_HEIGHT` disagree and the list mis-virtualizes — see the
            // constant's own note.
            ui.set_min_height(TAB_HEIGHT);
            // Reserved now, sized after layout: `max_rect()` is all remaining
            // space, not this tab.
            let bg_idx = ui.painter().add(egui::Shape::Noop);
            let accent_idx = ui.painter().add(egui::Shape::Noop);
            let bar_idx = ui.painter().add(egui::Shape::Noop);

            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let (swatch, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(swatch, 2.0, accent);

                if is_renaming {
                    let resp = {
                        let (_, buf) = app.ui_state.renaming_transform.as_mut().unwrap();
                        ui.add(egui::TextEdit::singleline(buf).desired_width(70.0))
                    };
                    resp.request_focus();
                    let enter = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                    let escape = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                    if escape {
                        app.ui_state.renaming_transform = None;
                    } else if enter || resp.lost_focus() {
                        if let Some((idx, name)) = app.ui_state.renaming_transform.take() {
                            app.rename_transform(
                                idx,
                                if name.trim().is_empty() { None } else { Some(name) },
                            );
                        }
                    }
                } else {
                    let text = egui::RichText::new(&display_name);
                    let text = if !row.enabled {
                        text.color(ui.visuals().weak_text_color()).strikethrough()
                    } else if selected {
                        text.color(ui.visuals().strong_text_color()).strong()
                    } else {
                        text.color(ui.visuals().text_color())
                    };
                    ui.add(egui::Label::new(text).truncate().selectable(false));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    let eye_icon = if row.enabled { icons::EYE } else { icons::EYE_SLASH };
                    let eye_resp = ui.add(egui::Button::new(eye_icon).small().frame(false));
                    let eye_resp = hinted(
                        eye_resp,
                        &mut app.ui_state,
                        if row.enabled {
                            "Disable this transform (Enter)\n\nAlt+click to solo it — \
                             everything else off, and alt+click again to bring them back."
                        } else {
                            "Enable this transform (Enter)\n\nAlt+click to solo it."
                        },
                        "click: toggle enabled · alt+click: solo",
                    );
                    if eye_resp.clicked() {
                        // Alt+click on a visibility control means *solo* in
                        // Photoshop, Blender and every layer-ish list since —
                        // and "show me only this transform's contribution" is a
                        // real question to ask of an IFS, not a borrowed idiom.
                        if ui.input(|inp| inp.modifiers.alt) {
                            app.solo_transform(i);
                        } else {
                            app.toggle_transform_enabled(i);
                        }
                    }
                });
            });

            let mut rect = ui.min_rect();
            rect.max.y = rect.min.y + TAB_HEIGHT - TAB_GAP;
            // Selected tabs run past the rail's right edge so no border or gap
            // separates them from the detail pane.
            if selected {
                let mut fill_rect = rect;
                fill_rect.max.x += 6.0;
                ui.painter().set(
                    bg_idx,
                    egui::Shape::rect_filled(fill_rect, 3.0, pane_fill),
                );
                ui.painter().set(
                    accent_idx,
                    egui::Shape::rect_filled(
                        egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                        1.0,
                        accent,
                    ),
                );
            } else if ui.response().hovered() {
                ui.painter().set(
                    bg_idx,
                    egui::Shape::rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill),
                );
            }

            // Relative chaos weight, as a bar along the tab's bottom edge.
            let track = weight_bar_track(rect);
            let bar_w = track.width() * (row.weight / max_weight).clamp(0.0, 1.0);
            if bar_w > 0.5 {
                let bar = egui::Rect::from_min_size(track.min, egui::vec2(bar_w, BAR_THICKNESS));
                let color = if row.enabled {
                    accent.gamma_multiply(0.8)
                } else {
                    ui.visuals().weak_text_color()
                };
                ui.painter().set(bar_idx, egui::Shape::rect_filled(bar, 1.0, color));
            }
        })
        .response;

    if is_renaming {
        return;
    }

    let tab_resp = hinted(
        tab_resp,
        &mut app.ui_state,
        row.summary.as_str(),
        "click: select · double-click: rename · right-click: menu",
    );
    if tab_resp.clicked() {
        app.select_transform(Some(i));
    }

    // The weight bar, as a control rather than a readout.
    //
    // Registered *after* the tab, so egui hands it the pointer inside its own
    // strip — later registration wins. That puts the most-adjusted
    // per-transform value under the pointer that is already there to select the
    // row, which is the whole argument for it. It is also why the strip has to
    // be small and has to *say* where it is: everything it covers is a place
    // where clicking does not do the thing the tab under it advertises.
    //
    // Multiplicative, like the scroll gesture and the , / . keys, rather than
    // an absolute position-to-weight mapping. An absolute mapping would be a
    // feedback loop for whichever transform is currently the largest: the bar
    // is drawn relative to the maximum, so dragging the maximum wider changes
    // the scale it is measured against and the bar never moves.
    let mut body = tab_resp.rect;
    body.max.y -= TAB_GAP;
    let track = weight_bar_track(body);
    let bar_rect = egui::Rect::from_center_size(
        track.center(),
        egui::vec2(track.width(), BAR_GRAB),
    );
    let bar_resp = ui.interact(
        bar_rect,
        ui.id().with(("weight_bar", i)),
        egui::Sense::drag(),
    );
    let bar_active = bar_resp.hovered() || bar_resp.dragged();
    if bar_active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        // Say which of the two controls stacked here the pointer is on. The
        // cursor alone can't: it changes at the top of the app's window
        // furniture and is nowhere near the thing it's talking about, and a
        // tab that looks identical whether a click will select it or drag its
        // weight is a tab that surprises you every time it does the second.
        //
        // Drawn from the parent painter, after the tab's own shapes, so it
        // lands on top of the bar rather than needing last frame's hover
        // state to paint inside it.
        let painter = ui.painter();
        let grown = egui::Rect::from_center_size(
            track.center(),
            egui::vec2(track.width(), BAR_THICKNESS * 2.0),
        );
        // The full track first: hovering says how much room the value has to
        // move in, which is the one thing a bar drawn relative to the maximum
        // can't show while it's short.
        painter.rect_filled(grown, 1.0, ui.visuals().widgets.inactive.bg_fill);
        let frac = (row.weight / max_weight).clamp(0.0, 1.0);
        let mut fill = grown;
        fill.max.x = fill.min.x + grown.width() * frac;
        painter.rect_filled(fill, 1.0, color32_from_vec3(row.color));
    }
    let bar_resp = hinted(
        bar_resp,
        &mut app.ui_state,
        format!(
            "Chaos-game weight: {}. Drag sideways to change it.",
            super::num::fixed(row.weight, 2)
        ),
        "drag: adjust this transform's weight",
    );
    if bar_resp.dragged() {
        let dx = bar_resp.drag_delta().x;
        if dx != 0.0 {
            // ~5x across the width of the rail — enough range to be useful in
            // one gesture, gentle enough to land on a value.
            let next = row.weight * 1.015f32.powf(dx);
            app.set_transform_weight(i, next);
        }
    }
    // Double-click to rename: Blender's outliner, every file manager, every
    // editor's tab bar. Free here, since the name is drawn on this very row —
    // which is the whole reason the gesture is universal.
    if tab_resp.double_clicked() {
        app.select_transform(Some(i));
        app.ui_state.renaming_transform = Some((i, row.name.clone()));
    }
    tab_resp.context_menu(|ui| {
        if context_menu(ui, app, i) {
            ui.close();
        }
    });
}

/// Where a tab's weight bar is drawn, given the tab's painted body rect: a
/// `BAR_THICKNESS` strip along the bottom edge, inset from both ends.
///
/// One function so the painted bar and the strip you grab it by cannot drift
/// apart — the grab strip is this rect grown to `BAR_GRAB` about its centre.
fn weight_bar_track(body: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(body.min.x + BAR_INSET, body.max.y - 1.0 - BAR_THICKNESS),
        egui::vec2((body.width() - 2.0 * BAR_INSET).max(0.0), BAR_THICKNESS),
    )
}

/// One transform's context menu. Shared by its row in this window and by
/// right-clicking its gizmo in the viewport (`ui::draw_transform_menu`) — the
/// same operations wherever you find the transform, rather than two menus that
/// drift apart. Returns true when an item was chosen, i.e. when the caller
/// should close the menu.
pub fn context_menu(ui: &mut egui::Ui, app: &mut App, i: usize) -> bool {
    if i >= app.scene.transforms.len() {
        return true;
    }
    let name = app
        .scene
        .transform_names
        .get(i)
        .and_then(|n| n.clone())
        .unwrap_or_default();
    let enabled = app.is_transform_enabled(i);
    let mut chose = false;

    if ui.button("Duplicate").clicked() {
        app.duplicate_transform_at(i);
        chose = true;
    }
    if ui.button(if enabled { "Disable" } else { "Enable" }).clicked() {
        app.toggle_transform_enabled(i);
        chose = true;
    }
    // The chaos game needs somewhere to send a point, so the last transform
    // can't go. Shown disabled rather than hidden: a menu whose items move
    // around is harder to use than one with a greyed row.
    let can_delete = app.scene.transforms.len() > 1;
    let del = ui.add_enabled(can_delete, egui::Button::new("Delete"));
    if !can_delete {
        del.on_hover_text("A scene needs at least one transform");
    } else if del.clicked() {
        app.delete_transform_at(i);
        chose = true;
    }
    if ui.button("Rename").clicked() {
        app.ui_state.renaming_transform = Some((i, name));
        app.ui_state.panels.transforms_open = true;
        chose = true;
    }

    // Infinite zoom is a property of *one map* — you are choosing which
    // transform the scene's scale symmetry is — so it belongs anywhere you
    // say something about one map: this menu, and the detail pane's action
    // row. See `src/renorm.rs`.
    ui.separator();
    let zoom = zoom_action(app, i);
    let btn = ui.add_enabled(
        zoom.enabled,
        egui::Button::new(ZOOM_LABEL).selected(zoom.is_zoom),
    );
    let btn = hinted(btn, &mut app.ui_state, zoom.tooltip, zoom.hint);
    if btn.clicked() {
        app.set_zoom_map((!zoom.is_zoom).then_some(i));
        chose = true;
    }
    chose
}

/// What the "Zoom about this" control should say and do for transform `i`.
///
/// Shared by the context menu and the detail pane's action row: the same
/// operation wherever you find the transform, rather than two copies of a
/// fiddly `Renorm::build` check that drift apart.
pub struct ZoomAction {
    /// Already the scene's zoom map, so clicking clears it
    pub is_zoom: bool,
    /// The map renormalizes, or is already the one in use
    pub enabled: bool,
    pub tooltip: String,
    pub hint: &'static str,
}

/// A transform's display name, or `T<i>` when it hasn't got one.
pub fn transform_label(app: &App, i: usize) -> String {
    app.scene
        .transform_names
        .get(i)
        .and_then(|n| n.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("T{}", i))
}

/// The button's label. Constant, because "this is the one" is carried by
/// `Button::selected` rather than a tick in the text: the UI font has no
/// U+2713, so a "✓" prefix rendered as a missing-glyph box — and a highlight
/// says *selected* more directly than a character does anyway.
const ZOOM_LABEL: &str = "Zoom about this";

pub fn zoom_action(app: &App, i: usize) -> ZoomAction {
    let is_zoom = app.zoom_map() == Some(i);
    let built = crate::renorm::Renorm::build(
        &crate::renorm::ZoomSpec { map: i, ..Default::default() },
        &app.scene.transforms,
        app.scene.camera_distance,
    );
    let octaves = |r: &crate::renorm::Renorm| r.log_scale / std::f32::consts::LN_2;
    let tooltip = match (is_zoom, &built) {
        (true, Ok(r)) => format!(
            "This is the scene's zoom map: {:.2} octaves per zoom period, centred on \
             its fixed point ({:.2}, {:.2}, {:.2}). Click to turn infinite zoom off.",
            octaves(r),
            r.fixed_point.x,
            r.fixed_point.y,
            r.fixed_point.z,
        ),
        // Editing a transform can break the map it used to be. Say so rather
        // than silently greying the only control that could undo it.
        (true, Err(why)) => format!(
            "This is the scene's zoom map, but it no longer renormalizes: {}\n\n\
             Click to turn infinite zoom off.",
            why
        ),
        (false, Ok(r)) => format!(
            "Render this scene as the set invariant under this map: {:.2} octaves \
             per zoom period, no largest or smallest feature, zoom that never runs \
             out. The zoom centre is the map's fixed point, ({:.2}, {:.2}, {:.2}).",
            octaves(r),
            r.fixed_point.x,
            r.fixed_point.y,
            r.fixed_point.z,
        ),
        (false, Err(why)) => format!(
            "{}\n\nInfinite zoom needs a pure affine map that contracts on all three \
             axes; it renders the attractor as the unbounded set invariant under that \
             map, so there is no largest or smallest feature and zoom never runs out.",
            why
        ),
    };
    ZoomAction {
        is_zoom,
        enabled: is_zoom || built.is_ok(),
        tooltip,
        hint: if is_zoom {
            "click: turn infinite zoom off"
        } else {
            "click: make this the scene's zoom map"
        },
    }
}

/// The selected transform's own operations, in the pane rather than only
/// behind a right-click.
///
/// A control you can only reach by guessing that a context menu exists is one
/// most people never find: "Zoom about this" is the entire entry point to
/// infinite zoom, and from this window there was nothing to suggest it was
/// there at all. Enable/Disable and Rename were in the same position.
///
/// These act on *this* transform, so they sit on the transform's side of the
/// window. `+ add` and `dup` stay under the rail, because those act on the
/// list — which one you have selected is incidental to them.
fn draw_transform_actions(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    let enabled = app.is_transform_enabled(idx);
    // Wrapped: "✓ Zoom about this" is a wide label, and the pane is resizable
    // down to where three buttons no longer fit on one line.
    ui.horizontal_wrapped(|ui| {
        let resp = ui.add(egui::Button::new(if enabled { "Disable" } else { "Enable" }).small());
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if enabled {
                "Stop the chaos game choosing this transform — it keeps its weight \
                 and settings, it just stops contributing (Enter)"
            } else {
                "Let the chaos game choose this transform again (Enter)"
            },
            "click: toggle enabled",
        );
        if resp.clicked() {
            app.toggle_transform_enabled(idx);
        }

        // This used to open an inline editor over in the *rail*, six rows away
        // from the Name field sitting right below it — two visible rename
        // controls in one window that put your caret in different places, which
        // is worse than either alone. It opens the header's own editor now, so
        // the button and the name are one affordance rather than two. The
        // rail's inline editor is still there, reached by double-clicking a tab
        // where the name is also drawn.
        //
        // Kept even though the header name is click-to-edit: click-to-edit is
        // invisible until you hover the right four words, and this button is
        // what says the gesture exists.
        let resp = ui.add(egui::Button::new("Rename").small());
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Edit this transform's name, up in the header. Clicking the name \
             itself does the same; double-clicking its tab in the rail edits \
             it there instead.",
            "click: rename this transform",
        );
        if resp.clicked() {
            app.ui_state.focus_name_field = true;
        }

        let zoom = zoom_action(app, idx);
        let is_zoom = zoom.is_zoom;
        let resp = ui.add_enabled(
            zoom.enabled,
            egui::Button::new(ZOOM_LABEL).small().selected(is_zoom),
        );
        let resp = hinted(resp, &mut app.ui_state, zoom.tooltip, zoom.hint);
        if resp.clicked() {
            app.set_zoom_map((!is_zoom).then_some(idx));
        }
    });
}

/// The detail pane: everything about the selected transform.
fn draw_detail(ui: &mut egui::Ui, app: &mut App) {
    ui.vertical(|ui| {
        ui.set_min_width(280.0);
        ui.add_space(4.0);
        draw_inspector(ui, app);
    });
}

fn color32_from_vec3(c: Vec3) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c.x.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.y.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.z.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

// === Inspector ===

/// Decomposed TRS fields for a transform's matrix, plus whether decomposing
/// and recomposing it round-trips faithfully (see [`decompose_trs`]).
pub struct TrsFields {
    pub position: Vec3,
    /// Euler XYZ degrees (matches the scene-file rotation convention)
    pub rotation_deg: Vec3,
    pub scale: Vec3,
    /// True when the matrix is a faithful T*R*S composition: recomposing
    /// the decomposition reproduces the original within tolerance, AND the
    /// determinant is non-negative (a mirrored/reflected matrix is always
    /// routed to the non-TRS fallback, even if it happens to recompose
    /// closely, since a `Quat` can't represent the reflection itself).
    pub faithful: bool,
}

/// Decompose a matrix into position/rotation/scale and check whether that
/// decomposition is faithful (see [`TrsFields::faithful`]). Pure function —
/// no egui/GPU dependency — so the fidelity check is unit-testable on its
/// own (mutate.rs's rotate-after-scale composition is the main source of
/// sheared, non-faithful matrices in practice).
pub fn decompose_trs(matrix: Mat4) -> TrsFields {
    let trs = crate::rot::Trs::of(matrix);
    TrsFields {
        position: trs.translation,
        // The transform chart: extrinsic XYZ degrees, the same convention the
        // scene loader reads. Both go through `rot` so there is one definition
        // of it, not two that could drift.
        rotation_deg: Vec3::from(trs.rotation.to_xyz_degrees()),
        scale: trs.scale,
        faithful: trs.is_faithful(matrix),
    }
}

/// Cached inspector state for the selected transform, refreshed only when
/// `(transform_index, matrix_generation)` changes and no field in it is
/// actively being dragged/typed into (the plan's jitter guard) — otherwise a
/// same-frame GPU/history round trip would yank the field out from under an
/// in-progress drag.
pub struct TrsCache {
    key: (usize, u64),
    position: [f32; 3],
    rotation_deg: [f32; 3],
    scale: [f32; 3],
    uniform_scale_linked: bool,
    faithful: bool,
    /// Column-major, matching `Mat4::to_cols_array_2d` (`[col][row]`) — used
    /// for the non-TRS fallback grid.
    matrix_cols: [[f32; 4]; 4],
    /// The same three fields for the post-affine slot — the matrix applied
    /// *after* the variations. Cached alongside rather than in a second cache
    /// because both are decompositions of the same transform and both have to
    /// survive the same jitter guard.
    post_position: [f32; 3],
    post_rotation_deg: [f32; 3],
    post_scale: [f32; 3],
    post_uniform_scale_linked: bool,
    /// Whether the post-affine slot is showing. Sticky once opened, like
    /// `UiState::variation_rows`: a slot you have started editing must not
    /// vanish when you drag it back through identity mid-gesture.
    post_shown: bool,
    /// Set after drawing this frame's fields; read at the *start* of next
    /// frame to decide whether a generation bump should force a refresh.
    editing: bool,
}

impl TrsCache {
    fn from_matrix(idx: usize, generation: u64, matrix: Mat4, post: Mat4, post_shown: bool) -> Self {
        let fields = decompose_trs(matrix);
        let uniform_scale_linked = (fields.scale.x - fields.scale.y).abs() < 1e-4
            && (fields.scale.x - fields.scale.z).abs() < 1e-4;
        let pf = decompose_trs(post);
        let post_uniform_scale_linked = (pf.scale.x - pf.scale.y).abs() < 1e-4
            && (pf.scale.x - pf.scale.z).abs() < 1e-4;
        Self {
            key: (idx, generation),
            position: fields.position.to_array(),
            rotation_deg: fields.rotation_deg.to_array(),
            scale: fields.scale.to_array(),
            uniform_scale_linked,
            faithful: fields.faithful,
            matrix_cols: matrix.to_cols_array_2d(),
            post_position: pf.position.to_array(),
            post_rotation_deg: pf.rotation_deg.to_array(),
            post_scale: pf.scale.to_array(),
            post_uniform_scale_linked,
            post_shown,
            editing: false,
        }
    }
}

/// One of the inspector's four block headings.
fn block_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.separator();
    ui.label(egui::RichText::new(title).strong().small());
}

fn draw_inspector(ui: &mut egui::Ui, app: &mut App) {
    let Some(idx) = app.selected_transform() else {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Select a transform above to edit it here.")
                .weak()
                .italics(),
        );
        return;
    };
    if idx >= app.scene.transforms.len() {
        return;
    }

    // Header repeats the selected row's swatch and name so it's unambiguous
    // which row the fields below belong to — the list is a selector, and this
    // is the detail view for whatever it has selected. The name here is also
    // the *only* place the pane edits it: see `draw_header_name`.
    let name = app
        .scene
        .transform_names
        .get(idx)
        .and_then(|n| n.clone())
        .unwrap_or_default();
    let color = app.scene.colors.get(idx).copied().unwrap_or(Vec3::ONE);
    let n = app.scene.transforms.len();
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().rect_filled(swatch, 2.0, color32_from_vec3(color));
        draw_header_name(ui, app, idx, &name);
        ui.label(egui::RichText::new(format!("(T{})", idx)).weak().small());

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.add_enabled(n > 1, egui::Button::new(icons::X).small().frame(false));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Delete this transform (Del)",
                "click: delete this transform",
            );
            if resp.clicked() {
                app.delete_transform_at(idx);
            }
        });
    });
    ui.add_space(2.0);
    draw_transform_actions(ui, app, idx);
    ui.add_space(2.0);

    // Variation rows are remembered per selected transform; moving the
    // selection starts a fresh set (see `UiState::variation_rows`).
    if app.ui_state.variation_rows.0 != idx {
        app.ui_state.variation_rows = (idx, Vec::new());
    }

    let key = (idx, app.matrix_generation());
    let needs_refresh = match &app.ui_state.trs_cache {
        Some(c) if c.key == key => false,
        // Jitter guard: while one of this transform's own fields is being
        // dragged/typed into, our own commits bump the generation every
        // frame — don't let that re-decompose yank the field mid-edit. A
        // different transform index always refreshes (stale fields would
        // otherwise edit the wrong transform).
        Some(c) if c.key.0 == idx && c.editing => false,
        _ => true,
    };
    if needs_refresh {
        let matrix = app.scene.transforms[idx].matrix;
        let post = app.scene.transforms[idx].post_affine;
        // A slot the scene already uses is always showing; one left at
        // identity waits behind its button. Stickiness carries across the
        // refresh so a mid-drag pass through identity can't close the block.
        let shown = post != Mat4::IDENTITY
            || app
                .ui_state
                .trs_cache
                .as_ref()
                .is_some_and(|c| c.key.0 == idx && c.post_shown);
        app.ui_state.trs_cache = Some(TrsCache::from_matrix(idx, key.1, matrix, post, shown));
    }

    // Pull the cache out of `UiState` for the duration of the draw calls so
    // both `&mut App` (for committing edits) and `&mut TrsCache` (for the
    // DragValue targets) can be held at once.
    let mut cache = app.ui_state.trs_cache.take().unwrap();
    let mut editing = false;

    // Blocks with headings, rather than fifteen-odd controls in a flat stack
    // with two anonymous rules doing all the grouping work.
    //
    //   Shape       position, rotation, scale — already together
    //   Behaviour   weight *and* variations: what this map does in the chaos
    //               game. The two controls that most define the transform, and
    //               they used to sit at opposite ends with colour in between.
    //   Appearance  colour, colour value, colour speed
    //
    // There was an `Identity` block too, holding one Name field. It's gone:
    // the name is edited in the header that already displays it (see
    // `draw_header_name`), which is both nearer and one fewer heading.
    //
    // Headings rather than disclosure sections: `todo.txt` is explicit that
    // hidden bits are the problem and not the solution, and that instinct is
    // right. Separate with rules and headings; don't close things.
    block_heading(ui, "Shape");
    if cache.faithful {
        draw_trs_fields(ui, app, idx, &mut cache, &mut editing);
    } else {
        draw_matrix_grid(ui, app, idx, &mut cache, &mut editing);
    }

    // After Shape, because that is the order the map runs in: Shape, then the
    // variations under Behaviour, then this.
    block_heading(ui, "Post-affine");
    draw_post_affine(ui, app, idx, &mut cache, &mut editing);

    cache.editing = editing;
    app.ui_state.trs_cache = Some(cache);

    // And after Post-affine, by the same rule: the group composes on the
    // outside of the whole map, so it is the last thing that happens to a
    // point and the last block that describes one.
    block_heading(ui, "Symmetry");
    draw_symmetry(ui, app, idx);

    block_heading(ui, "Behaviour");
    draw_weight(ui, app, idx);
    draw_variations(ui, app, idx);

    block_heading(ui, "Appearance");
    draw_appearance(ui, app, idx);
}

fn drag_row(
    ui: &mut egui::Ui,
    app: &mut App,
    label: &str,
    fields: &mut [f32; 3],
    speed: f64,
    // `decimals`: how many places are shown, fixed, so the digit count can't
    // change under the drag. `chars`: the character budget for the number,
    // picked from the range the field can actually take.
    decimals: usize,
    chars: usize,
    suffix: &str,
    tooltip: &str,
    hint: &str,
    editing: &mut bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for (axis, v) in ["x", "y", "z"].iter().zip(fields.iter_mut()) {
            // Three content-sized DragValues in a horizontal used to be the
            // most hostile readout in the app: dragging **x** through zero
            // changed x's width and shoved **y** and **z** sideways *while your
            // pointer was on the control*. It moved the thing you were using,
            // mid-gesture. Fixed cells, so the row's geometry doesn't depend on
            // its contents at all.
            let resp = super::num::drag(
                ui,
                chars,
                decimals,
                egui::DragValue::new(v)
                    .speed(speed)
                    .prefix(format!("{}: ", axis))
                    .suffix(suffix),
            );
            let resp = hinted(resp, &mut app.ui_state, tooltip, hint);
            changed |= resp.changed();
            *editing |= resp.dragged() || resp.has_focus();
        }
    });
    changed
}

/// Which *variety* of group, with the fold count edited separately.
///
/// `C5` and `C3` are one choice with a number beside it, not two choices: a
/// picker with sixty cyclic segments in it would be unusable, and the fold
/// count is a quantity you scrub, not a category you pick.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variety {
    Cyclic,
    Dihedral,
    Tetrahedral,
    Octahedral,
    Icosahedral,
    /// Not a group. Sits in the same picker because the choice the author is
    /// making *is* one choice — what set of copies this map gets — and the
    /// badge below says which of them are symmetries.
    Repeat,
}

impl Variety {
    fn of(kind: crate::symmetry::OrbitKind) -> Self {
        use crate::symmetry::OrbitKind as G;
        match kind {
            G::Cyclic(_) => Variety::Cyclic,
            G::Dihedral(_) => Variety::Dihedral,
            G::Tetrahedral => Variety::Tetrahedral,
            G::Octahedral => Variety::Octahedral,
            G::Icosahedral => Variety::Icosahedral,
            G::Repeat(_) => Variety::Repeat,
        }
    }

    /// The kind this variety names, keeping `fold` where it means something so
    /// switching `C5 → D5 → C5` doesn't quietly reset the fold count.
    fn to_kind(self, fold: u32, step: crate::symmetry::Repeat) -> crate::symmetry::OrbitKind {
        use crate::symmetry::OrbitKind as G;
        match self {
            Variety::Cyclic => G::Cyclic(fold.max(1)),
            Variety::Dihedral => G::Dihedral(fold.max(2)),
            Variety::Tetrahedral => G::Tetrahedral,
            Variety::Octahedral => G::Octahedral,
            Variety::Icosahedral => G::Icosahedral,
            Variety::Repeat => G::Repeat(step),
        }
    }
}

/// The symmetry group this map is a motif of.
///
/// **Symmetry is a property of a transform here**, which is why it is a block
/// in this pane rather than a window of its own. A scene file writes it as a
/// `[[symmetry]]` block naming several motifs — that is how it reads best on a
/// page — but the thing itself belongs to the map, and so enrolling a map,
/// changing its group, and withdrawing it are all edits to one transform and
/// nothing else.
///
/// Takes the same bargain as `draw_post_affine`: offered as a button when the
/// map has no group, so the 44 scenes that don't use symmetry don't carry six
/// permanent rows about it, while the affordance itself is never hidden.
fn draw_symmetry(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    use crate::symmetry::{OrbitColor, Symmetry};

    let Some(spec) = app.scene.transforms.get(idx) else { return };

    // Only the description is read, never the elements: they are a pure
    // function of it, and copying 120 matrices out of the scene on every frame
    // of every panel draw to show a label would be silly.
    let Some((kind, axis, mirror, color)) = spec
        .symmetry
        .as_ref()
        .map(|s| (s.kind(), s.axis(), s.mirror(), s.color()))
    else {
        let resp = ui.button("Add symmetry…");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Make this map a motif of a symmetry group — every copy of it under \
             the group is added to the attractor",
            "click: enrol this map in a symmetry group",
        );
        if resp.clicked() {
            // C3 about Y: the smallest group with something to look at, and
            // the one whose effect on the picture is unmistakable.
            if let Ok(sym) = Symmetry::new(
                crate::symmetry::OrbitKind::Cyclic(3),
                glam::Vec3::Y,
                false,
                OrbitColor::Shared,
            ) {
                app.set_transform_symmetry(idx, Some(sym));
            }
        }
        ui.label(
            egui::RichText::new(
                "One map under a group is |G| maps in the attractor — and stays \
                 one row here, one entry in the file, and one thing to drag.",
            )
            .weak()
            .small(),
        );
        return;
    };

    let order = kind.order() * if mirror { 2 } else { 1 };
    let motifs = app
        .scene
        .transforms
        .iter()
        .filter(|t| {
            t.symmetry
                .as_ref()
                .is_some_and(|s| (s.kind(), s.mirror()) == (kind, mirror) && s.axis() == axis)
        })
        .count();

    // The badge: what this group is and what it costs, in one line. `maps` is
    // the arithmetic that makes the feature worth having, so it is on screen
    // rather than in a tooltip.
    ui.label(
        egui::RichText::new(format!(
            "{} · {} copies · {} motif{} = {} maps{}",
            if mirror { format!("{} + mirror", kind.label()) } else { kind.label() },
            order,
            motifs,
            if motifs == 1 { "" } else { "s" },
            motifs * order,
            // Said on the badge and not only in a tooltip, because it is the
            // one fact that changes what the picture will do: a group makes the
            // attractor exactly invariant, a repeat only makes it repetitive.
            if kind.is_group() { "" } else { " · repeats, not symmetric" },
        ))
        .strong()
        .small(),
    );

    let fold = kind.fold().unwrap_or(5);
    // Remembered the same way the fold count is, so flipping Repeat → Icosa →
    // Repeat comes back to the step you built rather than to the default.
    let step = kind.repeat().unwrap_or_default();
    let mut rebuild: Option<Symmetry> = None;

    // The group picker. A choose-1-of-n with no off state — withdrawing is a
    // separate verb below, not a sixth segment — so it is a segmented radio,
    // per the house rule.
    if let Some(v) = super::radio::radio(&mut app.ui_state, "sym_kind", Variety::of(kind))
        .option(
            Variety::Cyclic,
            "Cyc",
            "Cyclic (C\u{2099}): n-fold rotation about one axis — the mandala",
            "click: n-fold rotation about an axis",
        )
        .option(
            Variety::Dihedral,
            "Dih",
            "Dihedral (D\u{2099}): n-fold rotation plus a half-turn flip, so the \
             form has no top and bottom",
            "click: n-fold rotation plus a flip",
        )
        .option(
            Variety::Tetrahedral,
            "Tetra",
            "Tetrahedral (T): the 12 rotations of a tetrahedron",
            "click: tetrahedral group (12 copies)",
        )
        .option(
            Variety::Octahedral,
            "Octa",
            "Octahedral (O): the 24 rotations of a cube",
            "click: octahedral group (24 copies)",
        )
        .option(
            Variety::Icosahedral,
            "Icosa",
            "Icosahedral (I): the 60 rotations of an icosahedron — the largest \
             finite rotation group there is",
            "click: icosahedral group (60 copies)",
        )
        .option(
            Variety::Repeat,
            "Repeat",
            "Repeat: n copies stepped by a turn, a slide and a shrink — a helix, \
             a spiral, a row. The only one here that is not a group: it makes \
             the attractor repetitive, not symmetric",
            "click: n copies along a step",
        )
        .show(ui)
    {
        rebuild = Symmetry::new(v.to_kind(fold, step), axis, mirror, color).ok();
    }

    // The fold count, for the two groups that have one. Disabled rather than
    // hidden under T/O/I, with the tooltip saying why: a control that vanishes
    // can't tell you it exists.
    ui.horizontal(|ui| {
        ui.label("folds");
        let mut n = fold as f32;
        let resp = ui.add_enabled(
            kind.fold().is_some(),
            egui::DragValue::new(&mut n)
                .speed(0.08)
                .range(if matches!(kind, crate::symmetry::OrbitKind::Dihedral(_)) {
                    2.0..=crate::symmetry::MAX_FOLD as f32
                } else {
                    1.0..=crate::symmetry::MAX_FOLD as f32
                })
                .fixed_decimals(0),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if kind.fold().is_some() {
                "How many copies around the axis. Every copy is the same map, \
                 so this costs nothing but arithmetic"
            } else if kind.repeat().is_some() {
                "A repeat sets its own count below — this is the fold count of \
                 the cyclic and dihedral groups"
            } else {
                "The polyhedral groups have a fixed size — Tetra is 12, Octa is \
                 24, Icosa is 60. Pick Cyc or Dih to choose a fold count"
            },
            "drag: how many copies around the axis",
        );
        if resp.changed() {
            rebuild = Symmetry::new(
                Variety::of(kind).to_kind(n.round() as u32, step),
                axis,
                mirror,
                color,
            )
            .ok();
        }

        let mut m = mirror;
        let resp = ui.checkbox(&mut m, "mirror");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Add the central inversion, doubling the group: every copy gains a \
             twin through the origin. Changes no contraction, so it does not \
             move the dimension",
            "click: double the group with a central inversion",
        );
        if resp.changed() {
            rebuild = Symmetry::new(kind, axis, m, color).ok();
        }
    });

    // The axis, for C/D. This is the one row in the block with a gesture in the
    // viewport to match it — the drawn axis and its spokes — so the numbers and
    // the drawing say the same thing.
    //
    // Greyed under T/O/I rather than removed, like `folds` above: the
    // polyhedral groups genuinely have no axis to aim, and a row that vanished
    // would leave you wondering whether the control had moved or you had
    // imagined it. The tooltip is where it says why.
    {
        let mut a = axis.to_array();
        let mut changed = false;
        let aimable = kind.uses_axis();
        ui.horizontal(|ui| {
            ui.label("axis ");
            for (name, v) in ["x", "y", "z"].iter().zip(a.iter_mut()) {
                let resp = ui.add_enabled_ui(aimable, |ui| {
                    super::num::drag(
                        ui,
                        5,
                        2,
                        egui::DragValue::new(v).speed(0.01).prefix(format!("{}: ", name)),
                    )
                });
                let resp = hinted(
                    resp.inner,
                    &mut app.ui_state,
                    if aimable {
                        "The axis the copies turn about. Drawn in the viewport \
                         with one spoke per fold"
                    } else {
                        "The polyhedral groups have no single axis — their \
                         symmetry axes come in threes, fours and fives at fixed \
                         angles. The viewport draws the group's solid instead. \
                         Pick Cyc or Dih to aim an axis"
                    },
                    "drag: aim the symmetry axis",
                );
                changed |= resp.changed();
            }
        });
        if changed && aimable {
            // A zero axis names no rotation, so it is simply not applied —
            // dragging x through zero must not destroy the group on the way
            // past.
            rebuild = Symmetry::new(kind, glam::Vec3::from(a), mirror, color).ok().or(rebuild);
        }
    }

    // The repeat's step. Only under Repeat — unlike `folds`, which is greyed
    // rather than hidden because it belongs to a group you might switch back
    // to, these five rows describe a thing the other five kinds simply do not
    // have, and greying five rows is a wall rather than a hint.
    if let Some(mut r) = kind.repeat() {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("copies");
            let mut n = r.count as f32;
            let resp = ui.add(
                egui::DragValue::new(&mut n)
                    .speed(0.1)
                    .range(1.0..=crate::symmetry::MAX_REPEAT as f32)
                    .fixed_decimals(0),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "How many copies, counting the motif itself. Each one is the \
                 previous one stepped again",
                "drag: how many copies",
            );
            if resp.changed() {
                r.count = n.round().max(1.0) as u32;
                changed = true;
            }

            ui.label("turn");
            let resp = ui.add(egui::DragValue::new(&mut r.turn).speed(0.5).suffix("°"));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Degrees about the axis per copy. 137.5 is the golden angle — \
                 the one that never lines a copy up with an earlier one",
                "drag: degrees per copy",
            );
            changed |= resp.changed();
        });

        ui.horizontal(|ui| {
            ui.label("step");
            for (label, v) in [
                ("x", &mut r.translate.x),
                ("y", &mut r.translate.y),
                ("z", &mut r.translate.z),
            ] {
                let resp = ui.add(egui::DragValue::new(v).speed(0.005).prefix(label).fixed_decimals(3));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "How far each copy slides from the last. Along the axis this \
                     is a helix; across it, a fan",
                    "drag: slide per copy",
                );
                changed |= resp.changed();
            }
        });

        ui.horizontal(|ui| {
            ui.label("shrink");
            let resp = ui.add(
                egui::DragValue::new(&mut r.scale).speed(0.002).range(0.05..=1.0).fixed_decimals(3),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Size multiplier per copy. Capped at 1: a step that grows makes \
                 the far copies expansive and the walk runs away. Start from the \
                 far end instead",
                "drag: size multiplier per copy",
            );
            changed |= resp.changed();
        });

        if changed {
            rebuild = Symmetry::new(crate::symmetry::OrbitKind::Repeat(r), axis, mirror, color)
                .ok()
                .or(rebuild);
        }
    }

    // The orbit's colour mode is not drawn here. It is a colour control, so it
    // lives under Appearance beside the other two — see `draw_orbit_color`.

    if let Some(sym) = rebuild {
        app.set_transform_symmetry(idx, Some(sym));
    }

    ui.horizontal(|ui| {
        // The defect, one click away and sitting right next to the group it is
        // a defect in. A group orbit flattens the measure by construction —
        // every copy shares a contraction and a weight — so a scene that is
        // 100% symmetric is CRAFT §3.6's unrecoverable case. Breaking the
        // symmetry a little is not an advanced option; it is the second thing
        // you do, so it is the second button here.
        let resp = ui.button("Add a map outside this group");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Add a plain transform with no symmetry. A wholly symmetric scene \
             has a flat measure by construction — one map off the group is what \
             gives it something to look at",
            "click: add an unsymmetric map (the deliberate defect)",
        );
        if resp.clicked() {
            app.add_transform(true);
        }

        let resp = ui.button("Withdraw");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Take this map out of its group. It stays exactly where it is; its \
             copies stop being drawn",
            "click: remove this map from its symmetry group",
        );
        if resp.clicked() {
            app.set_transform_symmetry(idx, None);
        }
    });
}

/// The post-affine slot: the matrix applied *after* the variation blend.
///
/// Three rows that look exactly like Shape's, because they are the same three
/// numbers doing the same job at the other end of the map — the whole point is
/// that a person who can drive Shape can drive this without learning anything.
///
/// The block is shown when the slot is in use and offered as a button when it
/// isn't. That is not the disclosure section the inspector's own comment warns
/// against: the heading and the affordance are always on screen, so there is
/// nothing hidden to fail to find. It is the `variation_rows` bargain — a
/// slot appears when you ask for it and then stays put, rather than every map
/// permanently carrying three rows that 43 of the repo's 44 scenes leave at
/// identity.
fn draw_post_affine(ui: &mut egui::Ui, app: &mut App, idx: usize, cache: &mut TrsCache, editing: &mut bool) {
    if !cache.post_shown {
        let resp = ui.button("Add post-affine…");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Add a matrix applied after this map's variations (fold-then-rotate)",
            "click: add a post-affine transform to this map",
        );
        if resp.clicked() {
            cache.post_shown = true;
        }
        ui.label(
            egui::RichText::new(
                "Runs after the variations. Only changes anything when a \
                 non-linear variation is in play — with pure linear it folds \
                 into Shape. See scenes/fold_lantern.toml.",
            )
            .small()
            .weak(),
        );
        return;
    }

    let mut changed = false;

    changed |= drag_row(
        ui,
        app,
        "Position",
        &mut cache.post_position,
        0.01,
        3,
        7,
        "",
        "Post-affine position, applied after the variations",
        "drag: adjust post position · click: type exact value",
        editing,
    );

    changed |= drag_row(
        ui,
        app,
        "Rotation",
        &mut cache.post_rotation_deg,
        0.5,
        1,
        6,
        "°",
        "Post-affine rotation — rotating *after* a fold is what this slot is for",
        "drag: adjust post rotation · click: type exact value",
        editing,
    );

    let mut post_scale_axis: Option<usize> = None;
    ui.horizontal(|ui| {
        ui.label("Scale");
        let resp = ui.checkbox(&mut cache.post_uniform_scale_linked, "linked");
        hinted(
            resp,
            &mut app.ui_state,
            "Lock per-axis post scale to change together",
            "click: toggle uniform post-scale link",
        );
    });
    ui.horizontal(|ui| {
        for (axis, (label, v)) in
            ["x", "y", "z"].iter().zip(cache.post_scale.iter_mut()).enumerate()
        {
            let resp = super::num::drag(
                ui,
                10,
                3,
                egui::DragValue::new(v)
                    .speed(0.005)
                    .prefix(format!("{}: ", label))
                    .range(-1000.0..=1000.0),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Post-affine scale. This counts toward the map's contraction \
                 just as Shape's does — the status bar's d moves with it.",
                "drag: adjust post scale · click: type exact value",
            );
            if resp.changed() {
                post_scale_axis = Some(axis);
            }
            *editing |= resp.dragged() || resp.has_focus();
        }
    });
    if let Some(axis) = post_scale_axis {
        if cache.post_uniform_scale_linked {
            let v = cache.post_scale[axis];
            cache.post_scale = [v, v, v];
        }
        changed = true;
    }

    let resp = ui.button("Clear post-affine");
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Reset this map's post-affine slot to the identity",
        "click: clear the post-affine transform",
    );
    if resp.clicked() {
        cache.post_position = [0.0; 3];
        cache.post_rotation_deg = [0.0; 3];
        cache.post_scale = [1.0; 3];
        cache.post_shown = false;
        app.set_transform_post_affine(idx, Mat4::IDENTITY, format!("Clear post-affine T{}", idx), None);
        return;
    }

    if changed {
        let post = crate::rot::Trs {
            scale: Vec3::from(cache.post_scale),
            rotation: crate::rot::Orientation::from_xyz_degrees(cache.post_rotation_deg),
            translation: Vec3::from(cache.post_position),
        }
        .matrix();
        app.set_transform_post_affine(
            idx,
            post,
            format!("Post-affine T{}", idx),
            Some(format!("insp:t{}:post", idx)),
        );
    }
}

fn draw_trs_fields(ui: &mut egui::Ui, app: &mut App, idx: usize, cache: &mut TrsCache, editing: &mut bool) {
    let mut changed_field: Option<&'static str> = None;

    if drag_row(
        ui,
        app,
        "Position",
        &mut cache.position,
        0.01,
        // Positions live in roughly -9..9 in every shipped scene; three
        // decimals is what the gizmo drag resolves to.
        3,
        7,
        "",
        "Transform position (drag the gizmo's origin dot)",
        "drag: adjust position · click: type exact value",
        editing,
    ) {
        changed_field = Some("position");
    }

    if drag_row(
        ui,
        app,
        "Rotation",
        &mut cache.rotation_deg,
        0.5,
        // Euler degrees: -180.0 to 180.0.
        1,
        6,
        "°",
        "Transform rotation, XYZ Euler degrees (drag an outer gizmo edge)",
        "drag: adjust rotation · click: type exact value",
        editing,
    ) {
        changed_field = Some("rotation");
    }

    ui.horizontal(|ui| {
        ui.label("Scale");
        let resp = ui.checkbox(&mut cache.uniform_scale_linked, "linked");
        hinted(
            resp,
            &mut app.ui_state,
            "Lock per-axis scale to change together (ctrl+drag any gizmo part)",
            "click: toggle uniform-scale link",
        );

        // How much smaller this copy is than the grey identity cell drawn in
        // the viewport — the number the reference gizmo is there to convey,
        // stated rather than left to be eyeballed.
        let contraction = app.scene.transforms[idx].contraction();
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(format!("×{}", super::num::cell(contraction, 3, 6)))
                    .monospace()
                    .weak()
                    .small(),
            )
            .sense(egui::Sense::hover()),
        );
        hinted(
            resp,
            &mut app.ui_state,
            "Contraction: linear size of this copy relative to the grey identity cell (cube root of |det|). Below 1 the IFS converges.",
            "the transform's contraction against the identity cell",
        );
    });
    let mut scale_changed_axis: Option<usize> = None;
    ui.horizontal(|ui| {
        for (axis, (label, v)) in ["x", "y", "z"].iter().zip(cache.scale.iter_mut()).enumerate() {
            // Negative values are allowed: they mirror the transform, which
            // reads back as a det<0 matrix and routes the inspector to the
            // matrix (non-TRS) view — intentional, not an error.
            let resp = super::num::drag(
                ui,
                10,
                3,
                egui::DragValue::new(v)
                    .speed(0.005)
                    .prefix(format!("{}: ", label))
                    .range(-1000.0..=1000.0),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Transform scale (ctrl+drag any gizmo part = uniform scale; negative mirrors)",
                "drag: adjust scale · click: type exact value",
            );
            if resp.changed() {
                scale_changed_axis = Some(axis);
            }
            *editing |= resp.dragged() || resp.has_focus();
        }
    });
    if let Some(axis) = scale_changed_axis {
        if cache.uniform_scale_linked {
            let v = cache.scale[axis];
            cache.scale = [v, v, v];
        }
        changed_field = Some("scale");
    }

    if let Some(field) = changed_field {
        commit_trs(app, idx, cache, field);
    }
}

fn commit_trs(app: &mut App, idx: usize, cache: &TrsCache, field: &str) {
    let matrix = crate::rot::Trs {
        scale: Vec3::from(cache.scale),
        rotation: crate::rot::Orientation::from_xyz_degrees(cache.rotation_deg),
        translation: Vec3::from(cache.position),
    }
    .matrix();
    let label_field = match field {
        "position" => "Position",
        "rotation" => "Rotation",
        "scale" => "Scale",
        other => other,
    };
    app.set_transform_matrix(
        idx,
        matrix,
        format!("{} T{}", label_field, idx),
        Some(format!("insp:t{}:{}", idx, field)),
    );
}

fn draw_matrix_grid(ui: &mut egui::Ui, app: &mut App, idx: usize, cache: &mut TrsCache, editing: &mut bool) {
    ui.label(egui::RichText::new("matrix (non-TRS)").color(egui::Color32::from_rgb(230, 180, 80)));
    ui.label(
        egui::RichText::new(
            "Shear, or a mirrored (det<0) matrix — edit components directly, or discard the shear/mirroring.",
        )
        .small()
        .weak(),
    );

    let mut changed = false;
    egui::Grid::new(("transform_matrix_grid", idx))
        .num_columns(5)
        .show(ui, |ui| {
            ui.label("");
            for h in ["X", "Y", "Z", "T"] {
                ui.label(h);
            }
            ui.end_row();

            for row in 0..3 {
                ui.label(["x", "y", "z"][row]);
                for col in 0..4 {
                    let v = &mut cache.matrix_cols[col][row];
                    let resp = ui.add(egui::DragValue::new(v).speed(0.01));
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Raw matrix component",
                        "drag: adjust matrix component · click: type exact value",
                    );
                    changed |= resp.changed();
                    *editing |= resp.dragged() || resp.has_focus();
                }
                ui.end_row();
            }
        });

    if changed {
        let c = cache.matrix_cols;
        let matrix = Mat4::from_cols(
            Vec4::new(c[0][0], c[0][1], c[0][2], 0.0),
            Vec4::new(c[1][0], c[1][1], c[1][2], 0.0),
            Vec4::new(c[2][0], c[2][1], c[2][2], 0.0),
            Vec4::new(c[3][0], c[3][1], c[3][2], 1.0),
        );
        app.set_transform_matrix(
            idx,
            matrix,
            format!("Edit matrix T{}", idx),
            Some(format!("insp:t{}:matrix", idx)),
        );
    }

    let resp = ui.button("Orthogonalize -> TRS");
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Discard shear/mirroring: replace with the nearest scale·rotate·translate matrix",
        "click: orthogonalize to TRS",
    );
    if resp.clicked() {
        app.orthogonalize_transform(idx);
        // The orthogonalize commit already bumped matrix_generation; drop
        // the cache so the inspector re-decomposes (and likely switches to
        // the TRS fields) on the very next draw.
        app.ui_state.trs_cache = None;
    }
}

/// Weight — how often the chaos game picks this map.
///
/// Sits with the variations, under **Behaviour**, because those two are the
/// pair that most define what a transform *does*: one says how often it's
/// chosen, the other says what it does when it is. They used to sit at opposite
/// ends of the pane with the entire colour block between them.
fn draw_weight(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    let mut weight = app.scene.transforms[idx].weight;
    ui.horizontal(|ui| {
        ui.label("Weight");
        let speed = (weight.abs().max(0.01) * 0.05) as f64;
        let resp = super::num::drag(
            ui,
            6,
            2,
            egui::DragValue::new(&mut weight).speed(speed).range(0.01..=100.0),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "How often the chaos game picks this map, relative to the others (, / .)",
            "drag: adjust weight · click: type exact value",
        );
        if resp.changed() {
            app.set_transform_weight(idx, weight);
        }
    });
}

fn draw_appearance(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    // In palette mode the per-transform RGB is not rendered at all — the
    // gradient owns the colours — so showing a swatch there would be a
    // control that does nothing. What matters instead is *where in the
    // gradient* this transform sits, drawn on the gradient itself.
    if app.scene.color_mode == crate::scene::ColorMode::Palette {
        ui.label("Color value");
        super::gradient::transform_color_value(ui, app, idx);
        draw_orbit_color(ui, app, idx);
        return;
    }

    let color = app.scene.colors[idx];
    let mut srgb = [
        (color.x.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.y.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.z.clamp(0.0, 1.0) * 255.0).round() as u8,
    ];
    ui.horizontal(|ui| {
        ui.label("Color");
        let resp = ui.color_edit_button_srgb(&mut srgb);
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "This transform's gradient color (J/K/L: hue/saturation/value)",
            "click: pick a color",
        );
        if resp.changed() {
            let new_color = Vec3::new(
                srgb[0] as f32 / 255.0,
                srgb[1] as f32 / 255.0,
                srgb[2] as f32 / 255.0,
            );
            app.set_transform_color(idx, new_color);
        }
    });

    let mut cv = app.scene.transforms[idx].color_value;
    let resp = ui.add(egui::Slider::new(&mut cv, 0.0..=1.0).text("color value"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Explicit colormap index override (0-1 around the cyclic gradient)",
        "drag: adjust colormap index",
    );
    if resp.changed() {
        app.set_transform_color_value(idx, cv);
    }

    let mut has_override = app.scene.transforms[idx].explicit_color_speed.is_some();
    let resp = ui.checkbox(&mut has_override, "Override color speed");
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Per-transform color blend speed (wins over global color_speed / color_falloff)",
        "click: toggle color-speed override",
    );
    if resp.changed() {
        if has_override {
            let current = app.scene.transforms[idx].color_speed;
            app.set_transform_explicit_color_speed(idx, Some(current));
        } else {
            app.set_transform_explicit_color_speed(idx, None);
        }
    }
    if has_override {
        let mut speed = app.scene.transforms[idx]
            .explicit_color_speed
            .unwrap_or(app.scene.color_speed);
        let resp = ui.add(egui::Slider::new(&mut speed, 0.0..=1.0).text("color speed"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Color blend speed (0 = slow/cohesive, 1 = fast/noisy)",
            "drag: adjust color speed",
        );
        if resp.changed() {
            app.set_transform_explicit_color_speed(idx, Some(speed));
        }
    }

    draw_orbit_color(ui, app, idx);
}

/// How the symmetry orbit is coloured.
///
/// It lives under Appearance rather than under Symmetry because it is a colour
/// control — it changes nothing about the geometry, only which colormap index
/// each copy lands on, so it belongs beside the other two colour rows and not
/// beside the group picker. The group *is* its subject, though, so the row is
/// only drawn when this transform has one; on a plain map there is no orbit to
/// colour and the row would be a dead control.
fn draw_orbit_color(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    let Some((kind, axis, mirror, color)) = app.scene.transforms[idx]
        .symmetry
        .as_ref()
        .map(|s| (s.kind(), s.axis(), s.mirror(), s.color()))
    else {
        return;
    };

    ui.horizontal(|ui| {
        ui.label("Orbit");
        if let Some(c) = super::radio::radio(&mut app.ui_state, "sym_color", color)
            .option(
                crate::symmetry::OrbitColor::Shared,
                "shared",
                "Every copy takes the motif's own colour — the copies are the \
                 same map, and this says so",
                "click: one colour across the orbit",
            )
            .option(
                crate::symmetry::OrbitColor::Orbit,
                "orbit",
                "Offset the colour index by which group element was drawn. The \
                 element is redrawn every iteration, so this reads as an \
                 interference pattern across the form, not as |G| solid petals",
                "click: colour by the drawn group element",
            )
            .show(ui)
        {
            if let Ok(sym) = crate::symmetry::Symmetry::new(kind, axis, mirror, c) {
                app.set_transform_symmetry(idx, Some(sym));
            }
        }
    });
}

/// The egui id of the inspector's Name field, so the action row's Rename button
/// can put the caret in it (see `draw_transform_actions`).
pub const NAME_FIELD_ID: &str = "fracturize_inspector_name";

/// The pane header's name: a label until it's clicked, a text field after.
///
/// This is the *only* place the pane edits a name. It used to carry a whole
/// "Identity" block whose entire content was one Name field — last of five
/// headings, below the variations and the colour, at the bottom of a pane you
/// have to scroll to reach on any transform with a few variations on it. A
/// heading for one field is furniture that doesn't earn its rule and its
/// label, and putting the most-edited property of a transform furthest from
/// the header that already displays it is exactly backwards.
///
/// Click-to-edit-in-place, so the name is edited where it is read. The gesture
/// matches the rail's own double-click-to-rename (single click here, because a
/// single click on the header has nothing else to mean — over in the rail it
/// selects the row).
fn draw_header_name(ui: &mut egui::Ui, app: &mut App, idx: usize, name: &str) {
    // An editor left open by a selection change belongs to the transform it
    // was opened on, not to whatever is selected now.
    if app.ui_state.editing_name.as_ref().is_some_and(|(i, _)| *i != idx) {
        app.ui_state.editing_name = None;
    }
    // The action row's Rename button asks for the caret by opening this.
    if app.ui_state.focus_name_field {
        app.ui_state.focus_name_field = false;
        app.ui_state.editing_name = Some((idx, name.to_owned()));
    }

    if app.ui_state.editing_name.is_some() {
        let resp = {
            let (_, buf) = app.ui_state.editing_name.as_mut().unwrap();
            ui.add(
                egui::TextEdit::singleline(buf)
                    .id(egui::Id::new(NAME_FIELD_ID))
                    .desired_width(150.0)
                    .hint_text("unnamed"),
            )
        };
        resp.request_focus();
        let enter = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
        let escape = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
        if escape {
            app.ui_state.editing_name = None;
        } else if enter || resp.lost_focus() {
            if let Some((i, text)) = app.ui_state.editing_name.take() {
                let trimmed = text.trim();
                app.rename_transform(
                    i,
                    if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) },
                );
            }
        }
        return;
    }

    // Unnamed transforms say so rather than repeating the `(T3)` index label
    // sitting immediately to their right, which is what the fallback name did.
    let text = if name.trim().is_empty() {
        egui::RichText::new("unnamed").weak().italics()
    } else {
        egui::RichText::new(name).strong()
    };
    let resp = ui.add(egui::Label::new(text).sense(egui::Sense::click()).selectable(false));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "This transform's label, shown in the rail and on its gizmo. Click to \
         edit it here; double-clicking its tab in the rail edits it there.",
        "click: rename this transform",
    );
    if resp.clicked() {
        app.ui_state.editing_name = Some((idx, name.to_owned()));
    }
}

fn draw_variations(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    ui.label(egui::RichText::new("Variations").strong());

    let weights = app.scene.transforms[idx].variations;
    let selected_slot = app.selected_variation();

    // A row is shown when the slot carries weight, or when it's been pinned:
    // either freshly added, or dragged down to zero. Zero is a perfectly
    // ordinary place to pass through — the blend is `out += w * f(p)`, so
    // sweeping a weight from +0.6 to -0.6 is a continuous exploration, and a
    // row that vanished at the origin would both interrupt the gesture and
    // make negative weights unreachable by mouse. Rows leave only via their
    // own ✕.
    let pinned = app.ui_state.variation_rows.1.clone();
    let shown: Vec<usize> = (0..NUM_VARIATIONS)
        .filter(|&s| weights[s] != 0.0 || pinned.contains(&s))
        .collect();

    let mut change: Option<(usize, f32)> = None;
    let mut remove: Option<usize> = None;

    for &slot in &shown {
        ui.horizontal(|ui| {
            let text = if selected_slot == slot {
                egui::RichText::new(VARIATION_NAMES[slot]).strong()
            } else {
                egui::RichText::new(VARIATION_NAMES[slot])
            };
            let name_resp = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
            let name_resp = hinted(
                name_resp,
                &mut app.ui_state,
                "Click to target this slot for E/-/= (keeps keyboard cycling in sync)",
                "click: target this variation slot",
            );
            if name_resp.clicked() {
                app.set_selected_variation(slot);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let x_resp = ui.add(egui::Button::new(icons::X).small().frame(false));
                let x_resp = hinted(
                    x_resp,
                    &mut app.ui_state,
                    "Remove this variation from the transform",
                    "click: remove this variation",
                );
                if x_resp.clicked() {
                    remove = Some(slot);
                }

                let mut v = weights[slot];
                // Apophysis-style negative weights mean this one crosses zero
                // constantly, and it sits in a row with a remove button.
                let resp = super::num::drag(
                    ui,
                    5,
                    2,
                    egui::DragValue::new(&mut v).speed(0.05).range(-4.0..=4.0),
                );
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Variation blend weight, Apophysis-style — negative inverts the variation's contribution (- / = on the targeted slot)",
                    "drag: adjust weight · click: type exact value",
                );
                if resp.changed() {
                    // Snap through zero cleanly, matching adjust_variation_weight
                    let snapped = (v * 100.0).round() / 100.0;
                    change = Some((slot, snapped));
                }
            });
        });
    }

    if let Some(slot) = remove {
        app.set_transform_variation(idx, slot, 0.0);
        app.ui_state.variation_rows.1.retain(|&s| s != slot);
    }
    if let Some((slot, w)) = change {
        app.set_transform_variation(idx, slot, w);
        // Pin on the way through zero so the row the user is dragging stays
        // put for the rest of the gesture.
        if !app.ui_state.variation_rows.1.contains(&slot) {
            app.ui_state.variation_rows.1.push(slot);
        }
    }

    let unused: Vec<usize> = (0..NUM_VARIATIONS).filter(|&s| !shown.contains(&s)).collect();
    if !unused.is_empty() {
        let mut chosen: Option<usize> = None;
        let combo_resp = egui::ComboBox::new(("add_variation", idx), "add variation")
            .selected_text("+ add...")
            .show_ui(ui, |ui| {
                for &slot in &unused {
                    if ui.selectable_label(false, VARIATION_NAMES[slot]).clicked() {
                        chosen = Some(slot);
                    }
                }
            })
            .response;
        hinted(
            combo_resp,
            &mut app.ui_state,
            "Add a variation to this transform",
            "click: choose a variation to add",
        );
        if let Some(slot) = chosen {
            app.set_transform_variation(idx, slot, 0.35);
            app.set_selected_variation(slot);
            app.ui_state.variation_rows.1.push(slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{EulerRot, Mat4, Quat, Vec3};

    #[test]
    fn pure_trs_composition_is_faithful() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(0.5, 0.7, 0.3),
            Quat::from_euler(EulerRot::XYZ, 0.3, -0.6, 1.1),
            Vec3::new(0.2, -0.4, 0.1),
        );
        let fields = decompose_trs(m);
        assert!(fields.faithful, "a pure T*R*S composition must decompose faithfully");
        assert!((fields.scale - Vec3::new(0.5, 0.7, 0.3)).length() < 1e-3);
    }

    #[test]
    fn uniform_scale_is_faithful() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.42),
            Quat::IDENTITY,
            Vec3::new(1.0, 2.0, 3.0),
        );
        assert!(decompose_trs(m).faithful);
    }

    #[test]
    fn sheared_matrix_is_not_faithful() {
        // An explicit shear (y column leaning into x): no T*R*S composition
        // can reproduce non-orthogonal columns, so the recompose-compare
        // check must fail and route the inspector to the matrix fallback.
        //
        // Note: mutate.rs's rotate op canNOT produce this — it left-
        // multiplies a rotation onto an R*S linear part, and rot*(R*S) =
        // (rot*R)*S keeps the columns orthogonal. Shear only enters via a
        // non-uniform scale applied AFTER a rotation (S2*R*S1), direct
        // matrix-grid edits, or externally authored matrices.
        let sheared = Mat4::from_cols(
            glam::Vec4::new(1.0, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.4, 1.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 1.0, 0.0),
            glam::Vec4::new(0.1, 0.2, 0.3, 1.0),
        );
        assert!(
            !decompose_trs(sheared).faithful,
            "a sheared linear part must be routed to the non-TRS fallback"
        );
    }

    #[test]
    fn rotation_after_scale_stays_faithful() {
        // Pin the counterintuitive case: left-multiplying a rotation onto an
        // anisotropically-scaled transform (mutate.rs's rotate op) keeps the
        // columns orthogonal — (rot*R)*S is still a clean TRS. The inspector
        // must keep showing TRS fields for these, not the fallback grid.
        let base = Mat4::from_scale_rotation_translation(
            Vec3::new(0.05, 0.6, 0.05),
            Quat::IDENTITY,
            Vec3::new(0.2, -0.1, 0.4),
        );
        let rot = Mat4::from_quat(Quat::from_axis_angle(Vec3::new(1.0, 1.0, 0.3).normalize(), 0.7));
        let mut rotated = rot * Mat4::from_cols(base.x_axis, base.y_axis, base.z_axis, glam::Vec4::W);
        rotated.w_axis = base.w_axis;
        assert!(
            decompose_trs(rotated).faithful,
            "rotate-after-scale keeps orthogonal columns and must stay on the TRS path"
        );
    }

    #[test]
    fn mirrored_det_negative_routes_to_fallback() {
        // Mirror one axis: determinant goes negative even though the matrix
        // is otherwise a clean axis-aligned scale (no shear).
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(-0.5, 0.5, 0.5),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        assert!(m.determinant() < 0.0, "test setup: expected a mirrored matrix");
        assert!(
            !decompose_trs(m).faithful,
            "det<0 must route to the non-TRS fallback regardless of recompose closeness"
        );
    }

    #[test]
    fn decompose_matches_euler_xyz_convention() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::ONE,
            Quat::from_euler(EulerRot::XYZ, 0.1, 0.2, 0.3),
            Vec3::ZERO,
        );
        let fields = decompose_trs(m);
        assert!((fields.rotation_deg.x - 0.1f32.to_degrees()).abs() < 1e-2);
        assert!((fields.rotation_deg.y - 0.2f32.to_degrees()).abs() < 1e-2);
        assert!((fields.rotation_deg.z - 0.3f32.to_degrees()).abs() < 1e-2);
    }
}
