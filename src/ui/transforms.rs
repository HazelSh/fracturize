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

use glam::{EulerRot, Mat4, Quat, Vec3, Vec4};

use crate::app::App;
use crate::scene::{NUM_VARIATIONS, VARIATION_NAMES};

use super::hints::hinted;
use super::icons;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.transforms_open;
    if !open {
        return;
    }

    egui::Window::new("Transforms")
        .id(egui::Id::new("fracturize_transforms_window"))
        .open(&mut open)
        .default_pos(egui::pos2(260.0, 60.0))
        .default_width(300.0)
        .show(ctx, |ui| {
            draw_list(ui, app);
            ui.separator();
            draw_inspector(ui, app);
        });

    app.ui_state.panels.transforms_open = open;
}

// === List ===

/// Snapshot of one row's display data, taken before the row loop so the loop
/// body is free to call `&mut App` methods without fighting the borrow
/// checker over `app.scene`.
struct RowData {
    name: String,
    color: Vec3,
    weight: f32,
    enabled: bool,
    /// Hover summary (position/scale/variations) — the info the retired HUD
    /// transform list used to show per row.
    summary: String,
}

fn draw_list(ui: &mut egui::Ui, app: &mut App) {
    let n = app.scene.transforms.len();
    ui.label(egui::RichText::new(format!("{} transform{}", n, if n == 1 { "" } else { "s" })).strong());

    let rows: Vec<RowData> = (0..n)
        .map(|i| {
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
        })
        .collect();

    let selected = app.selected_transform();

    egui::ScrollArea::vertical()
        .max_height(180.0)
        .show(ui, |ui| {
            for (i, row) in rows.into_iter().enumerate() {
                draw_row(ui, app, i, row, selected == Some(i));
            }
        });

    ui.horizontal(|ui| {
        let resp = ui.button("+ Add");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Add a fresh transform (Shift+A)",
            "click: add a new transform",
        );
        if resp.clicked() {
            app.add_transform(true);
        }

        let resp = ui.add_enabled(selected.is_some(), egui::Button::new("Duplicate"));
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

        let resp = ui.add_enabled(selected.is_some() && n > 1, egui::Button::new("Delete"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Delete the selected transform (Del)",
            "click: delete selected transform",
        );
        if resp.clicked() {
            if let Some(i) = selected {
                app.delete_transform_at(i);
            }
        }
    });
}

