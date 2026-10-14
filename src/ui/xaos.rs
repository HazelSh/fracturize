//! Xaos in the panels: which map may follow which (see `crate::xaos`).
//!
//! Three views of one graph, every edit through the same `App` methods
//! (`set_xaos_edge`, `set_xaos_rest`, …) so they are one edit wherever made:
//!
//! * the **inspector block** ([`draw_block`]) — one map's row, or its column;
//! * the **matrix window** ([`draw_window`]) — a layer's whole graph;
//! * the **viewport overlay** ([`draw_overlay`]) — the selected map's edges,
//!   drawn between gizmo origins.
//!
//! Two rules from the plan shape all three. **A cut is drawn differently from
//! a scale**, never as a darker shade of one, because only zeros change the
//! shape: a cut is a different glyph (struck through, ×, empty). And **walk
//! share is shown beside weight** wherever xaos is on, because under xaos a
//! weight no longer says how often a map fires.

use egui::Color32;

use super::hints::hinted;
use super::icons;
use crate::app::App;

/// Past this many maps in a layer the block lists only a row's exceptions,
/// with a picker to add one, rather than a row per map.
const FILTER_THRESHOLD: usize = 12;

fn color32(c: glam::Vec3) -> Color32 {
    let s = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
    Color32::from_rgb(s(c.x), s(c.y), s(c.z))
}

fn name_of(app: &App, i: usize) -> String {
    app.scene.transform_names.get(i).cloned().flatten().unwrap_or_else(|| format!("T{i}"))
}

/// The one-line summary of a row: what a reader wants before the detail.
fn summary(app: &App, idx: usize, sum: &crate::app::XaosSummary) -> String {
    let Some(i) = sum.pos(idx) else { return String::new() };
    let n = sum.idx.len();
    let exits: Vec<usize> = (0..n).filter(|&j| sum.chain.allowed(i, j)).collect();
    if sum.chain.dead.contains(&i) {
        return "goes nowhere — restarts from the plain weights".into();
    }
    match exits.len() {
        k if k == n => "any map may follow".into(),
        1 => format!("only \u{E06C} {}", name_of(app, sum.idx[exits[0]])),
        k => format!("{} of {} exits cut", n - k, n),
    }
}

/// The Xaos block in the Transforms inspector, under Behaviour.
///
/// Opt-in by disclosure rather than by a mode: until a row is authored in the
/// layer it is one closed line; once one is, it opens by default — the state
/// is keyed by whether the layer has xaos, so the default applies afresh when
/// that changes.
pub fn draw_block(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    let layer = app.scene.transforms[idx].layer as usize;
    let Some(sum) = app.xaos_summary(layer) else { return };
    let Some(me) = sum.pos(idx) else { return };
    let n = sum.idx.len();

    let header = if sum.has_xaos {
        format!("Xaos — {}", summary(app, idx, &sum))
    } else {
        "Xaos — any map may follow".to_string()
    };
    let resp = egui::CollapsingHeader::new(egui::RichText::new(header).small())
        .id_salt(("xaos_block", sum.has_xaos))
        .default_open(sum.has_xaos)
        .show(ui, |ui| draw_block_body(ui, app, idx, me, &sum));
    hinted(
        resp.header_response,
        &mut app.ui_state,
        "Xaos: which maps may follow this one in the chaos game, and how likely each is. \
         Cutting an edge removes material; scaling one only moves density.",
        "click: show or hide which maps may follow this one",
    );
    let _ = n;
}

