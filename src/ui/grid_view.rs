//! The mutation grid as the view (Ctrl+U): the viewport split into a 3×3 or
//! 5×5 of panes, the scene in the middle and its mutations around it, every
//! pane drawn from the one camera. So orbiting, zooming, panning or flying
//! the path explores all of them together — a mutation can be judged in 3D,
//! from the angle that matters, before it's taken.
//!
//! A click that doesn't become a drag picks the tile under it (one edit; the
//! ring grows again around it, and Ctrl+Z brings the old grid back), and a
//! click on the middle rolls new mutations. Everything else about the camera
//! is unchanged, which is why the panes are painted on egui's background
//! layer, under every window, and take no input of their own: the viewport's
//! drag and scroll handling in `app.rs` never knows the grid is there, except
//! to turn a click into a pick (`App::grid_click`).
//!
//! The model is `mutation_grid`, shared with the Mutate window; the panes are
//! `gpu::thumbs::GridPanes`, splatted whatever the renderer, at the window's
//! point budget divided between them.

use crate::app::App;
use crate::gpu::thumbs::ThumbState;
use crate::mutation_grid::{self, Tile};

/// Points between panes.
const GAP: f32 = 2.0;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    if !app.grid_view() {
        app.ui_state.grid_hover = None;
        return;
    }
    app.poll_mutation_grid();
    let area = app.ui_state.grid_area.unwrap_or_else(|| ctx.content_rect());
    let ppp = ctx.pixels_per_point();
    let n = app.mutation_grid_size();
    let span = GAP * (n - 1) as f32;
    let cell = egui::vec2((area.width() - span) / n as f32, (area.height() - span) / n as f32);
    if cell.x < 8.0 || cell.y < 8.0 {
        return;
    }
    // Even, so a resize by one pixel doesn't rebuild every pane twice.
    let even = |v: f32| ((v * ppp / 2.0).round() as u32 * 2).max(2);
    app.ui_state.grid_pane_px = (even(cell.x), even(cell.y));

    let pointer = ctx.pointer_hover_pos().filter(|_| !ctx.is_pointer_over_egui());
    let painter = ctx.layer_painter(egui::LayerId::background());
    let visuals = ctx.global_style().visuals.clone();
    let (grid, panes, ui_state) = app.grid_view_parts();
    let mut hover = None;
    let mut hint = None;
    let Some(parent) = grid.parent() else {
        ui_state.grid_hover = None;
        ui_state.status_hint = Some("making mutations…".to_string());
        return;
    };
    if grid.tiles.len() as u32 != mutation_grid::children(n) {
        return;
    }
    for k in 0..=mutation_grid::children(n) {
        let (col, row) = mutation_grid::cell(n, k);
        let rect = egui::Rect::from_min_size(
            area.min + egui::vec2(col as f32 * (cell.x + GAP), row as f32 * (cell.y + GAP)),
            cell,
        );
        let hovered = pointer.is_some_and(|p| rect.contains(p));
        let (id, scene) = if k == 0 {
            (Some(grid.parent_id), Some(parent))
        } else {
            match &grid.tiles[k as usize - 1] {
                Tile::Ready(c) => (Some(c.id), Some(&c.scene)),
                _ => (None, None),
            }
        };
        let ground = |s: &crate::scene::Scene| {
            let [r, g, b] = crate::palette::to_srgb8(s.background);
            egui::Color32::from_rgb(r, g, b)
        };
        match id.zip(scene) {
            Some((id, scene)) => match panes.get(id, scene) {
                ThumbState::Ready(tex) => {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.image(tex, rect, uv, egui::Color32::WHITE);
                }
                ThumbState::Pending => {
                    painter.rect_filled(rect, 0.0, ground(scene));
                }
                ThumbState::Failed(e) => {
                    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
                    caption(&painter, rect, &format!("can't draw this one: {e}"), &visuals);
                }
            },
            None => {
                painter.rect_filled(rect, 0.0, ground(parent).gamma_multiply(0.6));
                let failed = matches!(grid.tiles.get(k as usize - 1), Some(Tile::Failed(_)));
                caption(&painter, rect, if failed { "nothing passed here" } else { "mutating…" }, &visuals);
            }
        }

        if k == 0 {
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, visuals.selection.stroke.color), egui::StrokeKind::Inside);
        }
        if hovered {
            hover = Some(k);
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(2.0, visuals.widgets.hovered.fg_stroke.color),
                egui::StrokeKind::Inside,
            );
            hint = Some(match &grid.tiles.get((k as usize).wrapping_sub(1)) {
                _ if k == 0 => "your scene · click: new mutations around it · drag: orbit all of them · Ctrl+U: back to one view".to_string(),
                Some(Tile::Ready(c)) => {
                    let what = c.log.join("; ");
                    caption(&painter, rect, &what, &visuals);
                    let strength = if c.strength < c.asked {
                        format!("strength {:.2}, backed off from {:.2}", c.strength, c.asked)
                    } else {
                        format!("strength {:.2}", c.strength)
                    };
                    format!("click: make this the scene (Ctrl+Z brings this grid back) · {strength} · seed {}", c.seed)
                }
                Some(Tile::Failed(why)) => format!("no mutation here: {why}"),
                _ => "being made".to_string(),
            });
        }
    }
    ui_state.grid_hover = hover;
    if hint.is_some() {
        ui_state.status_hint = hint;
    }
}

/// A line of text along a pane's foot, on a dark band so it reads over any
/// picture.
fn caption(painter: &egui::Painter, rect: egui::Rect, text: &str, visuals: &egui::Visuals) {
    let font = egui::FontId::proportional(12.0);
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, visuals.strong_text_color());
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - 12.0).max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let band = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - galley.size().y - 8.0), rect.max);
    painter.rect_filled(band, 0.0, egui::Color32::from_black_alpha(150));
    painter.galley(egui::pos2(rect.left() + 6.0, band.top() + 4.0), galley, visuals.strong_text_color());
}
