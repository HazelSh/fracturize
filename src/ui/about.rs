//! About Fracturize: what it is, which build this is, and where it lives.
//! Help → About. It used to head the Controls window, which is a reference
//! for the keys now and nothing else.

use crate::app::App;

use super::hints::hinted;

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
fn draw_body(ui: &mut egui::Ui, app: &mut App) {
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

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.about_open;
    if !open {
        return;
    }
    super::window(ctx, app, super::WindowKey::About, "About Fracturize")
        .open(&mut open)
        .show(ctx, |ui| draw_body(ui, app));
    super::remember(ctx, app, super::WindowKey::About);
    app.ui_state.panels.about_open = open;
}
