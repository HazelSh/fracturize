//! The split viewport's own furniture (`multiview`): a faint edge round each
//! pane and, in its top-left corner, what it is looking at. "Top +Y" names
//! the view in the words a drawing uses and the axis the way the axis cross
//! in the pane's corner draws it, so the caption and the cross can be read
//! against each other.
//!
//! On egui's background layer, like the gizmo labels: under every window, and
//! taking no input. Drawn in view mode too, unlike the axis crosses, because
//! four pictures with nothing saying which is which is a puzzle rather than a
//! way of looking.

use crate::app::App;
use crate::multiview::Slot;

pub fn draw(ctx: &egui::Context, app: &App) {
    let panes = app.panes();
    if panes.len() < 2 {
        return;
    }
    let ppp = ctx.pixels_per_point();
    let visuals = ctx.global_style().visuals.clone();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("fracturize_panes")));
    let font = egui::FontId::proportional(12.0);
    for pane in &panes {
        let r = pane.rect;
        let rect = egui::Rect::from_min_size(
            egui::pos2(r.x as f32 / ppp, r.y as f32 / ppp),
            egui::vec2(r.w as f32 / ppp, r.h as f32 / ppp),
        );
        painter.rect_stroke(rect, 0.0, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Outside);
        let text = match pane.slot {
            Slot::Perspective => "Perspective".to_string(),
            Slot::Axis(k) => {
                let axis = app.view_axis(k);
                format!("{}  {}", axis.name(), axis.signed())
            }
        };
        let galley = painter.layout_no_wrap(text, font.clone(), visuals.text_color());
        let plate = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 6.0), galley.size()).expand2(egui::vec2(5.0, 2.0));
        painter.rect_filled(plate, 3.0, egui::Color32::from_black_alpha(150));
        painter.galley(rect.min + egui::vec2(6.0, 6.0), galley, visuals.text_color());
    }
}