fn color32_from_vec3(c: Vec3) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c.x.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.y.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.z.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// One list row. The whole row is the select target — built as a
/// `scope_builder` `Ui` that senses clicks, because egui registers a
/// container's own sense *below* its children's: the eye button and the
/// weight DragValue inside still get their clicks first, and only the
/// leftover row area selects. (A plain `ui.interact` over the finished rect
/// registers last and would swallow both.)
fn draw_row(ui: &mut egui::Ui, app: &mut App, i: usize, row: RowData, selected: bool) {
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

    let row_resp = ui
        .scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
            ui.set_width(ui.available_width());

            // Reserve the background shapes now (so they paint *under* the
            // row's widgets) but size them after the content is laid out —
            // `max_rect()` here is every remaining point in the window, not
            // this row, so filling it directly bleeds the selection colour
            // down over everything below.
            let bg_idx = ui.painter().add(egui::Shape::Noop);
            let accent_idx = ui.painter().add(egui::Shape::Noop);

            ui.horizontal(|ui| {
                ui.add_space(6.0);
                let (swatch, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().rect_filled(swatch, 2.0, color32_from_vec3(row.color));

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

                if is_renaming {
                    let resp = {
                        let (_, buf) = app.ui_state.renaming_transform.as_mut().unwrap();
                        ui.add(egui::TextEdit::singleline(buf).desired_width(96.0))
                    };
                    resp.request_focus();
                    let enter = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                    let escape = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                    if escape {
                        app.ui_state.renaming_transform = None;
                    } else if enter || resp.lost_focus() {
                        if let Some((idx, name)) = app.ui_state.renaming_transform.take() {
                            app.rename_transform(idx, if name.trim().is_empty() { None } else { Some(name) });
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
                    ui.add(egui::Label::new(text).selectable(false));
                }

                // Weight sits at the trailing edge so the rows line up as a
                // column regardless of name length.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    let mut w = row.weight;
                    let speed = (w.abs().max(0.01) * 0.05) as f64;
                    let resp = ui.add(
                        egui::DragValue::new(&mut w)
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
                        app.set_transform_weight(i, w);
                    }
                });
            });

            // Selection reads as a filled row with an accent bar down its
            // leading edge, hover as a dimmer fill — the same visual grammar
            // as a list selection anywhere else, so the list is legible as
            // "one of these is active" rather than "these are buttons".
            let rect = ui.min_rect().expand2(egui::vec2(0.0, 2.0));
            let visuals = ui.visuals();
            if selected {
                ui.painter()
                    .set(bg_idx, egui::Shape::rect_filled(rect, 3.0, visuals.selection.bg_fill));
                ui.painter().set(
                    accent_idx,
                    egui::Shape::rect_filled(
                        egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                        1.0,
                        visuals.selection.stroke.color,
                    ),
                );
            } else if ui.response().hovered() {
                ui.painter().set(
                    bg_idx,
                    egui::Shape::rect_filled(rect, 3.0, visuals.widgets.hovered.bg_fill),
                );
            }
        })
        .response;

    if is_renaming {
        return;
    }

    let row_resp = hinted(
        row_resp,
        &mut app.ui_state,
        row.summary.as_str(),
        "click: select this transform · right-click: menu",
    );
    if row_resp.clicked() {
        app.select_transform(Some(i));
    }
    row_resp.context_menu(|ui| {
        if ui.button("Duplicate").clicked() {
            app.duplicate_transform_at(i);
            ui.close();
        }
        if ui.button(if row.enabled { "Disable" } else { "Enable" }).clicked() {
            app.toggle_transform_enabled(i);
            ui.close();
        }
        let can_delete = app.scene.transforms.len() > 1;
        if ui.add_enabled(can_delete, egui::Button::new("Delete")).clicked() {
            app.delete_transform_at(i);
            ui.close();
        }
        if ui.button("Rename").clicked() {
            app.ui_state.renaming_transform = Some((i, row.name.clone()));
            ui.close();
        }
    });
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
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    let (rx, ry, rz) = rotation.to_euler(EulerRot::XYZ);

    let recomposed = Mat4::from_scale_rotation_translation(scale, rotation, translation);
    let diff = (matrix - recomposed).to_cols_array();
    let max_diff = diff.iter().fold(0.0f32, |acc, v| acc.max(v.abs()));
    let norm = matrix
        .to_cols_array()
        .iter()
        .map(|v| v * v)
        .sum::<f32>()
        .sqrt()
        .max(1e-6);

    let faithful = max_diff < 1e-4 * norm && matrix.determinant() >= 0.0;

    TrsFields {
        position: translation,
        rotation_deg: Vec3::new(rx.to_degrees(), ry.to_degrees(), rz.to_degrees()),
        scale,
        faithful,
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
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().rect_filled(swatch, 2.0, color32_from_vec3(color));
        ui.label(egui::RichText::new(&name).strong());
        ui.label(egui::RichText::new(format!("(T{})", idx)).weak().small());
    });
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
    let rotation = Quat::from_euler(
        EulerRot::XYZ,
        cache.rotation_deg[0].to_radians(),
        cache.rotation_deg[1].to_radians(),
        cache.rotation_deg[2].to_radians(),
    );
    let matrix = Mat4::from_scale_rotation_translation(
        Vec3::from(cache.scale),
        rotation,
        Vec3::from(cache.position),
    );
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

    let resp = ui.button("Orthogonalize → TRS");
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