fn draw_block_body(ui: &mut egui::Ui, app: &mut App, idx: usize, me: usize, sum: &crate::app::XaosSummary) {
    let n = sum.idx.len();

    // Where the walk actually puts this map, against what its weight asks.
    let weight_share = sum.chain.start[me] * 100.0;
    let walk_share = sum.shares[me] * 100.0;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("fires").small());
        super::num::small_cell(ui, &format!("{}%", super::num::fixed(walk_share as f32, 1)), 6);
        ui.label(egui::RichText::new("of the walk; its weight asks").small());
        super::num::small_cell(ui, &format!("{}%", super::num::fixed(weight_share as f32, 1)), 6);
    });

    let from_view = app.ui_state.xaos_from;
    let chosen = super::radio::radio(&mut app.ui_state, "xaos_from", from_view)
        .option(
            false,
            "goes to",
            "The maps that may follow this one: this map's own row.",
            "click: list the maps that may follow this one",
        )
        .option(
            true,
            "comes from",
            "The maps this one may follow: an entry in each of their rows. Editing here edits \
             theirs — the natural place for \"only reachable from …\".",
            "click: list the maps this one may follow",
        )
        .show(ui);
    if let Some(v) = chosen {
        app.ui_state.xaos_from = v;
    }

    // The targets to list: every map of the layer, or past a dozen only the
    // ones that differ from the rest, plus the filter's matches.
    let filter = app.ui_state.xaos_filter.to_lowercase();
    let listed: Vec<usize> = if n <= FILTER_THRESHOLD {
        (0..n).collect()
    } else {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("find").small());
            let r = ui.add(egui::TextEdit::singleline(&mut app.ui_state.xaos_filter).desired_width(110.0));
            hinted(
                r,
                &mut app.ui_state,
                "Type part of a name to list that map here, so you can set its edge.",
                "type: find a map to set an edge to",
            );
        });
        (0..n)
            .filter(|&j| {
                let (f, t) = if from_view { (j, me) } else { (me, j) };
                let row = &app.scene.transforms[sum.idx[f]].xaos;
                row.get(t) != row.rest
                    || (!filter.is_empty() && name_of(app, sum.idx[j]).to_lowercase().contains(&filter))
            })
            .take(200)
            .collect()
    };

    egui::Grid::new(("xaos_rows", idx, from_view)).num_columns(4).spacing([6.0, 2.0]).show(ui, |ui| {
        for &j in &listed {
            let target = sum.idx[j];
            let (f, t) = if from_view { (j, me) } else { (me, j) };
            let (from_idx, to_idx) = (sum.idx[f], sum.idx[t]);
            let x = app.scene.transforms[from_idx].xaos.get(t);
            let p = sum.chain.prob(f, t) * 100.0;
            let cut = x == 0.0;

            ui.horizontal(|ui| {
                let (sw, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                let col = app.scene.colors.get(target).copied().unwrap_or(glam::Vec3::ONE);
                ui.painter().rect_filled(sw, 2.0, color32(col));
                let mut text = egui::RichText::new(name_of(app, target)).small();
                if cut {
                    text = text.strikethrough().color(ui.visuals().weak_text_color());
                }
                ui.add(egui::Label::new(text).truncate());
            });

            let mut v = x;
            let speed = (v.max(0.05) * 0.02) as f64;
            let r = super::num::drag(
                ui,
                5,
                2,
                egui::DragValue::new(&mut v).speed(speed).range(0.0..=100.0).prefix("×"),
            );
            let (a, b) = (name_of(app, from_idx), name_of(app, to_idx));
            let r = hinted(
                r,
                &mut app.ui_state,
                format!(
                    "{a} \u{E06C} {b}: the odds of {b} straight after {a}, relative to its weight. \
                     1 is as usual, 0 never, 3 three times as likely."
                ),
                "drag: scale this edge · click: type an exact value",
            );
            if r.changed() {
                app.set_xaos_edge(from_idx, to_idx, v);
            }

            let readout = if p == 0.0 { "never".to_string() } else { format!("{}%", super::num::fixed(p as f32, 1)) };
            let r = super::num::small_cell(ui, &readout, 6);
            hinted(
                r,
                &mut app.ui_state,
                format!("The chance {b} follows {a}, with every edge as it stands."),
                "the chance this map follows",
            );

            let (icon, tip, hint) = if cut {
                (icons::RESET, format!("Restore {a} \u{E06C} {b} to 1."), "click: restore this edge")
            } else {
                (
                    icons::SCISSORS,
                    format!("Cut {a} \u{E06C} {b}: {b} never follows {a}. This removes material, where a scale only moves density."),
                    "click: cut this edge",
                )
            };
            let r = ui.add(egui::Button::new(icon).small().frame(false));
            if hinted(r, &mut app.ui_state, tip, hint).clicked() {
                app.set_xaos_edge(from_idx, to_idx, if cut { 1.0 } else { 0.0 });
            }
            ui.end_row();
        }

        // The rest of the row: what every unlisted map — and every map added
        // later — gets. Only in the "goes to" view: a column has no rest.
        if !from_view {
            ui.label(egui::RichText::new("every other").small().italics());
            let mut c = app.scene.transforms[idx].xaos.rest;
            let speed = (c.max(0.05) * 0.02) as f64;
            let r = super::num::drag(
                ui,
                5,
                2,
                egui::DragValue::new(&mut c).speed(speed).range(0.0..=100.0).prefix("×"),
            );
            let r = hinted(
                r,
                &mut app.ui_state,
                "The multiplier for every map this row doesn't list — including maps added later. \
                 0 makes the row \"only these\".",
                "drag: the rest of the row · click: type",
            );
            if r.changed() {
                app.set_xaos_rest(idx, c);
            }
            ui.label("");
            ui.label("");
            ui.end_row();
        }
    });

    ui.horizontal_wrapped(|ui| {
        let r = ui.add(egui::Button::new((icons::RESET, "Reset")).small());
        if hinted(r, &mut app.ui_state, "Clear this map's row: any map may follow it again.", "click: reset this row")
            .clicked()
        {
            app.reset_xaos_row(idx);
        }
        let r = ui.add(egui::Button::new((icons::SCISSORS, "No self-loop")).small());
        if hinted(
            r,
            &mut app.ui_state,
            "This map may not follow itself. The classic xaos move: dramatic on spirals and dense self-copies.",
            "click: cut this map's edge to itself",
        )
        .clicked()
        {
            app.set_xaos_edge(idx, idx, 0.0);
        }
        let only = ui.menu_button(egui::RichText::new("Only to…").small(), |ui| {
            for j in 0..n.min(200) {
                let target = sum.idx[j];
                let label = name_of(app, target);
                if ui.button(label).clicked() {
                    app.set_xaos_probability(idx, target, 1.0);
                    ui.close();
                }
            }
        });
        hinted(
            only.response,
            &mut app.ui_state,
            "Make one map the only one that may follow this one.",
            "click: choose the only map that may follow",
        );
        let r = ui.add(egui::Button::new((icons::LINK, "Linked map")).small());
        if hinted(
            r,
            &mut app.ui_state,
            "Insert a new map that runs straight after this one and only there (Apophysis's \
             linked transform). It starts as the identity, so nothing changes until you edit it — \
             then it acts on this map's copies alone.",
            "click: insert a map that runs only after this one",
        )
        .clicked()
        {
            app.insert_linked_transform(idx);
        }
    });

    if let Some(e) = app.zoom_error.as_deref().filter(|e| e.contains("xaos")) {
        ui.label(egui::RichText::new(e).small().color(ui.visuals().warn_fg_color));
    }
}

