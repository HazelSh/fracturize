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
/// Height of one tab, including its weight bar.
const TAB_HEIGHT: f32 = 30.0;

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

fn draw_rail(ui: &mut egui::Ui, app: &mut App) {
    let n = app.scene.transforms.len();
    let selected = app.selected_transform();
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
                // Virtualized: L-system scenes reach tens of thousands of
                // transforms, and laying every tab out per frame would cost
                // more than the whole rest of the UI.
                egui::ScrollArea::vertical()
                    .id_salt("fracturize_transform_rail")
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show_rows(ui, TAB_HEIGHT, n, |ui, range| {
                        for i in range {
                            let row = row_data(app, i);
                            draw_tab(ui, app, i, row, selected == Some(i), max_weight);
                        }
                    });

                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    let resp = ui.small_button("+ add");
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Add a fresh transform (Shift+A)",
                        "click: add a new transform",
                    );
                    if resp.clicked() {
                        app.add_transform(true);
                    }

                    let resp = ui.add_enabled(selected.is_some(), egui::Button::new("dup").small());
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
                });
            });
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
                            "Disable this transform (Enter)"
                        } else {
                            "Enable this transform (Enter)"
                        },
                        "click: toggle enabled",
                    );
                    if eye_resp.clicked() {
                        app.toggle_transform_enabled(i);
                    }
                });
            });

            let mut rect = ui.min_rect();
            rect.max.y = rect.min.y + TAB_HEIGHT - 4.0;
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
            let frac = (row.weight / max_weight).clamp(0.0, 1.0);
            let bar_w = (rect.width() - 16.0) * frac;
            if bar_w > 0.5 {
                let bar = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + 8.0, rect.max.y - 3.0),
                    egui::vec2(bar_w, 2.0),
                );
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
        "click: select this transform · right-click: menu",
    );
    if tab_resp.clicked() {
        app.select_transform(Some(i));
    }
    tab_resp.context_menu(|ui| {
        if context_menu(ui, app, i) {
            ui.close();
        }
    });
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
struct ZoomAction {
    /// Already the scene's zoom map, so clicking clears it
    is_zoom: bool,
    /// The map renormalizes, or is already the one in use
    enabled: bool,
    tooltip: String,
    hint: &'static str,
}

/// The button's label. Constant, because "this is the one" is carried by
/// `Button::selected` rather than a tick in the text: the UI font has no
/// U+2713, so a "✓" prefix rendered as a missing-glyph box — and a highlight
/// says *selected* more directly than a character does anyway.
const ZOOM_LABEL: &str = "Zoom about this";

fn zoom_action(app: &App, i: usize) -> ZoomAction {
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

        let resp = ui.add(egui::Button::new("Rename").small());
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Rename this transform. The field opens on its tab in the rail, where \
             the name lives.",
            "click: rename this transform",
        );
        if resp.clicked() {
            let name = app
                .scene
                .transform_names
                .get(idx)
                .and_then(|n| n.clone())
                .unwrap_or_default();
            app.ui_state.renaming_transform = Some((idx, name));
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
    /// Set after drawing this frame's fields; read at the *start* of next
    /// frame to decide whether a generation bump should force a refresh.
    editing: bool,
}

impl TrsCache {
    fn from_matrix(idx: usize, generation: u64, matrix: Mat4) -> Self {
        let fields = decompose_trs(matrix);
        let uniform_scale_linked = (fields.scale.x - fields.scale.y).abs() < 1e-4
            && (fields.scale.x - fields.scale.z).abs() < 1e-4;
        Self {
            key: (idx, generation),
            position: fields.position.to_array(),
            rotation_deg: fields.rotation_deg.to_array(),
            scale: fields.scale.to_array(),
            uniform_scale_linked,
            faithful: fields.faithful,
            matrix_cols: matrix.to_cols_array_2d(),
            editing: false,
        }
    }
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
    // is the detail view for whatever it has selected.
    let name = app
        .scene
        .transform_names
        .get(idx)
        .and_then(|n| n.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("T{}", idx));
    let color = app.scene.colors.get(idx).copied().unwrap_or(Vec3::ONE);
    let n = app.scene.transforms.len();
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().rect_filled(swatch, 2.0, color32_from_vec3(color));
        ui.label(egui::RichText::new(&name).strong());
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
        app.ui_state.trs_cache = Some(TrsCache::from_matrix(idx, key.1, matrix));
    }

    // Pull the cache out of `UiState` for the duration of the draw calls so
    // both `&mut App` (for committing edits) and `&mut TrsCache` (for the
    // DragValue targets) can be held at once.
    let mut cache = app.ui_state.trs_cache.take().unwrap();
    let mut editing = false;

    if cache.faithful {
        draw_trs_fields(ui, app, idx, &mut cache, &mut editing);
    } else {
        draw_matrix_grid(ui, app, idx, &mut cache, &mut editing);
    }

    cache.editing = editing;
    app.ui_state.trs_cache = Some(cache);

    ui.separator();
    draw_weight_color(ui, app, idx);
    ui.separator();
    draw_variations(ui, app, idx);
}

fn drag_row(
    ui: &mut egui::Ui,
    app: &mut App,
    label: &str,
    fields: &mut [f32; 3],
    speed: f64,
    suffix: &str,
    tooltip: &str,
    hint: &str,
    editing: &mut bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for (axis, v) in ["x", "y", "z"].iter().zip(fields.iter_mut()) {
            let resp = ui.add(
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

fn draw_trs_fields(ui: &mut egui::Ui, app: &mut App, idx: usize, cache: &mut TrsCache, editing: &mut bool) {
    let mut changed_field: Option<&'static str> = None;

    if drag_row(
        ui,
        app,
        "Position",
        &mut cache.position,
        0.01,
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
                egui::RichText::new(format!("×{:.3}", contraction)).weak().small(),
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
            let resp = ui.add(
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

fn draw_weight_color(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    let mut weight = app.scene.transforms[idx].weight;
    ui.horizontal(|ui| {
        ui.label("Weight");
        let speed = (weight.abs().max(0.01) * 0.05) as f64;
        let resp = ui.add(
            egui::DragValue::new(&mut weight)
                .speed(speed)
                .range(0.01..=100.0),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Chaos-game selection weight (, / .)",
            "drag: adjust weight · click: type exact value",
        );
        if resp.changed() {
            app.set_transform_weight(idx, weight);
        }
    });

    // In palette mode the per-transform RGB is not rendered at all — the
    // gradient owns the colours — so showing a swatch there would be a
    // control that does nothing. What matters instead is *where in the
    // gradient* this transform sits, drawn on the gradient itself.
    if app.scene.color_mode == crate::scene::ColorMode::Palette {
        ui.label("Color value");
        super::gradient::transform_color_value(ui, app, idx);
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

    let mut name = app
        .scene
        .transform_names
        .get(idx)
        .and_then(|n| n.clone())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Name");
        let resp = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Label shown in the list and the gizmo overlay",
            "type: rename this transform",
        );
        if resp.changed() {
            app.rename_transform(idx, if name.trim().is_empty() { None } else { Some(name.clone()) });
        }
    });
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
                let resp = ui.add(egui::DragValue::new(&mut v).speed(0.05).range(-4.0..=4.0));
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
