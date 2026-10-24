//! Tables: `egui_extras::TableBuilder` in the house style.
//!
//! A table is for values that line up — the same kind of number down a
//! column, a header saying what it is. egui's own `Grid` lays things out in
//! columns but draws none of them, so a grid of mixed controls reads as a
//! scatter of fields; this draws the column rules, stripes the rows and puts
//! a hinted header over each column, so a column's tooltip says what every
//! number in it means.
//!
//! The rules are `TableBuilder`'s resize handles, drawn at rest in the
//! non-interactive stroke, so a column can also be widened by dragging its
//! rule. A table never scrolls on its own: it sits in its window's scroll
//! area with everything else.

use egui_extras::{Column, TableBuilder};

use super::hints::hinted;
use super::UiState;

/// One column: its header, the header's tooltip, and its starting width
/// (the last column takes what's left).
pub struct Col<'a> {
    pub title: &'a str,
    pub tip: &'a str,
    pub width: f32,
}

impl<'a> Col<'a> {
    pub const fn new(title: &'a str, tip: &'a str, width: f32) -> Self {
        Self { title, tip, width }
    }
}

/// Row height: one interactive control and a little air.
pub fn row_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y + 4.0
}

/// Draw a table: `cols` across the top, then `body` adds the rows.
pub fn show(
    ui: &mut egui::Ui,
    st: &mut UiState,
    id: &str,
    cols: &[Col<'_>],
    body: impl FnOnce(&mut egui_extras::TableBody<'_>, &mut UiState),
) {
    let h = row_height(ui);
    let mut builder = TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .resizable(true)
        .vscroll(false)
        .animate_scrolling(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
    for (i, c) in cols.iter().enumerate() {
        let col = if i + 1 == cols.len() { Column::remainder() } else { Column::initial(c.width) };
        builder = builder.column(col.at_least(24.0).clip(true));
    }
    builder
        .header(h, |mut header| {
            for c in cols {
                header.col(|ui| {
                    let resp = ui.add(
                        egui::Label::new(egui::RichText::new(c.title).small().strong()).sense(egui::Sense::hover()),
                    );
                    hinted(resp, st, c.tip, c.title);
                });
            }
        })
        .body(|mut b| body(&mut b, st));
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