// ---------------------------------------------------------------------------
// The matrix window: one layer's whole graph.
// ---------------------------------------------------------------------------

/// Pitch of one matrix cell, in points. Fixed, because it is what the grid's
/// virtualization is told — the rail's `TAB_HEIGHT` lesson.
const CELL: f32 = 18.0;
/// Width of the row-label column.
const LABEL_W: f32 = 96.0;
/// Height of the column-header row, and of the walk-share strip under it.
const HEAD_H: f32 = 14.0;
const STRIP_H: f32 = 22.0;
/// Past this many maps the grid shows only rows and columns that carry an
/// exception (plus the selection): the full matrix of a 40k-map layer is not a
/// thing anyone looks at.
const SPARSE_AT: usize = 300;

/// What a cell shows, as a fill: neutral at 1, cooler below, warmer above, on
/// a log scale. A cut is not a colour at all — it is drawn as an ×, because
/// only zeros change the shape and the two must not read as degrees of one
/// thing.
fn cell_fill(ui: &egui::Ui, x: f32) -> Color32 {
    let neutral = ui.visuals().widgets.inactive.bg_fill;
    if x == 1.0 || x <= 0.0 {
        return neutral;
    }
    let t = (x.ln() / 3f32.ln()).clamp(-1.0, 1.0); // ×3 or ×1/3 is full
    let target = if t > 0.0 { Color32::from_rgb(214, 120, 52) } else { Color32::from_rgb(64, 120, 200) };
    lerp_color(neutral, target, t.abs())
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn draw_window(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.xaos_open;
    if !open {
        return;
    }
    super::window(ctx, app, super::WindowKey::Xaos, "Xaos")
        .open(&mut open)
        .show(ctx, |ui| draw_window_body(ui, app));
    super::remember(ctx, app, super::WindowKey::Xaos);
    app.ui_state.panels.xaos_open = open;
}

fn draw_window_body(ui: &mut egui::Ui, app: &mut App) {
    let layer = app.current_layer();
    let Some(sum) = app.xaos_summary(layer) else {
        ui.label(egui::RichText::new("No map in this layer has any weight, so there is no walk to show.").weak());
        return;
    };
    let n = sum.idx.len();
    if app.scene.is_composite() {
        ui.label(
            egui::RichText::new(format!("{} — choose the layer in the Transforms window", app.scene.layer_name(layer)))
                .small(),
        );
    }

    // Presets: operations that write rows, each one undoable edit.
    ui.horizontal_wrapped(|ui| {
        let presets: [(crate::xaos::Preset, &str, &str); 4] = [
            (crate::xaos::Preset::NoSelfLoops, "No self-loops", "No map may follow itself."),
            (crate::xaos::Preset::Cycle, "Cycle", "Each map may only be followed by the next one in the list, wrapping round. Forces ordered sequences; often architectural."),
            (crate::xaos::Preset::Blocks(2), "Two blocks", "Split the list in half, each half closed to the other: two attractors sharing the walkers."),
            (crate::xaos::Preset::Reset, "Reset all", "Clear every row: any map may follow any other again."),
        ];
        for (preset, label, tip) in presets {
            let r = ui.add(egui::Button::new(egui::RichText::new(label).small()));
            if hinted(r, &mut app.ui_state, format!("{tip} Rewrites every row in this layer; one Ctrl+Z undoes it."), "click: rewrite every row from this preset")
                .clicked()
            {
                app.apply_xaos_preset(layer, preset);
            }
        }
    });

    let show_p = app.ui_state.xaos_show_p;
    let chosen = super::radio::radio(&mut app.ui_state, "xaos_show_p", show_p)
        .option(false, "multiplier", "Each cell is the multiplier as written: 1 is as usual, 0 a cut.", "click: show the multipliers")
        .option(true, "probability", "Each cell is the chance its column's map follows its row's map, with every edge as it stands.", "click: show the probabilities")
        .show(ui);
    if let Some(v) = chosen {
        app.ui_state.xaos_show_p = v;
    }

    // Which rows and columns to draw: all of them, or past SPARSE_AT only the
    // ones that say something.
    let selected = app.selected_transform().and_then(|i| sum.pos(i));
    let shown: Vec<usize> = if n <= SPARSE_AT {
        (0..n).collect()
    } else {
        let mut keep = vec![false; n];
        for (p, &i) in sum.idx.iter().enumerate() {
            let row = &app.scene.transforms[i].xaos;
            if !row.is_default() {
                keep[p] = true;
            }
            for e in &row.exceptions {
                if let Some(k) = keep.get_mut(e.0 as usize) {
                    *k = true;
                }
            }
        }
        if let Some(s) = selected {
            keep[s] = true;
        }
        let v: Vec<usize> = (0..n).filter(|&p| keep[p]).take(SPARSE_AT).collect();
        ui.label(
            egui::RichText::new(format!(
                "{n} maps: showing the {} with a row or an edge that isn't 1, and the selected one",
                v.len()
            ))
            .small()
            .weak(),
        );
        v
    };
    let m = shown.len();
    let max_share = shown.iter().map(|&p| sum.shares[p]).fold(0.0f64, f64::max).max(1e-12);

    let content = egui::vec2(LABEL_W + m as f32 * CELL + 4.0, HEAD_H + m as f32 * CELL + STRIP_H + 6.0);
    let mut hovered_cell: Option<(usize, usize)> = None;
    egui::ScrollArea::both().id_salt("fracturize_xaos_grid").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        let (outer, _) = ui.allocate_exact_size(content, egui::Sense::hover());
        let origin = outer.min;
        let visible = viewport.translate(origin.to_vec2());
        let painter = ui.painter_at(outer);
        let grid_min = origin + egui::vec2(LABEL_W, HEAD_H);
        let cell_rect = |r: usize, c: usize| {
            egui::Rect::from_min_size(grid_min + egui::vec2(c as f32 * CELL, r as f32 * CELL), egui::vec2(CELL - 1.0, CELL - 1.0))
        };
        let first = |lo: f32, base: f32| (((lo - base) / CELL).floor().max(0.0)) as usize;
        let (c0, c1) = (first(visible.min.x, grid_min.x), (first(visible.max.x, grid_min.x) + 1).min(m));
        let (r0, r1) = (first(visible.min.y, grid_min.y), (first(visible.max.y, grid_min.y) + 1).min(m));

        // Column headers: a swatch each, the selected one ringed.
        for c in c0..c1 {
            let i = sum.idx[shown[c]];
            let nm = name_of(app, i);
            let rect = egui::Rect::from_min_size(origin + egui::vec2(LABEL_W + c as f32 * CELL + 3.0, 1.0), egui::vec2(CELL - 7.0, HEAD_H - 3.0));
            let col = app.scene.colors.get(i).copied().unwrap_or(glam::Vec3::ONE);
            painter.rect_filled(rect, 2.0, color32(col));
            if selected == Some(shown[c]) {
                painter.rect_stroke(rect.expand(1.5), 2.0, egui::Stroke::new(1.5, ui.visuals().strong_text_color()), egui::StrokeKind::Outside);
            }
            let resp = ui.interact(rect, ui.id().with(("xaos_col", i)), egui::Sense::click());
            let resp = hinted(resp, &mut app.ui_state, format!("To {nm}. Click to select it; right-click for column actions."), "click: select this map · right-click: column actions");
            if resp.clicked() {
                app.select_transform(Some(i));
            }
            resp.context_menu(|ui| column_menu(ui, app, &sum, shown[c]));
        }

        // Row labels: swatch and name.
        for r in r0..r1 {
            let i = sum.idx[shown[r]];
            let nm = name_of(app, i);
            let rect = egui::Rect::from_min_size(origin + egui::vec2(0.0, HEAD_H + r as f32 * CELL), egui::vec2(LABEL_W - 4.0, CELL - 1.0));
            let col = app.scene.colors.get(i).copied().unwrap_or(glam::Vec3::ONE);
            painter.rect_filled(egui::Rect::from_min_size(rect.min + egui::vec2(2.0, 4.0), egui::vec2(9.0, 9.0)), 2.0, color32(col));
            let text_col = if selected == Some(shown[r]) { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
            let galley = painter.layout(nm.clone(), egui::FontId::proportional(11.0), text_col, LABEL_W - 20.0);
            painter.galley(rect.min + egui::vec2(15.0, 2.0), galley, text_col);
            let resp = ui.interact(rect, ui.id().with(("xaos_row", i)), egui::Sense::click());
            let resp = hinted(resp, &mut app.ui_state, format!("From {nm}. Click to select it; right-click for row actions."), "click: select this map · right-click: row actions");
            if resp.clicked() {
                app.select_transform(Some(i));
            }
            resp.context_menu(|ui| row_menu(ui, app, &sum, shown[r]));
        }

        // The cells.
        let paint = app.ui_state.xaos_paint.clone();
        for r in r0..r1 {
            for c in c0..c1 {
                let (f, t) = (shown[r], shown[c]);
                let (fi, ti) = (sum.idx[f], sum.idx[t]);
                let rect = cell_rect(r, c);
                let x = app.scene.transforms[fi].xaos.get(t);
                let p = sum.chain.prob(f, t);
                let pending = paint.as_ref().filter(|(_, cells)| cells.contains(&(fi, ti))).map(|(v, _)| *v);
                let shown_x = pending.unwrap_or(x);
                if show_p && pending.is_none() {
                    let row_max = (0..n).map(|j| sum.chain.prob(f, j)).fold(0.0, f64::max).max(1e-12);
                    let fill = lerp_color(ui.visuals().widgets.inactive.bg_fill, Color32::from_rgb(214, 160, 70), (p / row_max) as f32);
                    painter.rect_filled(rect, 1.0, fill);
                } else {
                    painter.rect_filled(rect, 1.0, cell_fill(ui, shown_x));
                }
                if shown_x == 0.0 || (show_p && p == 0.0 && pending.is_none()) {
                    let s = egui::Stroke::new(1.5, ui.visuals().text_color());
                    let k = rect.shrink(4.5);
                    painter.line_segment([k.left_top(), k.right_bottom()], s);
                    painter.line_segment([k.right_top(), k.left_bottom()], s);
                }
                if f == t {
                    painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Inside);
                }
                if pending.is_some() {
                    painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.5, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
                }

                let resp = ui.interact(rect, ui.id().with(("xaos_cell", fi, ti)), egui::Sense::click_and_drag());
                if resp.hovered() || resp.dragged() {
                    hovered_cell = Some((f, t));
                }
                let (a, b) = (name_of(app, fi), name_of(app, ti));
                let row_total_p = p * 100.0;
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    format!(
                        "{a} \u{E06C} {b}: ×{}, {}% of {a}'s exits.\nClick: cut or restore. Drag up or down: scale. Alt+scroll: scale. Shift+drag across cells: paint cuts (or restores) in one stroke.",
                        super::num::fixed(x, 2),
                        super::num::fixed(row_total_p as f32, 1)
                    ),
                    "click: cut/restore · drag: scale · shift+drag: paint",
                );
                let shift = ui.input(|i| i.modifiers.shift);
                if resp.drag_started() && shift {
                    app.ui_state.xaos_paint = Some((if x == 0.0 { 1.0 } else { 0.0 }, vec![(fi, ti)]));
                } else if resp.dragged() && app.ui_state.xaos_paint.is_none() {
                    let dy = resp.drag_delta().y;
                    if dy != 0.0 {
                        let base = if x == 0.0 { 0.05 } else { x };
                        app.set_xaos_edge(fi, ti, (base * 1.03f32.powf(-dy)).clamp(0.0, 100.0));
                    }
                } else if resp.clicked() {
                    app.set_xaos_edge(fi, ti, if x == 0.0 { 1.0 } else { 0.0 });
                }
                if resp.hovered() {
                    let (scroll, alt) = ui.input(|i| {
                        let notches: f32 = i
                            .raw
                            .events
                            .iter()
                            .filter_map(|e| match e {
                                egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                                _ => None,
                            })
                            .sum();
                        (notches, i.modifiers.alt)
                    });
                    if alt && scroll != 0.0 {
                        let base = if x == 0.0 { 0.05 } else { x };
                        app.set_xaos_edge(fi, ti, (base * 1.1f32.powf(scroll.signum())).clamp(0.0, 100.0));
                    }
                }
            }
        }

        // A paint stroke collects the cells the pointer crosses, and lands as
        // one edit when the button comes up.
        if let Some((v, mut cells)) = app.ui_state.xaos_paint.take() {
            let down = ui.input(|i| i.pointer.primary_down());
            if let Some((f, t)) = hovered_cell {
                let pair = (sum.idx[f], sum.idx[t]);
                if !cells.contains(&pair) {
                    cells.push(pair);
                }
            }
            if down {
                app.ui_state.xaos_paint = Some((v, cells));
            } else {
                let edges: Vec<(usize, usize, f32)> = cells.iter().map(|&(f, t)| (f, t, v)).collect();
                let label = format!("Xaos: {} {} edge{}", if v == 0.0 { "cut" } else { "restore" }, edges.len(), if edges.len() == 1 { "" } else { "s" });
                app.set_xaos_edges(&edges, Some(label));
            }
        }

        // The walk-share strip: how much of the walk each column's map gets.
        let strip_top = grid_min.y + m as f32 * CELL + 4.0;
        for c in c0..c1 {
            let p = shown[c];
            let i = sum.idx[p];
            let nm = name_of(app, i);
            let frac = (sum.shares[p] / max_share) as f32;
            let full = egui::Rect::from_min_size(egui::pos2(grid_min.x + c as f32 * CELL + 2.0, strip_top), egui::vec2(CELL - 5.0, STRIP_H - 4.0));
            painter.rect_filled(full, 1.0, ui.visuals().extreme_bg_color);
            let drained = sum.chain.start[p] > 0.0 && sum.shares[p] <= 1e-7;
            let mut bar = full;
            bar.min.y = full.max.y - full.height() * frac;
            let col = app.scene.colors.get(i).copied().unwrap_or(glam::Vec3::ONE);
            painter.rect_filled(bar, 1.0, color32(col));
            if drained {
                painter.rect_stroke(full, 1.0, egui::Stroke::new(1.0, ui.visuals().warn_fg_color), egui::StrokeKind::Inside);
            }
            let resp = ui.interact(full, ui.id().with(("xaos_share", i)), egui::Sense::hover());
            hinted(
                resp,
                &mut app.ui_state,
                if drained {
                    format!("{}: drained. Its weight asks {}% of the walk, but nothing that keeps firing leads to it.", nm, super::num::fixed(sum.chain.start[p] as f32 * 100.0, 1))
                } else {
                    format!("{} fires {}% of the walk; its weight asks {}%.", nm, super::num::fixed(sum.shares[p] as f32 * 100.0, 1), super::num::fixed(sum.chain.start[p] as f32 * 100.0, 1))
                },
                "how much of the walk this map gets",
            );
        }
    });

    if !sum.chain.dead.is_empty() {
        let names: Vec<String> = sum.chain.dead.iter().map(|&p| name_of(app, sum.idx[p])).collect();
        ui.label(
            egui::RichText::new(format!("Dead end: {} — nothing may follow, so the walk restarts from the plain weights there.", names.join(", ")))
                .small()
                .color(ui.visuals().warn_fg_color),
        );
    }
}

