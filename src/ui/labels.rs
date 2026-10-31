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
    // Projection is in physical pixels; egui works in logical points.
    let ppp = ctx.pixels_per_point();

    let layer = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("fracturize_gizmo_labels"),
    ));
    let font = egui::FontId::proportional(FONT_SIZE);
    let selected = app.gizmo_selection();
    // Hovering a gizmo promotes its label to the same solid backdrop the
    // selected one gets. Bare white text vanishes against a bright attractor,
    // and reading a name is how you decide whether this is the transform you
    // meant — which matters most *before* you have selected it, exactly when
    // the label was hardest to read.
    let hovered = app.hovered_transform();

    // In scene space: a map in a placed subscene is labelled where it is
    // drawn, and a subscene selected as a whole is labelled by its name.
    let labels = app.gizmo_labels();
    // In every pane, through that pane's camera, and kept inside it
    // (`multiview`): one pane, the whole window, in the one-view layout.
    for pane in app.panes() {
        let r = pane.rect;
        let painter = layer.with_clip_rect(egui::Rect::from_min_size(
            egui::pos2(r.x as f32 / ppp, r.y as f32 / ppp),
            egui::vec2(r.w as f32 / ppp, r.h as f32 / ppp),
        ));
        let view_proj = pane.view_proj();
    for (i, (matrix, label, enabled)) in labels.iter().cloned().enumerate() {
        let origin = matrix.w_axis.truncate();
        let Some((sx, sy)) = world_to_screen(origin, view_proj, r.w as f32, r.h as f32) else {
            continue;
        };
        let (sx, sy) = (sx + r.x as f32, sy + r.y as f32);

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
            let alpha: u8 = if enabled { 200 } else { 80 };
            // A 1px black shadow, for the same reason the origin dots gained a
            // rim: this text is drawn over whatever the attractor happens to
            // be, and white on a pale fractal is not text. The plated cases
            // above don't need it — the plate *is* the contrast — so only the
            // bare label carries one.
            //
            // Laid out once and painted twice rather than calling `text`
            // twice, which would shape the glyphs a second time to put them in
            // exactly the same places. Laid out in `PLACEHOLDER` because
            // `Painter::galley`'s colour argument is only a *fallback* — it
            // recolours the parts of a galley that have no colour of their own,
            // so a galley laid out white would paint the shadow white too.
            let galley = painter.layout_no_wrap(
                label,
                font.clone(),
                egui::Color32::PLACEHOLDER,
            );
            // The shadow fades with the label, so a disabled transform doesn't
            // end up with a hard black edge round faint grey text.
            painter.galley(
                pos + egui::vec2(1.0, 1.0),
                galley.clone(),
                egui::Color32::from_black_alpha(alpha),
            );
            painter.galley(pos, galley, egui::Color32::from_white_alpha(alpha));
        }
    }
    }
}
