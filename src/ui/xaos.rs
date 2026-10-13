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
        1 => format!("only → {}", name_of(app, sum.idx[exits[0]])),
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
                    "{a} → {b}: the odds of {b} straight after {a}, relative to its weight. \
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
                (icons::RESET, format!("Restore {a} → {b} to 1."), "click: restore this edge")
            } else {
                (
                    icons::SCISSORS,
                    format!("Cut {a} → {b}: {b} never follows {a}. This removes material, where a scale only moves density."),
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