/// A row header's menu: the operations on one map's outgoing row.
fn row_menu(ui: &mut egui::Ui, app: &mut App, sum: &crate::app::XaosSummary, p: usize) {
    let i = sum.idx[p];
    if ui.button("Reset this row").clicked() {
        app.reset_xaos_row(i);
        ui.close();
    }
    if ui.button("No self-loop").clicked() {
        app.set_xaos_edge(i, i, 0.0);
        ui.close();
    }
    ui.menu_button("Only to…", |ui| {
        for &j in sum.idx.iter().take(200) {
            if ui.button(name_of(app, j)).clicked() {
                app.set_xaos_probability(i, j, 1.0);
                ui.close();
            }
        }
    });
    if ui.button("Insert a linked map").clicked() {
        app.insert_linked_transform(i);
        ui.close();
    }
}

/// A column header's menu: the operations on one map's incoming edges, which
/// live in everyone else's rows.
fn column_menu(ui: &mut egui::Ui, app: &mut App, sum: &crate::app::XaosSummary, p: usize) {
    let j = sum.idx[p];
    if ui.button("Restore every edge into this map").clicked() {
        let edges: Vec<(usize, usize, f32)> = sum.idx.iter().map(|&i| (i, j, 1.0)).collect();
        app.set_xaos_edges(&edges, Some(format!("Xaos: restore every edge into {}", name_of(app, j))));
        ui.close();
    }
    ui.menu_button("Only reachable from…", |ui| {
        for &k in sum.idx.iter().take(200) {
            if ui.button(name_of(app, k)).clicked() {
                let edges: Vec<(usize, usize, f32)> =
                    sum.idx.iter().map(|&i| (i, j, if i == k { 1.0 } else { 0.0 })).collect();
                app.set_xaos_edges(&edges, Some(format!("Xaos: {} only after {}", name_of(app, j), name_of(app, k))));
                ui.close();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// The viewport overlay: edges between gizmo origins.
// ---------------------------------------------------------------------------

/// At most this many edges are drawn; the rest are counted and said.
pub const OVERLAY_CAP: usize = 300;
/// Past this many maps in a layer, nothing-selected shows no edges: the matrix
/// window is where a big graph lives.
const OVERLAY_ALL_MAX: usize = 12;
/// How close, in physical pixels, the pointer must be to pick an edge.
pub const EDGE_PICK_PX: f32 = 6.0;

/// One drawn edge, in physical pixels. The single definition both the
/// painting and the picking use — an edge you can see in one place and grab in
/// another is worse than none (`pick::roll_ring`'s rule).
pub struct OverlayEdge {
    /// Scene indices.
    pub from: usize,
    pub to: usize,
    pub pts: Vec<egui::Pos2>,
    /// `P(to | from)`.
    pub p: f64,
    pub x: f32,
    pub cut: bool,
    /// An incoming edge in the comes-from view: drawn fainter.
    pub dim: bool,
}

/// Whether the overlay is showing at all.
pub fn overlay_active(app: &App) -> bool {
    app.show_gizmos && app.ui_state.xaos_overlay && app.selected_subscene().is_none()
}

/// The edges the overlay draws this frame, and how many it left out.
///
/// With a map selected: its outgoing edges, every allowed one as an arrow
/// whose width is its probability and every cut one as a short stub with an
/// × — plus, in the comes-from view, its incoming edges, fainter. With
/// nothing selected: only the edges that differ from 1, and only in a layer
/// small enough to read that way. An untouched scene draws nothing.
pub fn overlay_edges(app: &mut App) -> (Vec<OverlayEdge>, usize) {
    if !overlay_active(app) {
        return (Vec::new(), 0);
    }
    let selected = app.selected_transform();
    let layer = selected.and_then(|i| app.scene.transforms.get(i)).map_or(app.current_layer(), |t| t.layer as usize);
    let Some(sum) = app.xaos_summary(layer) else { return (Vec::new(), 0) };
    let n = sum.idx.len();
    let (w, h) = app.surface_size();
    if w == 0 || h == 0 {
        return (Vec::new(), 0);
    }
    let view_proj = app.current_view_proj();
    let placement = app.scene.layer_placement(layer);
    let screen: Vec<Option<egui::Pos2>> = sum
        .idx
        .iter()
        .map(|&i| {
            let o = placement.transform_point3(app.scene.transforms[i].matrix.w_axis.truncate());
            crate::camera::world_to_screen(o, view_proj, w as f32, h as f32).map(|(x, y)| egui::pos2(x, y))
        })
        .collect();

    let mut pairs: Vec<(usize, usize, bool)> = Vec::new();
    match selected.and_then(|i| sum.pos(i)) {
        Some(s) => {
            pairs.extend((0..n).map(|j| (s, j, false)));
            if app.ui_state.xaos_from {
                pairs.extend((0..n).filter(|&i| i != s).map(|i| (i, s, true)));
            }
        }
        None if n <= OVERLAY_ALL_MAX => {
            for i in 0..n {
                let row = &app.scene.transforms[sum.idx[i]].xaos;
                pairs.extend((0..n).filter(|&j| row.get(j) != 1.0).map(|j| (i, j, false)));
            }
        }
        None => {}
    }

    let mut out = Vec::new();
    let mut skipped = 0;
    for (f, t, dim) in pairs {
        let (Some(a), Some(b)) = (screen[f], screen[t]) else { continue };
        let x = app.scene.transforms[sum.idx[f]].xaos.get(t);
        let p = sum.chain.prob(f, t);
        let cut = x == 0.0;
        if !cut && p == 0.0 {
            // A zero-weight or disabled target: not an edge anyone drew.
            continue;
        }
        if out.len() >= OVERLAY_CAP {
            skipped += 1;
            continue;
        }
        out.push(OverlayEdge { from: sum.idx[f], to: sum.idx[t], pts: edge_curve(a, b, cut), p, x, cut, dim });
    }
    (out, skipped)
}

/// The curve an edge is drawn along. A self-loop is a small circle beside the
/// dot, up and to the left, clear of the name label on its right; anything else bows to its left, so `a → b` and `b → a` bow opposite
/// ways and both read. A cut is a stub a third of the way, not a whole line:
/// what isn't there shouldn't cross the picture.
fn edge_curve(a: egui::Pos2, b: egui::Pos2, cut: bool) -> Vec<egui::Pos2> {
    let d = b - a;
    let len = d.length();
    if len < 2.0 {
        let c = a + egui::vec2(-11.0, -10.0);
        return (0..=16)
            .map(|k| {
                let t = k as f32 / 16.0 * std::f32::consts::TAU + 2.35;
                c + egui::vec2(t.cos(), t.sin()) * 9.0
            })
            .collect();
    }
    let perp = egui::vec2(-d.y, d.x) / len;
    let ctrl = a + d * 0.5 + perp * len * 0.15;
    let end = if cut { 0.32 } else { 1.0 };
    // Stop short of the target's dot so the arrowhead sits beside it.
    let stop = if cut { end } else { (1.0 - 8.0 / len).max(0.5) };
    (0..=14)
        .map(|k| {
            let t = k as f32 / 14.0 * stop;
            let u = 1.0 - t;
            egui::pos2(
                u * u * a.x + 2.0 * u * t * ctrl.x + t * t * b.x,
                u * u * a.y + 2.0 * u * t * ctrl.y + t * t * b.y,
            )
        })
        .collect()
}

fn dist_to_polyline(pts: &[egui::Pos2], q: egui::Pos2) -> f32 {
    pts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((q - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t - q).length()
        })
        .fold(f32::INFINITY, f32::min)
}

/// The edge under a physical-pixel point, if any.
pub fn pick_edge(edges: &[OverlayEdge], q: (f32, f32)) -> Option<(usize, usize)> {
    let q = egui::pos2(q.0, q.1);
    edges
        .iter()
        .map(|e| (e, dist_to_polyline(&e.pts, q)))
        .filter(|(_, d)| *d <= EDGE_PICK_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(e, _)| (e.from, e.to))
}

fn segments_cross(p1: egui::Pos2, p2: egui::Pos2, q1: egui::Pos2, q2: egui::Pos2) -> bool {
    let cross = |o: egui::Pos2, a: egui::Pos2, b: egui::Pos2| (a - o).x * (b - o).y - (a - o).y * (b - o).x;
    let (d1, d2) = (cross(q1, q2, p1), cross(q1, q2, p2));
    let (d3, d4) = (cross(p1, p2, q1), cross(p1, p2, q2));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// The allowed edges a knife stroke (physical pixels) crosses.
pub fn knife_cuts(edges: &[OverlayEdge], stroke: &[(f32, f32)]) -> Vec<(usize, usize)> {
    let s: Vec<egui::Pos2> = stroke.iter().map(|&(x, y)| egui::pos2(x, y)).collect();
    edges
        .iter()
        .filter(|e| !e.cut)
        .filter(|e| {
            s.windows(2).any(|k| e.pts.windows(2).any(|w| segments_cross(k[0], k[1], w[0], w[1])))
        })
        .map(|e| (e.from, e.to))
        .collect()
}

/// Paint the overlay, under the panels like the gizmo labels, and the open
/// edge's popover and menu above them.
pub fn draw_overlay(ctx: &egui::Context, app: &mut App) {
    let (edges, skipped) = overlay_edges(app);
    let ppp = ctx.pixels_per_point();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("fracturize_xaos_overlay")));
    let pointer = ctx.pointer_hover_pos().map(|p| (p.x * ppp, p.y * ppp));
    let over_panel = ctx.is_pointer_over_egui();
    let hovered = pointer.filter(|_| !over_panel && app.hovered_transform().is_none()).and_then(|q| pick_edge(&edges, q));
    let selected_edge = app.ui_state.xaos_edge;

    for e in &edges {
        let pts: Vec<egui::Pos2> = e.pts.iter().map(|p| egui::pos2(p.x / ppp, p.y / ppp)).collect();
        let col = color32(app.scene.colors.get(e.to).copied().unwrap_or(glam::Vec3::ONE));
        let alpha = if e.dim { 0.35 } else { 0.85 };
        let lit = hovered == Some((e.from, e.to)) || selected_edge == Some((e.from, e.to));
        if e.cut {
            let stroke = egui::Stroke::new(if lit { 2.0 } else { 1.2 }, Color32::from_gray(200).gamma_multiply(alpha));
            painter.extend(egui::Shape::dashed_line(&pts, stroke, 4.0, 3.0));
            let end = *pts.last().unwrap();
            let s = egui::Stroke::new(1.6, Color32::from_rgb(230, 90, 80).gamma_multiply(alpha));
            painter.line_segment([end + egui::vec2(-4.0, -4.0), end + egui::vec2(4.0, 4.0)], s);
            painter.line_segment([end + egui::vec2(4.0, -4.0), end + egui::vec2(-4.0, 4.0)], s);
            continue;
        }
        let width = 1.0 + 4.0 * e.p as f32;
        if lit {
            painter.add(egui::Shape::line(pts.clone(), egui::Stroke::new(width + 3.0, Color32::from_white_alpha(170))));
        }
        painter.add(egui::Shape::line(pts.clone(), egui::Stroke::new(width, col.gamma_multiply(alpha))));
        // The arrowhead, at the end the walk goes to.
        if pts.len() >= 2 {
            let (a, b) = (pts[pts.len() - 2], pts[pts.len() - 1]);
            let d = (b - a).normalized();
            let side = egui::vec2(-d.y, d.x);
            let size = 5.0 + width;
            painter.add(egui::Shape::convex_polygon(
                vec![b, b - d * size + side * size * 0.5, b - d * size - side * size * 0.5],
                col.gamma_multiply(alpha),
                egui::Stroke::NONE,
            ));
        }
    }
    if let Some(stroke) = &app.ui_state.xaos_knife {
        let pts: Vec<egui::Pos2> = stroke.iter().map(|&(x, y)| egui::pos2(x / ppp, y / ppp)).collect();
        painter.add(egui::Shape::line(pts, egui::Stroke::new(2.0, Color32::from_rgb(230, 90, 80))));
    }
    if skipped > 0 {
        let at = ctx.content_rect().left_bottom() + egui::vec2(10.0, -44.0);
        painter.text(at, egui::Align2::LEFT_BOTTOM, format!("xaos: {skipped} more edges not drawn — the Xaos window has them all"), egui::FontId::proportional(12.0), Color32::from_gray(210));
    }
    if let Some((a, b)) = hovered {
        app.ui_state.status_hint = Some(format!(
            "{} \u{E06C} {}: click to edit · alt+scroll: scale · right-click: menu · ctrl+right-drag: cut several",
            name_of(app, a),
            name_of(app, b)
        ));
    }

    draw_edge_popover(ctx, app, &edges);
    draw_edge_menu(ctx, app);
}

/// The clicked edge's popover, at its midpoint: the multiplier, what it comes
/// to as a probability, and cut/restore.
fn draw_edge_popover(ctx: &egui::Context, app: &mut App, edges: &[OverlayEdge]) {
    let Some((a, b)) = app.ui_state.xaos_edge else { return };
    let Some(e) = edges.iter().find(|e| (e.from, e.to) == (a, b)) else {
        // The edge left the overlay (selection changed, overlay off).
        app.ui_state.xaos_edge = None;
        return;
    };
    let ppp = ctx.pixels_per_point();
    let mid = e.pts[e.pts.len() / 2];
    let pos = egui::pos2(mid.x / ppp + 12.0, mid.y / ppp + 8.0);
    let (x, p) = (e.x, e.p);
    let mut close = false;
    egui::Area::new(egui::Id::new("fracturize_xaos_edge"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{} \u{E06C} {}", name_of(app, a), name_of(app, b))).strong().small());
                    let r = ui.add(egui::Button::new(icons::X).small().frame(false));
                    if hinted(r, &mut app.ui_state, "Close (Esc)", "click: close").clicked() {
                        close = true;
                    }
                });
                ui.horizontal(|ui| {
                    let mut v = x;
                    let speed = (v.max(0.05) * 0.02) as f64;
                    let r = super::num::drag(ui, 5, 2, egui::DragValue::new(&mut v).speed(speed).range(0.0..=100.0).prefix("×"));
                    let r = hinted(r, &mut app.ui_state, "The odds of this edge, relative to the target's weight. 1 is as usual, 0 never.", "drag: scale this edge · click: type");
                    if r.changed() {
                        app.set_xaos_edge(a, b, v);
                    }
                    let readout = if p == 0.0 { "never".to_string() } else { format!("{}%", super::num::fixed(p as f32 * 100.0, 1)) };
                    let r = super::num::small_cell(ui, &readout, 6);
                    hinted(r, &mut app.ui_state, "The chance this edge's target follows its source.", "the chance this edge is taken");
                });
                ui.horizontal(|ui| {
                    let (label, v, tip) = if x == 0.0 {
                        ((icons::RESET, "Restore"), 1.0, "Restore this edge to 1.")
                    } else {
                        ((icons::SCISSORS, "Cut"), 0.0, "Cut this edge: removes material, where a scale only moves density.")
                    };
                    let r = ui.add(egui::Button::new(label).small());
                    if hinted(r, &mut app.ui_state, tip, "click: cut or restore this edge").clicked() {
                        app.set_xaos_edge(a, b, v);
                    }
                    let r = ui.add(egui::Button::new(egui::RichText::new("Only exit").small()));
                    if hinted(r, &mut app.ui_state, "Make this the only edge out of its source.", "click: make this the only way on").clicked() {
                        app.set_xaos_probability(a, b, 1.0);
                    }
                });
            });
        });
    if close {
        app.ui_state.xaos_edge = None;
    }
}

