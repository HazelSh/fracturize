//! Tables: `egui_extras::TableBuilder` in the house style.
//!
//! A table is for values that line up — the same kind of number down a
//! column, a header saying what it is. egui's own `Grid` lays things out in
//! columns but draws none of them, so a grid of mixed controls reads as a
//! scatter of fields; this stripes the rows, rules the columns, underlines
//! the header and puts a hinted title over each column, so a column's
//! tooltip says what every value in it means.
//!
//! **Rows carry more weight than columns.** You read a table along a row (this
//! part: its odds, its share, its switch), so the stripes are strong and the
//! column rules faint. egui's defaults run the other way — `faint_bg_color` is
//! all but invisible on the dark theme, and the rules are the full separator
//! stroke — which read as columns with no rows in them. Both are set here, for
//! the table only.
//!
//! **The name column takes the slack, and a tick column sits at the right
//! edge** ([`Col::tick`]). So every table with a switch has it in the same
//! place, flush right, whatever its other columns are; the ticks line up down
//! the window from one table to the next.
//!
//! The rules are `TableBuilder`'s resize handles, so a column can be widened
//! by dragging its rule. A table never scrolls on its own: it sits in its
//! window's scroll area with everything else.

use egui_extras::{Column, TableBuilder};


use super::hints::hinted;
use super::UiState;

/// How far the stripe is lifted from the window toward the text colour.
const STRIPE: f32 = 0.11;
/// How much of the separator stroke the column rules keep.
const RULE: f32 = 0.45;
/// Width of a tick column: a checkbox and its margins.
const TICK_W: f32 = 30.0;

/// How a column is sized.
#[derive(Clone, Copy, PartialEq)]
enum Width {
    /// The name column: whatever the others leave.
    Fill,
    Initial(f32),
    /// A switch, flush right.
    Tick,
}

/// One column: its header, the header's tooltip, and how it's sized.
pub struct Col<'a> {
    pub title: &'a str,
    pub tip: &'a str,
    width: Width,
}

impl<'a> Col<'a> {
    /// The name column, which takes the slack. Put it first.
    pub const fn name(title: &'a str, tip: &'a str) -> Self {
        Self { title, tip, width: Width::Fill }
    }

    /// A value column, starting `width` wide.
    pub const fn new(title: &'a str, tip: &'a str, width: f32) -> Self {
        Self { title, tip, width: Width::Initial(width) }
    }

    /// A switch column. Put it last, so it sits at the right edge.
    pub const fn tick(title: &'a str, tip: &'a str) -> Self {
        Self { title, tip, width: Width::Tick }
    }
}

/// Row height: one interactive control and a little air.
pub fn row_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y + 4.0
}

/// `a` moved toward `b` by `t`.
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

/// Draw a table: `cols` across the top, then `body` adds the rows.
///
/// Every column is an exact width — the name column whatever the window has
/// left — so a table always spans its window, and a tick column lands at the
/// same x in every table. The column rules and the header's rule are painted
/// after the rows, over the stripes, which would otherwise cover them.
pub fn show(
    ui: &mut egui::Ui,
    st: &mut UiState,
    id: &str,
    cols: &[Col<'_>],
    body: impl FnOnce(&mut egui_extras::TableBody<'_>, &mut UiState),
) {
    ui.scope(|ui| {
        let separator = ui.visuals().widgets.noninteractive.bg_stroke;
        let ground = ui.visuals().window_fill;
        let stripe = mix(ground, ui.visuals().text_color(), STRIPE);
        ui.visuals_mut().faint_bg_color = stripe;
        let rule = egui::Stroke::new(separator.width, mix(ground, separator.color, RULE));

        let sx = ui.spacing().item_spacing.x;
        let fixed: f32 = cols
            .iter()
            .map(|c| match c.width {
                Width::Fill => 0.0,
                Width::Initial(w) => w,
                Width::Tick => TICK_W,
            })
            .sum::<f32>()
            + sx * (cols.len().saturating_sub(1)) as f32;
        let fill = (ui.available_width() - fixed).max(90.0);
        let widths: Vec<f32> = cols
            .iter()
            .map(|c| match c.width {
                Width::Fill => fill,
                Width::Initial(w) => w,
                Width::Tick => TICK_W,
            })
            .collect();

        let left = ui.cursor().left();
        let top = ui.cursor().top();
        let h = row_height(ui);
        let mut builder = TableBuilder::new(ui)
            .id_salt(id)
            .striped(true)
            .resizable(false)
            .vscroll(false)
            .animate_scrolling(false)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for &w in &widths {
            builder = builder.column(Column::exact(w).clip(true));
        }
        let mut header_bottom = top + h;
        builder
            .header(h, |mut header| {
                for c in cols {
                    let (rect, _) = header.col(|ui| {
                        let text = egui::RichText::new(c.title).small().strong();
                        let resp = ui.add(egui::Label::new(text).sense(egui::Sense::hover()));
                        hinted(resp, st, c.tip, c.title);
                    });
                    header_bottom = rect.bottom();
                }
            })
            .body(|mut b| body(&mut b, st));

        let bottom = ui.min_rect().bottom();
        let right = left + widths.iter().sum::<f32>() + sx * (widths.len().saturating_sub(1)) as f32;
        let painter = ui.painter();
        let mut x = left - sx * 0.5;
        for w in &widths[..widths.len().saturating_sub(1)] {
            x += w + sx;
            painter.vline(x, top..=bottom, rule);
        }
        let y = header_bottom + ui.spacing().item_spacing.y * 0.5;
        painter.hline(left..=right, y, separator);
    });
}

/// A row naming a group of the rows under it, in the first column.
pub fn group_row(body: &mut egui_extras::TableBody<'_>, title: &str) {
    let h = body.ui_mut().spacing().interact_size.y;
    body.row(h, |mut row| {
        row.col(|ui| {
            ui.label(egui::RichText::new(title).small().weak());
        });
    });
}

/// A tick cell's contents: the checkbox alone, centred in its column.
pub fn tick(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Center), |ui| ui.add(egui::Checkbox::without_text(on)))
    .inner
}
