//! World-anchored transform name labels, painted next to each gizmo origin.
//!
//! The last thing the legacy text shim was still carrying. Painted straight
//! through an egui layer painter now, which also means the selected label
//! gets a real rounded-rect backdrop instead of the row of `█` glyphs the
//! shim used to fake one with.
//!
//! Lives on `Order::Background`, under the panels: these are viewport
//! decorations, and a transform whose origin happens to sit behind the
//! Transforms window must not paint across its interior.

use crate::app::App;
use crate::camera::world_to_screen;

const FONT_SIZE: f32 = 12.0;

pub fn draw(ctx: &egui::Context, app: &App) {
    // Labels live and die with the gizmos (G): they name the very things the
    // gizmos draw, and a separate toggle for "the gizmo's caption" was a
    // distinction without a difference.
    if !app.show_gizmos {
        return;
    }

    let (w, h) = app.surface_size();
    if w == 0 || h == 0 {
        return;
    }
    let view_proj = app.current_view_proj();
    // Projection is in physical pixels; egui works in logical points.
    let ppp = ctx.pixels_per_point();

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("fracturize_gizmo_labels"),
    ));
    let font = egui::FontId::proportional(FONT_SIZE);
    let selected = app.selected_transform();
    // Hovering a gizmo promotes its label to the same solid backdrop the
    // selected one gets. Bare white text vanishes against a bright attractor,
    // and reading a name is how you decide whether this is the transform you
    // meant — which matters most *before* you have selected it, exactly when
    // the label was hardest to read.
    let hovered = app.hovered_transform();

    // Offset magnitude, printed at the midpoint of the offset vector
    // src/indicators.rs draws from the origin to the selected transform.
    if let Some((offset, len)) = app.selected_offset() {
        if let Some((sx, sy)) = world_to_screen(offset * 0.5, view_proj, w as f32, h as f32) {
            painter.text(
                egui::pos2(sx / ppp + 6.0, sy / ppp),
                egui::Align2::LEFT_CENTER,
                format!("{:.2}", len),
                egui::FontId::proportional(FONT_SIZE - 1.0),
                egui::Color32::from_rgb(255, 199, 89),
            );
        }
    }

    for (i, spec) in app.scene.transforms.iter().enumerate() {
        let origin = spec.matrix.w_axis.truncate();
        let Some((sx, sy)) = world_to_screen(origin, view_proj, w as f32, h as f32) else {
            continue;
        };
        let label = app
            .scene
            .transform_names
            .get(i)
            .and_then(|n| n.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("T{}", i));

        let pos = egui::pos2(sx / ppp + 8.0, sy / ppp - 7.0);

        if selected == Some(i) || hovered == Some(i) {
            let galley = painter.layout_no_wrap(label, font.clone(), egui::Color32::BLACK);
            let rect = egui::Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(4.0, 2.0));
            // The hovered-but-unselected label sits a touch more opaque, so
            // scrubbing across a crowded scene reads as one name at a time
            // rather than two competing plates.
            let plate = if selected == Some(i) { 225 } else { 245 };
            painter.rect_filled(rect, 3.0, egui::Color32::from_white_alpha(plate));
            painter.galley(pos, galley, egui::Color32::BLACK);
        } else {
            let alpha: u8 = if app.is_transform_enabled(i) { 200 } else { 80 };
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                label,
                font.clone(),
                egui::Color32::from_white_alpha(alpha),
            );
        }
    }
}