/// An edge's right-click menu: the same operations, without opening the
/// popover.
fn draw_edge_menu(ctx: &egui::Context, app: &mut App) {
    let Some(((a, b), stored)) = app.ui_state.xaos_edge_menu else { return };
    let pos = match stored {
        Some(p) => p,
        None => {
            let p = ctx.pointer_latest_pos().unwrap_or_else(|| ctx.content_rect().center());
            app.ui_state.xaos_edge_menu = Some(((a, b), Some(p)));
            p
        }
    };
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("fracturize_xaos_edge_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_max_width(190.0);
                ui.label(egui::RichText::new(format!("{} \u{E06C} {}", name_of(app, a), name_of(app, b))).strong().small());
                ui.separator();
                let x = app.scene.transforms.get(a).map_or(1.0, |t| {
                    let pos = app.scene.layer_position(b);
                    t.xaos.get(pos)
                });
                if x != 0.0 && ui.button((icons::SCISSORS, "Cut")).clicked() {
                    app.set_xaos_edge(a, b, 0.0);
                    close = true;
                }
                if x != 1.0 && ui.button((icons::RESET, "Restore to 1")).clicked() {
                    app.set_xaos_edge(a, b, 1.0);
                    close = true;
                }
                if ui.button("Make it the only exit").clicked() {
                    app.set_xaos_probability(a, b, 1.0);
                    close = true;
                }
                if ui.button((icons::LINK, format!("Linked map after {}", name_of(app, a)))).clicked() {
                    app.insert_linked_transform(a);
                    close = true;
                }
                if ui.button("Edit…").clicked() {
                    app.ui_state.xaos_edge = Some((a, b));
                    close = true;
                }
            });
        });
    if stored.is_some() && ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() {
        close = true;
    }
    if close {
        app.ui_state.xaos_edge_menu = None;
    }
}
