//! The Timeline window (FEATURE-PLANS.md item 8): the one clock the camera and
//! every `[[track]]` share, drawn wide and low.
//!
//! A ruler in seconds, pinned above the rows so it never scrolls away, with
//! the playhead on it and a loop strip along its top; under it a row for the
//! camera and a row per track. Each track's row draws its keys as diamonds and
//! its value as a faint curve behind them, scaled to the row — the curve view
//! the plan asks for, small. Click a row to select its track, a diamond to
//! select its key; drag a diamond to retime it. The eye mutes a track.
//!
//! Only the ruler scrubs. A press on a row selects it and moves nothing else,
//! so choosing a track never shifts the camera.
//!
//! **Navigating** (ANIMATION-PLAN.md §2.2–2.3): Ctrl+scroll over the lanes
//! zooms about the moment under the pointer, Shift+scroll or a middle-drag
//! moves along, "Whole" shows the whole timeline again. Zoomed in, playback
//! that runs off the end turns the page — a jump, never a glide.
//!
//! **Right-click** (§2.4): on a lane, a key there or a paste; on a key, copy,
//! paste over, duplicate at the playhead, delete; on the camera's row, key the
//! framing there, or a view's own menu; on the ruler, the loop's ends.
//!
//! **Snapping** (§2.5, the magnet; Ctrl does the other thing for one drag): a
//! dragged key, a loop end and the playhead land on any row's keys, the
//! playhead, the loop's ends, the timeline's ends and the ruler's finer ticks
//! when they come within a few points. Always a few *points*, so how far it
//! reaches in seconds follows the zoom.
//!
//! **The loop region** (§3) is drawn in the ruler's strip and goes round with
//! the toolbar's loop on (`App::loop_region`, session state).
//!
//! The camera row's link attaches the camera to the timeline or detaches it.
//! Attached, it flies its path on the clock, playing and scrubbing; detached,
//! it stays where you put it while the tracks play. Taking the camera in the
//! viewport detaches it; O attaches it and plays; the Timeline's own Play
//! leaves it as it is.
//!
//! The column on the right edits what is selected: a keyed track's
//! interpolation and extrapolation, the selected key's time and value, "key at
//! the playhead", a cycle's or a rate's numbers; for the camera, attach, its
//! path's duration and loop, and a keypoint at the current framing. At its
//! foot, "Animate a parameter…" opens the scene's parameters as a tree to
//! browse or search (`ui::param_picker`); the new track starts with one key at
//! the playhead holding the scene's value there, so adding one changes nothing
//! until it is keyed again. Rows are named the same way; the `-S` path is only
//! in tooltips. Every edit is undoable; scrubbing and attaching aren't edits.
//!
//! **Keying from the scene is record** (the toolbar's ring, `ui::transport`):
//! armed, every edit keys what it changes at the playhead (`App::autokey`).
//! Disarmed, editing a tracked value in the inspector edits the scene's own
//! value — which a rate or cycle moves about, and which keys override — as it
//! always did.
//!
//! Not yet: a curve you can drag in value as well as time, box selection and
//! moving many keys at once, per-key interpolation, markers, a tempo grid.
//!
//! The playhead moves because time moves, which is content, not chrome; the
//! zero-animation rule still holds for everything else here.

use crate::app::App;
use crate::track::{Extrapolate, Interp, KeyValue, Source, Track, Wave};

use super::hints::hinted;
use super::{icons, num};

const LABEL_W: f32 = 220.0;
const DETAIL_W: f32 = 300.0;
/// How close, in points, a press has to land to a diamond to take it.
const KEY_GRAB: f32 = 7.0;
/// The loop strip along the top of the ruler...
const LOOP_H: f32 = 9.0;
/// ...and the whole ruler, ticks under it.
const RULER_H: f32 = 28.0;
const ROW_H: f32 = 22.0;
/// How close, in points, a dragged moment has to come to something to snap
/// onto it.
const SNAP_PX: f32 = 7.0;
/// How near a press in the loop strip has to be to an end of the region to
/// move that end.
const LOOP_GRAB: f32 = 6.0;
/// The shortest span zooming in reaches, in seconds.
const MIN_SPAN: f64 = 0.05;

/// What a drag in the ruler's loop strip is doing.
#[derive(Clone, Copy, Debug)]
pub enum LoopDrag {
    Start,
    End,
    /// Sliding the whole region, held `grab` seconds in from its start.
    Move { grab: f64 },
    /// Drawing a new region out from `anchor`.
    Make { anchor: f64 },
}

/// Where the lanes are across the window, and which seconds they show. The
/// ruler and the rows under it share one, so a tick and a key at the same
/// moment line up whatever the rows' scroll.
#[derive(Clone, Copy)]
struct Geo {
    left: f32,
    width: f32,
    t0: f64,
    t1: f64,
}

impl Geo {
    fn x(&self, t: f64) -> f32 {
        self.left + ((t - self.t0) / (self.t1 - self.t0)) as f32 * self.width
    }

    /// The moment at `x`, kept to the lanes and never before zero.
    fn t(&self, x: f32) -> f64 {
        (self.t0 + ((x - self.left) / self.width).clamp(0.0, 1.0) as f64 * (self.t1 - self.t0)).max(0.0)
    }

    fn lane(&self, top: f32, bottom: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(self.left, top), egui::pos2(self.left + self.width, bottom))
    }

    /// The ruler's tick steps at this zoom: a labelled one with room for its
    /// label, and the finer one between, which snapping also lands on.
    fn grid(&self) -> (f64, f64) {
        let span = self.t1 - self.t0;
        let step = [0.01, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0]
            .into_iter()
            .find(|s| (self.width as f64) / (span / s) >= 48.0)
            .unwrap_or(300.0);
        let parts = match step {
            s if s == 0.25 || s == 2.0 || s == 0.02 || s == 120.0 => 4.0,
            s if s == 15.0 || s == 30.0 => 3.0,
            _ => 5.0,
        };
        (step, step / parts)
    }
}

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.timeline_open;
    if !open {
        return;
    }
    super::window(ctx, app, super::WindowKey::Timeline, "Timeline")
        .open(&mut open)
        .show(ctx, |ui| {
            draw_header(ui, app);
            ui.separator();
            // Selection can go stale under it: undo takes a track away.
            let ui_state = &mut app.ui_state;
            if ui_state.timeline_track.is_some_and(|i| i >= app.scene.tracks.len()) {
                ui_state.timeline_track = None;
            }
            let h = ui.available_height();
            let lanes_w = (ui.available_width() - DETAIL_W - 12.0).max(200.0);
            ui.horizontal_top(|ui| {
                let down = egui::Layout::top_down(egui::Align::Min);
                ui.allocate_ui_with_layout(egui::vec2(lanes_w, h), down, |ui| {
                    // Room on the right for the rows' scroll bar, so the
                    // ruler and the rows under it map time to the same x.
                    let left = ui.max_rect().left() + LABEL_W;
                    let width = (ui.available_width() - LABEL_W - 10.0).max(60.0);
                    let area = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(lanes_w, h));
                    navigate(ui, app, area, left, width);
                    let geo = geometry(app, left, width);
                    draw_ruler(ui, app, &geo);
                    egui::ScrollArea::vertical().id_salt("timeline_lanes").auto_shrink([false, false]).show(ui, |ui| {
                        draw_rows(ui, app, &geo);
                    });
                });
                ui.separator();
                ui.allocate_ui_with_layout(egui::vec2(DETAIL_W, h), down, |ui| {
                    egui::ScrollArea::vertical().id_salt("timeline_detail").auto_shrink([false, false]).show(ui, |ui| {
                        draw_detail(ui, app);
                    });
                });
            });
        });
    super::remember(ctx, app, super::WindowKey::Timeline);
    app.ui_state.panels.timeline_open = open;
}

/// The window's own controls. The transport itself — record, return to
/// start, the time, loop — is in the toolbar (`ui::transport`); this Play
/// runs the clock and leaves the camera as attached or detached as it is.
fn draw_header(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let playing = app.is_playing();
        let w = num::icon_button_width(ui, &[(icons::PLAY, "Play"), (icons::PAUSE, "Pause")]);
        let resp = ui.add(
            egui::Button::new(if playing {
                (icons::PAUSE, "Pause", egui::Atom::grow())
            } else {
                (icons::PLAY, "Play", egui::Atom::grow())
            })
            .min_size(egui::vec2(w, 0.0)),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Play or pause the timeline. Every track plays; the camera follows only if it \
             is attached (the link on the camera row). The toolbar's Play, and O, Z or \
             Space, fly the camera too: they attach it and play.",
            if playing { "click: pause" } else { "click: play from the playhead" },
        );
        if resp.clicked() {
            app.toggle_playback();
        }

        let snap = app.ui_state.timeline_snap;
        let resp = ui.add(egui::Button::selectable(snap, (icons::MAGNET, "Snap")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if snap {
                "Snapping: a dragged key, a new key, the loop's ends and the playhead land on \
                 other keys (any track's, and the camera's), the playhead, the loop's ends \
                 and the ruler's ticks when they come near. Hold Ctrl to drag freely. Click \
                 to turn it off."
            } else {
                "Snapping is off: keys go where you drop them. Hold Ctrl to snap for one \
                 drag. Click to turn it on."
            },
            if snap { "click: stop snapping" } else { "click: snap" },
        );
        if resp.clicked() {
            app.ui_state.timeline_snap = !snap;
        }

        let zoomed = app.ui_state.timeline_view.is_some();
        let resp = ui.add_enabled(zoomed, egui::Button::new((icons::CORNERS_OUT, "Whole")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if zoomed {
                "Show the whole timeline again"
            } else {
                "Showing the whole timeline. Ctrl+scroll over the lanes zooms in on the \
                 pointer; Shift+scroll or a middle-drag moves along."
            },
            "click: show the whole timeline",
        );
        if resp.clicked() {
            app.ui_state.timeline_view = None;
        }

        if let Some(who) = app.track_invalid() {
            ui.label(
                egui::RichText::new(format!("{} {who} has taken a value the scene can't use; holding the last good frame", icons::X))
                    .small()
                    .color(super::theme::warn(ui.visuals())),
            );
        } else if let Some(e) = app.track_error() {
            ui.label(egui::RichText::new(format!("{} {e}", icons::X)).small().color(super::theme::warn(ui.visuals())));
        } else if app.scene.tracks.is_empty() {
            ui.label(
                egui::RichText::new("No tracks yet: \"Animate a parameter\" on the right starts one, or arm record")
                    .small()
                    .weak(),
            );
        }
    });
}

/// Zoom and scroll the lanes in time, from the pointer over them: Ctrl+scroll
/// zooms about the moment under the pointer, Shift+scroll (or a sideways
/// scroll) moves along, a middle-drag drags the lanes. Read before the rows'
/// scroll area takes the wheel for itself.
fn navigate(ui: &egui::Ui, app: &mut App, area: egui::Rect, left: f32, width: f32) {
    let Some(p) = ui.ctx().pointer_hover_pos().filter(|p| area.contains(*p) && p.x >= left) else { return };
    let (zoom, scroll, shift, middle) = ui.input(|i| {
        (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers.shift, i.pointer.middle_down().then(|| i.pointer.delta().x))
    });
    let geo = geometry(app, left, width);
    let (t0, t1) = (geo.t0, geo.t1);
    let span = t1 - t0;
    let len = app.timeline_length().max(1.0);
    let mut view = (t0, t1);
    if (zoom - 1.0).abs() > 1e-4 {
        let at = geo.t(p.x);
        let new = (span / zoom as f64).clamp(MIN_SPAN, len * 8.0);
        let a = at - (at - t0) * new / span;
        view = (a, a + new);
    }
    // A sideways scroll, or Shift with a plain wheel (on platforms that don't
    // turn that into a sideways one already).
    let dx = if scroll.x.abs() > 0.0 { scroll.x } else if shift { scroll.y } else { 0.0 };
    let drag = middle.unwrap_or(0.0);
    let dx = dx + drag;
    if dx.abs() > 0.0 {
        let dt = -(dx / width) as f64 * (view.1 - view.0);
        view = (view.0 + dt, view.1 + dt);
    }
    if view != (t0, t1) {
        // Never before zero: the lanes slide back rather than show nothing.
        if view.0 < 0.0 {
            view = (0.0, view.1 - view.0);
        }
        app.ui_state.timeline_view = Some(view);
    }
}

/// The seconds the lanes show. Held still while something on them is being
/// dragged: the timeline's length follows its last key and the view pages by
/// the playhead, so a ruler that rescaled under a drag fed the key's own
/// motion back into where the pointer maps to (a key dragged left past the
/// playhead jumped right).
fn geometry(app: &mut App, left: f32, width: f32) -> Geo {
    let ui_state = &app.ui_state;
    let dragging = ui_state.timeline_drag_key.is_some()
        || ui_state.timeline_drag_camera.is_some()
        || ui_state.timeline_loop_drag.is_some();
    let (t0, t1) = match ui_state.timeline_span {
        Some(span) if dragging => span,
        _ => visible_span(app),
    };
    Geo { left, width, t0, t1 }
}

/// Zoomed in, the span you zoomed to, turned a page when playback runs off
/// its end (a jump, not a scroll: the chrome doesn't animate). Otherwise one
/// timeline's length, paged so the playhead is always on it.
fn visible_span(app: &mut App) -> (f64, f64) {
    let p = app.playhead();
    match app.ui_state.timeline_view {
        Some((a, b)) => {
            let span = b - a;
            if app.is_playing() && (p < a || p > b) {
                let a = p;
                app.ui_state.timeline_view = Some((a, a + span));
                (a, a + span)
            } else {
                (a, b)
            }
        }
        None => {
            let len = app.timeline_length().max(0.1);
            let page = (p / len).floor().max(0.0);
            (page * len, page * len + len)
        }
    }
}

/// `t` moved onto whatever is near it, with snapping on (or off and Ctrl
/// held): another key on any row, the camera's included; the playhead; the
/// loop's ends; the start and end of the timeline; the ruler's ticks. `own`
/// is the moment being dragged away from, which is no place to snap to.
fn snap(ui: &egui::Ui, app: &App, geo: &Geo, t: f64, own: Option<f64>, playhead: bool) -> f64 {
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    if app.ui_state.timeline_snap == ctrl {
        return t.max(0.0);
    }
    let mut near: Vec<f64> = app.key_times();
    if let Some(o) = own {
        near.retain(|k| (k - o).abs() > 1e-6);
    }
    near.extend([0.0, app.timeline_length()]);
    if playhead {
        near.push(app.playhead());
    }
    if let Some((a, b)) = app.loop_region {
        near.extend([a, b]);
    }
    let (_, minor) = geo.grid();
    near.push((t / minor).round() * minor);
    let x = geo.x(t);
    near.into_iter()
        .map(|c| (c, (geo.x(c) - x).abs()))
        .filter(|&(_, d)| d <= SNAP_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(t, |(c, _)| c)
        .max(0.0)
}

/// The ruler, pinned above the rows: the loop strip along its top, the ticks
/// under it. Press in the ticks to move the playhead; press in the strip to
/// draw a loop region, or grab its ends or its middle.
fn draw_ruler(ui: &mut egui::Ui, app: &mut App, geo: &Geo) {
    let full_w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(full_w, RULER_H), egui::Sense::click_and_drag());
    let lane = geo.lane(rect.top(), rect.bottom());
    let strip = geo.lane(rect.top(), rect.top() + LOOP_H);
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(lane, 0.0, visuals.extreme_bg_color);
    painter.rect_filled(strip, 0.0, visuals.faint_bg_color);
    let label = |y: f32, s: &str| {
        painter.text(
            egui::pos2(rect.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            s,
            egui::FontId::proportional(10.0),
            visuals.weak_text_color(),
        );
    };
    label(strip.center().y, "loop");
    label((strip.bottom() + rect.bottom()) / 2.0, &format!("seconds · {} fps", app.frame_rate()));

    // The loop region: lit while looping is on, a ghost while it's off.
    if let Some((a, b)) = app.loop_region {
        let r = egui::Rect::from_min_max(egui::pos2(geo.x(a), strip.top()), egui::pos2(geo.x(b), strip.bottom()))
            .intersect(strip);
        if r.width() > 0.0 {
            let c = visuals.selection.bg_fill;
            painter.rect_filled(r, 2.0, if app.loop_playback { c } else { c.gamma_multiply(0.35) });
        }
    }

    // Ticks: a labelled one where there's room, finer ones between.
    let (step, minor) = geo.grid();
    let font = egui::FontId::monospace(10.0);
    let mut i = (geo.t0 / minor).ceil() as i64;
    loop {
        let t = i as f64 * minor;
        if t > geo.t1 + 1e-9 {
            break;
        }
        let x = geo.x(t);
        let major = ((t / step).round() * step - t).abs() < minor * 0.01;
        let h = if major { 6.0 } else { 3.0 };
        painter.line_segment(
            [egui::pos2(x, lane.bottom() - h), egui::pos2(x, lane.bottom())],
            visuals.widgets.noninteractive.fg_stroke,
        );
        if major {
            let decimals = if step < 0.1 || step == 0.25 { 2 } else if step < 1.0 { 1 } else { 0 };
            painter.text(
                egui::pos2(x + 2.0, strip.bottom() + 1.0),
                egui::Align2::LEFT_TOP,
                num::fixed(t as f32, decimals),
                font.clone(),
                visuals.weak_text_color(),
            );
        }
        i += 1;
    }
    // Where the timeline ends, and the playhead.
    let end = geo.x(app.timeline_length());
    if end <= lane.right() {
        painter.line_segment([egui::pos2(end, strip.bottom()), egui::pos2(end, lane.bottom())], egui::Stroke::new(1.0, visuals.weak_text_color()));
    }
    let px = geo.x(app.playhead());
    if (lane.left()..=lane.right()).contains(&px) {
        let c = visuals.selection.stroke.color;
        painter.line_segment([egui::pos2(px, strip.bottom()), egui::pos2(px, lane.bottom())], egui::Stroke::new(1.5, c));
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(px - 5.0, strip.bottom()),
                egui::pos2(px + 5.0, strip.bottom()),
                egui::pos2(px, strip.bottom() + 6.0),
            ],
            c,
            egui::Stroke::NONE,
        ));
    }

    // A press in the strip works the loop; anywhere else on the ruler, the
    // playhead.
    let origin = ui.input(|i| i.pointer.press_origin());
    let in_strip = origin.is_some_and(|o| o.y < strip.bottom() && o.x >= lane.left());
    if resp.drag_started() && in_strip {
        if let Some(o) = origin {
            let t = geo.t(o.x);
            let near = |at: f64| (geo.x(at) - o.x).abs() <= LOOP_GRAB;
            app.ui_state.timeline_loop_drag = Some(match app.loop_region {
                Some((a, _)) if near(a) => LoopDrag::Start,
                Some((_, b)) if near(b) => LoopDrag::End,
                Some((a, b)) if (a..=b).contains(&t) => LoopDrag::Move { grab: t - a },
                _ => LoopDrag::Make { anchor: snap(ui, app, geo, t, None, true) },
            });
            app.ui_state.timeline_span = Some((geo.t0, geo.t1));
        }
    }
    if let Some(p) = resp.interact_pointer_pos() {
        match app.ui_state.timeline_loop_drag {
            Some(drag) if resp.dragged() => {
                let (a, b) = app.loop_region.unwrap_or((0.0, 0.0));
                let t = geo.t(p.x);
                let region = match drag {
                    LoopDrag::Start => (snap(ui, app, geo, t, Some(a), true), b),
                    LoopDrag::End => (a, snap(ui, app, geo, t, Some(b), true)),
                    LoopDrag::Move { grab } => {
                        let start = snap(ui, app, geo, (t - grab).max(0.0), Some(a), true);
                        (start, start + (b - a))
                    }
                    LoopDrag::Make { anchor } => (anchor, snap(ui, app, geo, t, None, true)),
                };
                if (region.1 - region.0).abs() > 1e-3 {
                    app.set_loop_region(Some(region));
                }
            }
            None if !in_strip && (resp.dragged() || resp.clicked()) => {
                app.scrub_to(snap(ui, app, geo, geo.t(p.x), None, false));
            }
            _ => {}
        }
    }
    if resp.drag_stopped() {
        app.ui_state.timeline_loop_drag = None;
    }

    // Right-click: set the loop's ends from here.
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            app.ui_state.timeline_menu_at = Some((usize::MAX, snap(ui, app, geo, geo.t(p.x), None, true), None));
        }
    }
    let hover_strip = ui.ctx().pointer_hover_pos().is_some_and(|p| p.y < strip.bottom());
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if hover_strip {
            "The loop strip. Drag along it to make a loop region; drag an end of the region \
             to move that end, or its middle to slide it. With loop on (the toolbar), \
             playback goes round inside it. Right-click for more."
        } else {
            "The ruler, in seconds. Click or drag to move the playhead: the tracks follow \
             it, and so does the camera if it is attached. Scrubbing isn't an edit. \
             Right-click to set the loop from here."
        },
        if hover_strip {
            "drag: make or move the loop region · right-click: loop menu"
        } else {
            "click or drag: move the playhead · right-click: loop menu"
        },
    );
    resp.context_menu(|ui| {
        let Some((usize::MAX, t, _)) = app.ui_state.timeline_menu_at else {
            ui.close();
            return;
        };
        ui.label(egui::RichText::new(format!("at {t:.2} s")).small().strong());
        let (a, b) = app.loop_region.unwrap_or((0.0, app.timeline_length()));
        for (label, region, tip) in [
            ("Loop from here", (t, b.max(t + 0.1)), "Start the loop region here"),
            ("Loop to here", (a.min(t - 0.1).max(0.0), t), "End the loop region here"),
        ] {
            let r = ui.button(label);
            let r = hinted(r, &mut app.ui_state, tip, "click: set the loop region");
            if r.clicked() {
                app.set_loop_region(Some(region));
                ui.close();
            }
        }
        let r = ui.add_enabled(app.loop_region.is_some(), egui::Button::new("Loop the whole timeline"));
        let r = hinted(r, &mut app.ui_state, "Clear the loop region: looping goes round the whole timeline", "click: clear the region");
        if r.clicked() {
            app.set_loop_region(None);
            ui.close();
        }
        let r = ui.button((icons::PLAY, "Move the playhead here"));
        let r = hinted(r, &mut app.ui_state, "Put the playhead at this moment", "click: move the playhead");
        if r.clicked() {
            app.scrub_to(t);
            ui.close();
        }
    });
}

/// The camera's row and a row per track, under the ruler.
fn draw_rows(ui: &mut egui::Ui, app: &mut App, geo: &Geo) {
    let full_w = ui.available_width();
    let row_count = 1 + app.scene.tracks.len();
    let lanes_h = ROW_H * row_count as f32;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(full_w, lanes_h), egui::Sense::click_and_drag());
    let lane = geo.lane(rect.top(), rect.bottom());
    let x_of = |t: f64| geo.x(t);

    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(lane, 0.0, visuals.extreme_bg_color);
    // The loop region, faintly, down every lane.
    if let Some((a, b)) = app.active_loop_region() {
        let r = egui::Rect::from_min_max(egui::pos2(geo.x(a), lane.top()), egui::pos2(geo.x(b), lane.bottom())).intersect(lane);
        if r.width() > 0.0 {
            painter.rect_filled(r, 0.0, visuals.selection.bg_fill.gamma_multiply(0.12));
        }
    }

    // Rows: the camera, then each track, named as the parameter picker names
    // them — the -S path only where the scene no longer has the parameter.
    let names = track_names(app);
    let mut hovered_row = None;
    let pointer = ui.ctx().pointer_hover_pos();
    for row in 0..row_count {
        let top = rect.top() + row as f32 * ROW_H;
        let row_rect = egui::Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(full_w, ROW_H));
        let lane_row = geo.lane(top, top + ROW_H);
        if row % 2 == 1 {
            painter.rect_filled(lane_row, 0.0, visuals.faint_bg_color);
        }
        if pointer.is_some_and(|p| row_rect.contains(p)) {
            hovered_row = Some(row);
        }
        let selected = if row == 0 {
            app.ui_state.timeline_camera
        } else {
            app.ui_state.timeline_track == Some(row - 1)
        };
        // The selection marks the row's name, not its lane: the lanes are a
        // drawing surface for keys and curves, and a tint across one sits
        // over the very things being edited in it.
        if selected {
            let name = egui::Rect::from_min_max(row_rect.min, egui::pos2(lane_row.left() - 2.0, row_rect.max.y));
            painter.rect_filled(name.shrink2(egui::vec2(0.0, 1.0)), 2.0, visuals.selection.bg_fill);
        }
        if row == 0 {
            draw_camera_row(&painter, app, row_rect, lane_row, &x_of, &visuals);
        } else {
            let key = if selected { app.ui_state.timeline_key } else { None };
            draw_track_row(&painter, &app.scene.tracks[row - 1], &names[row - 1], row_rect, lane_row, (geo.t0, geo.t1), &x_of, &visuals, selected, key);
        }
    }

    // The end of the timeline, and the playhead, over everything.
    let end = geo.x(app.timeline_length());
    if end <= lane.right() {
        painter.line_segment([egui::pos2(end, lane.top()), egui::pos2(end, lane.bottom())], egui::Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.6)));
    }
    let px = geo.x(app.playhead());
    painter.line_segment(
        [egui::pos2(px, lane.top()), egui::pos2(px, lane.bottom())],
        egui::Stroke::new(1.5, visuals.selection.stroke.color),
    );

    // A press on a row selects it — a track's, and the diamond under the
    // pointer, which a drag then retimes; or the camera's — and moves
    // nothing else, so picking a track never shifts the camera. Only the
    // ruler moves the playhead.
    let row_at = |y: f32| -> Option<usize> {
        let r = ((y - rect.top()) / ROW_H).floor();
        (r >= 0.0 && (r as usize) < row_count).then_some(r as usize)
    };
    // The key under `p` on `row`: a camera key, or a track's.
    let key_at = |app: &App, row: usize, p: egui::Pos2| -> Option<usize> {
        let near = |t: f64| (x_of(t) - p.x).abs();
        if row == 0 {
            if app.path_is_default() {
                return None;
            }
            camera_key_times(app, lane, &x_of)
                .into_iter()
                .map(|o| (o.key, near(o.t)))
                .filter(|&(_, d)| d <= KEY_GRAB)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k)
        } else {
            let Source::Keys { keys, .. } = &app.scene.tracks.get(row - 1)?.source else { return None };
            keys.iter()
                .enumerate()
                .map(|(k, key)| (k, near(key.t)))
                .filter(|&(_, d)| d <= KEY_GRAB)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k)
        }
    };
    let primary = !ui.input(|i| i.pointer.middle_down());
    if primary && (resp.drag_started() || resp.clicked() || resp.secondary_clicked()) {
        // Where the press *began*: by the time egui calls it a drag the
        // pointer is already past its threshold, and a diamond is smaller.
        let origin = if resp.secondary_clicked() { None } else { ui.input(|i| i.pointer.press_origin()) };
        if let Some(p) = origin.or(resp.interact_pointer_pos()) {
            app.ui_state.timeline_drag_key = None;
            app.ui_state.timeline_drag_camera = None;
            let row = row_at(p.y);
            let hit = row.and_then(|r| key_at(app, r, p));
            match row {
                Some(0) => {
                    app.ui_state.timeline_camera = true;
                    app.ui_state.timeline_track = None;
                    app.ui_state.timeline_key = None;
                    app.ui_state.timeline_camera_key = hit;
                    // Only a key's own diamond drags, not its repeats on
                    // later laps.
                    let own = hit.filter(|&k| {
                        camera_key_times(app, lane, &x_of).iter().any(|o| o.key == k && o.primary && (x_of(o.t) - p.x).abs() <= KEY_GRAB)
                    });
                    if let Some(k) = own.filter(|_| !resp.secondary_clicked()) {
                        app.ui_state.timeline_drag_camera = Some(k);
                        app.ui_state.timeline_span = Some((geo.t0, geo.t1));
                    }
                }
                Some(r) => {
                    let i = r - 1;
                    app.ui_state.timeline_camera = false;
                    if app.ui_state.timeline_track != Some(i) || hit.is_some() {
                        app.ui_state.timeline_key = hit;
                    }
                    app.ui_state.timeline_track = Some(i);
                    if let Some(k) = hit.filter(|_| !resp.secondary_clicked()) {
                        app.ui_state.timeline_drag_key = Some((i, k));
                        app.ui_state.timeline_span = Some((geo.t0, geo.t1));
                    }
                }
                None => {}
            }
            if resp.secondary_clicked() {
                app.ui_state.timeline_menu_at = row.map(|r| (r, snap(ui, app, geo, geo.t(p.x), None, true), hit));
            }
        }
    }
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() && primary {
            match (app.ui_state.timeline_drag_key, app.ui_state.timeline_drag_camera) {
                (Some((i, k)), _) => {
                    let own = match &app.scene.tracks.get(i).map(|t| &t.source) {
                        Some(Source::Keys { keys, .. }) => keys.get(k).map(|key| key.t),
                        _ => None,
                    };
                    let t = snap(ui, app, geo, geo.t(p.x), own, true);
                    app.set_key_time(i, k, t);
                }
                (None, Some(k)) => {
                    let own = Some(app.camera_path().time(k) as f64);
                    let t = snap(ui, app, geo, geo.t(p.x), own, true);
                    app.set_camera_key_time(k, t);
                }
                (None, None) => {}
            }
        }
    }
    if resp.drag_stopped() {
        app.ui_state.timeline_drag_key = None;
        app.ui_state.timeline_drag_camera = None;
    }
    let row_hint = match hovered_row {
        Some(0) => "the camera · click a diamond: select that view · drag it: retime it · right-click: key here, or that view's menu".to_string(),
        Some(r) => format!("{} · click: select · drag a diamond: retime it · right-click: key here, or that key's menu", names[r - 1]),
        None => String::new(),
    };
    let tooltip = match hovered_row {
        Some(0) => camera_description(app),
        Some(r) => format!("{}\n{}", names[r - 1], track_description(&app.scene.tracks[r - 1])),
        None => String::new(),
    };
    let resp = hinted(resp, &mut app.ui_state, tooltip, &row_hint);

    // Per-row controls in the label column, drawn as real widgets over the
    // painted rows so each gets its own hint: the camera's attach link, and
    // each track's mute eye.
    {
        let link_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + 2.0, rect.top() + 1.0), egui::vec2(20.0, ROW_H - 2.0));
        let attached = app.camera_attached();
        let link = ui.put(link_rect, egui::Button::new(if attached { icons::ATTACHED } else { icons::DETACHED }).frame(false));
        let link = hinted(
            link,
            &mut app.ui_state,
            if attached {
                "Attached: the camera follows the timeline, playing and scrubbing. Click to \
                 detach it and keep the framing you have."
            } else {
                "Detached: the camera is yours, and playing or scrubbing leaves it where it \
                 is. Click to attach it to its path on the timeline. (Taking the camera in \
                 the viewport detaches it.)"
            },
            "click: attach / detach the camera",
        );
        if link.clicked() {
            app.set_camera_attached(!attached);
        }
    }
    for i in 0..app.scene.tracks.len() {
        let top = rect.top() + (i + 1) as f32 * ROW_H;
        let eye_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + 2.0, top + 1.0), egui::vec2(20.0, ROW_H - 2.0));
        let muted = app.scene.tracks[i].mute;
        let eye = ui.put(eye_rect, egui::Button::new(if muted { icons::EYE_SLASH } else { icons::EYE }).frame(false));
        let eye = hinted(
            eye,
            &mut app.ui_state,
            if muted {
                "Muted: kept in the file and drawn here, but not applied. Click to unmute."
            } else {
                "Click to mute this track: the value goes back to what the scene authors \
                 while the track stays in the file"
            },
            "click: mute / unmute this track",
        );
        if eye.clicked() {
            app.set_track_muted(i, !muted);
        }
    }

    resp.context_menu(|ui| {
        match app.ui_state.timeline_menu_at {
            Some((0, t, key)) => camera_menu(ui, app, t, key),
            Some((r, t, key)) if r != usize::MAX && r - 1 < app.scene.tracks.len() => {
                let name = names.get(r - 1).cloned().unwrap_or_default();
                track_menu(ui, app, r - 1, &name, t, key);
            }
            _ => ui.close(),
        }
    });
}

/// Right-click on the camera's row: key it here, or work on the view under
/// the pointer.
fn camera_menu(ui: &mut egui::Ui, app: &mut App, t: f64, key: Option<usize>) {
    match key {
        Some(k) => {
            let p = app.camera_path().clone();
            if k >= p.keys.len() {
                ui.close();
                return;
            }
            ui.label(egui::RichText::new(format!("View {} of {}, at {:.2} s", k + 1, p.keys.len(), p.time(k))).small().strong());
            let r = ui.button((icons::CAMERA, "Go to this view"));
            let r = hinted(r, &mut app.ui_state, "Put the playhead on this view, with the camera attached", "click: go to this view");
            if r.clicked() {
                app.scrub_to(p.time(k) as f64);
                app.set_camera_attached(true);
                ui.close();
            }
            let r = ui.button((icons::REFRESH, "Re-key with this framing"));
            let r = hinted(r, &mut app.ui_state, "Replace this view with the framing on screen, at the same time", "click: re-key this view");
            if r.clicked() {
                app.rekey_camera(k);
                ui.close();
            }
            let r = ui.button((icons::TRASH, "Delete this view"));
            let r = hinted(r, &mut app.ui_state, "Remove this view from the camera's track. Ctrl+Z brings it back.", "click: delete this view");
            if r.clicked() {
                app.remove_path_key_at(k);
                app.ui_state.timeline_camera_key = None;
                ui.close();
            }
        }
        None => {
            ui.label(egui::RichText::new(format!("The camera, at {t:.2} s")).small().strong());
            let r = ui.button((icons::PLUS, "Key the camera here"));
            let r = hinted(
                r,
                &mut app.ui_state,
                "Make the framing on screen the camera's view at this moment",
                "click: key this framing here",
            );
            if r.clicked() {
                app.key_camera_at(t, None);
                ui.close();
            }
        }
    }
}

/// Right-click on a track's row: a key here, or the key under the pointer's
/// copy, paste, duplicate and delete; then the track's own mute and delete.
fn track_menu(ui: &mut egui::Ui, app: &mut App, i: usize, name: &str, t: f64, key: Option<usize>) {
    let Some(Track { source, mute, .. }) = app.scene.tracks.get(i).cloned() else {
        ui.close();
        return;
    };
    ui.label(egui::RichText::new(name).small().strong());
    let keys = match &source {
        Source::Keys { keys, .. } => Some(keys.clone()),
        _ => None,
    };
    let clip = app.ui_state.timeline_clip.clone();
    // A copied value goes only onto a track keyed with the same kind.
    let pastes = |keys: &[crate::track::Key]| {
        clip.as_ref().is_some_and(|c| keys.iter().all(|k| std::mem::discriminant(&k.value) == std::mem::discriminant(c)))
    };
    match (&keys, key) {
        (Some(keys), Some(k)) if k < keys.len() => {
            let at = keys[k].t;
            ui.label(egui::RichText::new(format!("key at {at:.2} s")).small().weak());
            let r = ui.button((icons::COPY, "Copy"));
            let r = hinted(r, &mut app.ui_state, "Copy this key's value, to paste onto another moment or another track like it", "click: copy this key");
            if r.clicked() {
                app.ui_state.timeline_clip = Some(keys[k].value.clone());
                ui.close();
            }
            let r = ui.add_enabled(pastes(keys), egui::Button::new("Paste over it"));
            let r = hinted(
                r,
                &mut app.ui_state,
                if clip.is_some() { "Give this key the copied value" } else { "Copy a key first" },
                "click: paste the copied value here",
            );
            if r.clicked() {
                app.add_key_at(i, at, clip.clone());
                ui.close();
            }
            let here = app.playhead();
            let r = ui.add_enabled((here - at).abs() > 1e-3, egui::Button::new((icons::PLUS, format!("Duplicate at the playhead ({here:.2} s)"))));
            let r = hinted(r, &mut app.ui_state, "A copy of this key at the playhead", "click: duplicate this key");
            if r.clicked() {
                if let Some(k) = app.add_key_at(i, here, Some(keys[k].value.clone())) {
                    app.ui_state.timeline_key = Some(k);
                }
                ui.close();
            }
            let r = ui.button((icons::TRASH, "Delete key"));
            let r = hinted(r, &mut app.ui_state, "Delete this key (the last one takes the track with it). Ctrl+Z brings it back.", "click: delete this key");
            if r.clicked() {
                app.delete_key(i, k);
                app.ui_state.timeline_key = None;
                ui.close();
            }
        }
        (Some(keys), _) => {
            ui.label(egui::RichText::new(format!("at {t:.2} s")).small().weak());
            let r = ui.button((icons::PLUS, "Add a key here"));
            let r = hinted(r, &mut app.ui_state, "A key here holding the value the track has at this moment, so nothing changes until you set it", "click: key here");
            if r.clicked() {
                if let Some(k) = app.add_key_at(i, t, None) {
                    app.ui_state.timeline_key = Some(k);
                }
                ui.close();
            }
            let r = ui.add_enabled(pastes(keys), egui::Button::new("Paste key here"));
            let r = hinted(
                r,
                &mut app.ui_state,
                if clip.is_some() { "A key here with the copied value" } else { "Copy a key first: right-click one" },
                "click: paste a key here",
            );
            if r.clicked() {
                if let Some(k) = app.add_key_at(i, t, clip.clone()) {
                    app.ui_state.timeline_key = Some(k);
                }
                ui.close();
            }
        }
        (None, _) => {
            ui.label(egui::RichText::new("A rate or a cycle: its numbers are on the right").small().weak());
        }
    }
    ui.separator();
    let r = ui.button(if mute { (icons::EYE, "Unmute track") } else { (icons::EYE_SLASH, "Mute track") });
    let r = hinted(r, &mut app.ui_state, "Mute or unmute this track", "click: mute / unmute");
    if r.clicked() {
        app.set_track_muted(i, !mute);
        ui.close();
    }
    let r = ui.button((icons::TRASH, "Delete track"));
    let r = hinted(
        r,
        &mut app.ui_state,
        "Remove this track from the scene; the value goes back to what the scene authors. \
         Ctrl+Z brings it back.",
        "click: delete this track",
    );
    if r.clicked() {
        app.delete_track(i);
        ui.close();
    }
}

fn draw_camera_row(
    painter: &egui::Painter,
    app: &App,
    row: egui::Rect,
    lane: egui::Rect,
    x_of: &dyn Fn(f64) -> f32,
    visuals: &egui::Visuals,
) {
    let attached = app.camera_attached();
    painter.text(
        egui::pos2(row.left() + 24.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        if attached { "camera".to_string() } else { "camera (detached)".to_string() },
        egui::FontId::proportional(13.0),
        if app.ui_state.timeline_camera {
            visuals.selection.stroke.color
        } else if attached {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        },
    );
    if app.path_is_default() {
        painter.text(
            egui::pos2(lane.left() + 6.0, lane.center().y),
            egui::Align2::LEFT_CENTER,
            format!("default orbit: one turn every {:.0}s", app.camera_path().duration()),
            egui::FontId::proportional(11.0),
            visuals.weak_text_color(),
        );
        return;
    }
    let colour = visuals.widgets.active.fg_stroke.color;
    let colour = if attached { colour } else { colour.gamma_multiply(0.4) };
    let selected = app.ui_state.timeline_camera.then_some(app.ui_state.timeline_camera_key).flatten();
    for o in camera_key_times(app, lane, x_of) {
        let c = if Some(o.key) == selected { visuals.selection.stroke.color } else { colour };
        // Repeats of a key on later laps, or on a ping-pong's way back, are
        // drawn fainter: they are the same key seen again, and only the
        // first is dragged.
        diamond(painter, egui::pos2(x_of(o.t), lane.center().y), if o.primary { c } else { c.gamma_multiply(0.5) });
    }
}

/// One moment the camera is on one of its keys.
#[derive(Clone, Copy)]
struct CameraKeyAt {
    t: f64,
    key: usize,
    /// The key's own time, rather than a repeat on a later lap or on a
    /// ping-pong's way back: the one a drag retimes.
    primary: bool,
}

/// When the camera is on each of its keys, over the visible span: each key's
/// own time, and again on every later lap of a loop, and mirrored on a
/// ping-pong's way back.
fn camera_key_times(app: &App, lane: egui::Rect, x_of: &dyn Fn(f64) -> f32) -> Vec<CameraKeyAt> {
    let p = app.camera_path();
    let times: Vec<f64> = p.key_times().iter().map(|&t| t as f64).collect();
    let start = p.start() as f64;
    let lap = p.lap() as f64;
    let pong = p.loops == crate::path::Loop::PingPong;
    let cycle = p.duration() as f64;
    let mut out = Vec::new();
    let laps = if p.wraps() { 64 } else { 1 };
    for l in 0..laps {
        let base = l as f64 * cycle;
        let mut moments: Vec<(f64, usize, bool)> =
            times.iter().enumerate().map(|(k, &t)| (base + t, k, l == 0)).collect();
        if pong {
            let n = times.len();
            moments.extend(
                times.iter().enumerate().rev().skip(1).take(n.saturating_sub(2)).map(|(k, &t)| {
                    (base + start + 2.0 * lap - (t - start), k, false)
                }),
            );
        }
        for (t, key, primary) in moments {
            let x = x_of(t);
            if x > lane.right() + 1.0 {
                return out;
            }
            if x >= lane.left() - 1.0 {
                out.push(CameraKeyAt { t, key, primary });
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn draw_track_row(
    painter: &egui::Painter,
    track: &Track,
    name: &str,
    row: egui::Rect,
    lane: egui::Rect,
    (t0, t1): (f64, f64),
    x_of: &dyn Fn(f64) -> f32,
    visuals: &egui::Visuals,
    selected: bool,
    selected_key: Option<usize>,
) {
    let text = if selected {
        visuals.selection.stroke.color
    } else if track.mute {
        visuals.weak_text_color()
    } else {
        visuals.text_color()
    };
    // Shortened from the left: the end of a name is the part that says what
    // moves.
    let label = shorten_left(name, 30);
    painter.text(
        egui::pos2(row.left() + 24.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        text,
    );
    let colour = class_colour(track);
    let colour = if track.mute { colour.gamma_multiply(0.35) } else { colour };

    // The value as a faint curve, scaled to the row. Rates and cycles move
    // about the authored value; the shape doesn't depend on it, so any base
    // will draw it.
    let n = (lane.width() / 3.0).max(8.0) as usize;
    let samples: Vec<(f64, f64)> = (0..=n)
        .filter_map(|i| {
            let t = t0 + (t1 - t0) * i as f64 / n as f64;
            match track.sample(t, Some(0.0)) {
                KeyValue::Number(v) => Some((t, v)),
                // A pose isn't one number; its diamonds are the picture.
                KeyValue::Rotation(_) | KeyValue::Other(_) => None,
            }
        })
        .collect();
    if samples.len() > 1 {
        let (lo, hi) = samples.iter().fold((f64::MAX, f64::MIN), |(a, b), &(_, v)| (a.min(v), b.max(v)));
        let pad = 4.0;
        let y_of = |v: f64| {
            if hi - lo < 1e-12 {
                lane.center().y
            } else {
                lane.bottom() - pad - ((v - lo) / (hi - lo)) as f32 * (lane.height() - 2.0 * pad)
            }
        };
        let points: Vec<egui::Pos2> = samples.iter().map(|&(t, v)| egui::pos2(x_of(t), y_of(v))).collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, colour.gamma_multiply(0.45))));
    }
    if let Source::Keys { keys, .. } = &track.source {
        for (i, k) in keys.iter().enumerate() {
            let x = x_of(k.t);
            if x >= lane.left() - 1.0 && x <= lane.right() + 1.0 {
                let c = egui::pos2(x, lane.center().y);
                if selected_key == Some(i) {
                    painter.circle_stroke(c, 7.0, egui::Stroke::new(1.5, visuals.selection.stroke.color));
                }
                diamond(painter, c, colour);
            }
        }
    }
}

fn diamond(painter: &egui::Painter, c: egui::Pos2, colour: egui::Color32) {
    let r = 4.5;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x, c.y - r),
            egui::pos2(c.x + r, c.y),
            egui::pos2(c.x, c.y + r),
            egui::pos2(c.x - r, c.y),
        ],
        colour,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(160)),
    ));
}

/// A track's colour says what it costs: view, colour, placement, structure.
fn class_colour(track: &Track) -> egui::Color32 {
    use crate::track::Class;
    match track.class() {
        Class::View => egui::Color32::from_rgb(120, 180, 230),
        Class::Colour => egui::Color32::from_rgb(225, 190, 90),
        Class::Placement => egui::Color32::from_rgb(150, 210, 140),
        Class::Structure => egui::Color32::from_rgb(230, 120, 120),
    }
}

fn shorten_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(n - (max - 1)).collect::<String>())
    }
}

fn camera_description(app: &App) -> String {
    let p = app.camera_path();
    if app.path_is_default() {
        format!(
            "The camera flies the default orbit, one turn every {:.0}s, around wherever you \
             leave it. Add keypoints (Y) to give the scene a path of its own; they appear \
             here as diamonds.",
            p.duration()
        )
    } else {
        format!(
            "The camera's path: {} keypoints, {:.1}s, {}. Each diamond is the moment the \
             camera passes a key. Edit it in the Camera window.",
            p.keys.len(),
            p.duration(),
            p.loops.kind().label()
        )
    }
}

fn track_description(t: &Track) -> String {
    let what = match &t.source {
        Source::Keys { keys, interpolate, extrapolate, .. } => format!(
            "{} keys from {:.2}s to {:.2}s, {} between them, {} past the ends",
            keys.len(),
            keys.first().map_or(0.0, |k| k.t),
            keys.last().map_or(0.0, |k| k.t),
            interpolate.as_str(),
            extrapolate.as_str()
        ),
        Source::Rate { rate, from } => match from {
            Some(f) => format!("runs at {rate} a second from {f}"),
            None => format!("runs at {rate} a second from the scene's own value"),
        },
        Source::Cycle { wave, period, amplitude, .. } => format!(
            "a {} wave every {period}s, {amplitude} either side of the scene's value",
            wave.as_str()
        ),
    };
    let cost = match t.class() {
        crate::track::Class::View => "view: re-rendered each frame, nothing refills",
        crate::track::Class::Colour => "colour: recoloured each frame",
        crate::track::Class::Placement | crate::track::Class::Structure => {
            "moves the attractor: the window churns its points faster while this plays, \
             and an animation refills them every frame"
        }
    };
    format!(
        "\n{}.\n{}.{}\n\n(-S {})",
        what,
        cost,
        if t.mute { "\nMuted." } else { "" },
        t.target,
    )
}


/// The selected track's settings, and the new-track field at the foot.
fn draw_detail(ui: &mut egui::Ui, app: &mut App) {
    match app.ui_state.timeline_track {
        Some(i) => draw_track_detail(ui, app, i),
        None if app.ui_state.timeline_camera => draw_camera_detail(ui, app),
        None => {
            ui.label(
                egui::RichText::new(
                    "Click a track's row to edit it, or a diamond to edit that key.",
                )
                .small()
                .weak(),
            );
        }
    }
    ui.add_space(6.0);
    ui.separator();
    draw_new_track(ui, app);
}

fn draw_track_detail(ui: &mut egui::Ui, app: &mut App, i: usize) {
    let track = app.scene.tracks[i].clone();
    let name = track_names(app).swap_remove(i);
    let resp = ui.label(egui::RichText::new(name).strong());
    hinted(resp, &mut app.ui_state, format!("-S {}", track.target), "the parameter this track animates");
    let mut source = track.source.clone();
    match &mut source {
        Source::Keys { keys, interpolate, extrapolate, .. } => {
            let keys = keys.clone();
            let numeric = keys.iter().all(|k| matches!(k.value, KeyValue::Number(_)));
            ui.horizontal_wrapped(|ui| {
                let mut r = super::radio::radio(&mut app.ui_state, "track_interp", *interpolate);
                for (v, text, tip) in [
                    (Interp::Step, "step", "Hold each key's value until the next key"),
                    (Interp::Linear, "linear", "A straight line from key to key"),
                    (Interp::Smooth, "smooth", "Ease out of every key and into the next: still at each key"),
                    (Interp::Spline, "spline", "A smooth curve through every key that doesn't stop at them"),
                ] {
                    let ok = numeric || v == Interp::Step;
                    r = r.option_enabled(
                        ok,
                        v,
                        text,
                        if ok { tip.to_string() } else { format!("{tip}. These keys aren't numbers, so they can only step.") },
                        "click: how the value runs between keys",
                    );
                }
                if let Some(v) = r.show(ui) {
                    *interpolate = v;
                }
            });
            ui.horizontal_wrapped(|ui| {
                let mut r = super::radio::radio(&mut app.ui_state, "track_extrap", *extrapolate);
                for (v, text, tip) in [
                    (Extrapolate::Hold, "hold", "Hold the first and last values before and after the keys"),
                    (Extrapolate::Repeat, "repeat", "Play the keys again from the first, forever"),
                    (Extrapolate::PingPong, "ping-pong", "Forwards, then backwards, forever"),
                    (Extrapolate::Offset, "offset", "Repeat, each lap starting where the last ended: a value that keeps climbing"),
                ] {
                    let ok = numeric || v != Extrapolate::Offset;
                    r = r.option_enabled(ok, v, text, tip, "click: what happens past the last key");
                }
                if let Some(v) = r.show(ui) {
                    *extrapolate = v;
                }
            });
            // Only the two radios are this arm's to set: the key buttons
            // below edit the track's keys directly, and must not be undone by
            // a stale copy of them.
            let (ip, ex) = (*interpolate, *extrapolate);
            if let Source::Keys { interpolate, extrapolate, .. } = &track.source {
                if (*interpolate, *extrapolate) != (ip, ex) {
                    let mut fresh = track.source.clone();
                    if let Source::Keys { interpolate, extrapolate, .. } = &mut fresh {
                        *interpolate = ip;
                        *extrapolate = ex;
                    }
                    app.set_track_source(i, fresh);
                }
            }

            // The selected key, or a hint to select one.
            let key = app.ui_state.timeline_key.filter(|&k| k < keys.len());
            ui.horizontal(|ui| {
                let resp = ui.button((icons::PLUS, "Key at playhead"));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Add a key at the playhead with the value the track has there, so the \
                     curve doesn't change; then set its value below",
                    "click: add a key at the playhead",
                );
                if resp.clicked() {
                    if let Some(k) = app.add_key_at_playhead(i) {
                        app.ui_state.timeline_key = Some(k);
                    }
                }
                if keys.iter().any(|k| matches!(k.value, KeyValue::Rotation(_))) {
                    let resp = ui.button((icons::PLUS, "Key the map's rotation"));
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Key the rotation the map has in the Transforms window (or after a gizmo \
                         drag) at the playhead: pose the map, then key it here",
                        "click: key the map's own rotation at the playhead",
                    );
                    if resp.clicked() {
                        if let Some(k) = app.key_rotation_from_scene(i) {
                            app.ui_state.timeline_key = Some(k);
                        }
                    }
                }
                let resp = ui.add_enabled(key.is_some(), egui::Button::new((icons::TRASH, "Delete key")));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    if key.is_some() {
                        "Delete the selected key (the last one takes the track with it). Ctrl+Z \
                         brings it back."
                    } else {
                        "Select a key first: click its diamond"
                    },
                    "click: delete the selected key",
                );
                if resp.clicked() {
                    if let Some(k) = key {
                        app.delete_key(i, k);
                        app.ui_state.timeline_key = None;
                    }
                }
            });
            if let Some(k) = key {
                let (mut t, value) = (keys[k].t, keys[k].value.clone());
                ui.horizontal(|ui| {
                    ui.label("key at");
                    let resp = ui.add(num::Drag::new(&mut t).speed(0.02).range(0.0..=3600.0).fixed_decimals(2).suffix(" s"));
                    let resp = hinted(resp, &mut app.ui_state, "When this key is, in seconds", "drag: retime this key · click: type a time");
                    if resp.changed() {
                        app.set_key_time(i, k, t);
                    }
                    match value {
                        KeyValue::Number(mut v) => {
                            ui.label("value");
                            let resp = ui.add(num::Drag::new(&mut v).speed(0.01).max_decimals(4));
                            let resp = hinted(resp, &mut app.ui_state, "The value at this key", "drag: change the key's value · click: type one");
                            if resp.changed() {
                                app.set_key_value(i, k, v);
                            }
                        }
                        KeyValue::Rotation(r) => {
                            let mut e = r.euler.unwrap_or_else(|| crate::track::euler_of(r.orientation, None));
                            let mut changed = false;
                            for (c, axis) in ["x", "y", "z"].iter().enumerate() {
                                ui.label(*axis);
                                let resp = ui.add(num::Drag::new(&mut e[c]).speed(0.5).max_decimals(2).suffix("°"));
                                let resp = hinted(
                                    resp,
                                    &mut app.ui_state,
                                    "The map's rotation at this key, in degrees (XYZ, as its own fields \
                                     write it). Change one angle from the key before and the map turns \
                                     by exactly that much about it on the way: 360 more is a whole turn.",
                                    "drag: turn this key · click: type an angle",
                                );
                                changed |= resp.changed();
                            }
                            if changed {
                                app.set_key_rotation(i, k, e);
                            }
                        }
                        KeyValue::Other(v) => {
                            let resp = ui.label(egui::RichText::new(format!("= {v}")).monospace());
                            hinted(
                                resp,
                                &mut app.ui_state,
                                "Strings and booleans step from key to key; change them in the scene file",
                                "a step key's value",
                            );
                        }
                    }
                });
            }
        }
        Source::Rate { rate, .. } => {
            ui.horizontal(|ui| {
                ui.label("rate");
                let resp = ui.add(num::Drag::new(rate).speed(0.005).max_decimals(4).suffix(" /s"));
                hinted(resp, &mut app.ui_state, "How much the value changes a second, from the scene's own value", "drag: the rate");
            });
            app.set_track_source(i, source.clone());
        }
        Source::Cycle { wave, period, amplitude, phase, .. } => {
            ui.horizontal_wrapped(|ui| {
                let mut r = super::radio::radio(&mut app.ui_state, "track_wave", *wave);
                for (v, tip) in [
                    (Wave::Sine, "A smooth swing either side"),
                    (Wave::Triangle, "Straight up, straight down"),
                    (Wave::Saw, "Rises steadily, then drops back at once"),
                    (Wave::Square, "Jumps between the two ends"),
                ] {
                    r = r.option(v, v.as_str(), tip, "click: the wave's shape");
                }
                if let Some(v) = r.show(ui) {
                    *wave = v;
                }
            });
            egui::Grid::new("track_cycle").num_columns(2).show(ui, |ui| {
                ui.label("period");
                let resp = ui.add(num::Drag::new(period).speed(0.05).range(0.05..=3600.0).fixed_decimals(2).suffix(" s"));
                hinted(resp, &mut app.ui_state, "Seconds for one whole swing", "drag: the period");
                ui.end_row();
                ui.label("amplitude");
                let resp = ui.add(num::Drag::new(amplitude).speed(0.01).max_decimals(4));
                hinted(resp, &mut app.ui_state, "How far it swings either side of the scene's own value", "drag: the amplitude");
                ui.end_row();
                ui.label("phase");
                let resp = ui.add(num::Drag::new(phase).speed(0.005).range(0.0..=1.0).fixed_decimals(3));
                hinted(resp, &mut app.ui_state, "Where in its swing it starts, in periods (0.25 is a quarter of the way)", "drag: the phase");
                ui.end_row();
            });
            app.set_track_source(i, source.clone());
        }
    }
}

fn draw_camera_detail(ui: &mut egui::Ui, app: &mut App) {
    ui.label(egui::RichText::new("Camera").strong());
    let attached = app.camera_attached();
    let resp = ui.add(egui::Button::new(if attached {
        (icons::ATTACHED, "Attached to the timeline")
    } else {
        (icons::DETACHED, "Detached")
    }).selected(attached));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Attached, the camera flies its path on the timeline's clock, playing and \
         scrubbing. Detached, it stays where you put it while the tracks play.",
        "click: attach / detach the camera",
    );
    if resp.clicked() {
        app.set_camera_attached(!attached);
    }
    let authored = !app.path_is_default() || app.scene.camera_path.is_some();
    let flying_default = app.path_is_default();
    super::camera_panel::draw_path_controls(ui, app, flying_default, authored);

    // Loops come back round after their last key: how long that takes is the
    // lap, and the closing segment is whatever it leaves.
    let p = app.camera_path().clone();
    if authored && p.loops.closes() {
        let mut lap = p.lap();
        ui.horizontal(|ui| {
            ui.label("lap");
            let resp = ui.add(num::Drag::new(&mut lap).speed(0.05).range(0.1..=3600.0).suffix("s"));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "How long one lap of the loop takes, from the first key round to being back \
                 there. The way home from the last key gets whatever is left.",
                "drag: set the loop's lap",
            );
            if resp.changed() {
                app.set_camera_period(lap);
            }
        });
    }

    ui.add_space(4.0);
    let resp = ui.button((icons::PLUS, "Key the camera at the playhead"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Make the framing you're looking at the camera's view at the playhead, replacing \
         a key already there. Two keys make a camera track of the scene's own instead of \
         the default orbit. (Y adds one after the last key instead.)",
        "click: key this framing at the playhead",
    );
    if resp.clicked() {
        app.key_camera_at_playhead(None);
    }
    let views = app.saved_views().to_vec();
    ui.add_enabled_ui(!views.is_empty(), |ui| {
        let resp = ui.menu_button("Key a saved view at the playhead…", |ui| {
            for (label, path) in &views {
                let r = ui.button(label);
                let r = hinted(r, &mut app.ui_state, path.display().to_string(), "click: key this view here");
                if r.clicked() {
                    app.key_camera_at_playhead(Some(path));
                    ui.close();
                }
            }
        });
        hinted(
            resp.response,
            &mut app.ui_state,
            if views.is_empty() {
                "No saved views for this scene yet: V saves the framing on screen as one"
            } else {
                "Put one of this scene's saved views (V) on the camera's track at the playhead"
            },
            "click: choose a saved view to key",
        );
    });

    let Some(k) = app.ui_state.timeline_camera_key.filter(|&k| authored && k < p.keys.len()) else {
        ui.label(egui::RichText::new("Click a diamond to edit that view.").small().weak());
        return;
    };
    ui.separator();
    ui.label(egui::RichText::new(format!("View {} of {}", k + 1, p.keys.len())).strong());
    let mut t = p.time(k);
    ui.horizontal(|ui| {
        ui.label("at");
        let resp = ui.add(num::Drag::new(&mut t).speed(0.02).range(-3600.0..=3600.0).suffix("s"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "When the camera is on this view. It stays between its neighbours: the keys \
             keep their order, because each segment's route is the journey between two of them.",
            "drag: retime this view",
        );
        if resp.changed() {
            app.set_camera_key_time(k, t as f64);
        }
    });
    ui.horizontal(|ui| {
        let resp = ui.button((icons::CAMERA, "Go to it"));
        let resp = hinted(resp, &mut app.ui_state, "Put the playhead on this view", "click: scrub to this view");
        if resp.clicked() {
            app.scrub_to(p.time(k) as f64);
            app.set_camera_attached(true);
        }
        let resp = ui.button((icons::REFRESH, "Re-key"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Replace this view with the framing on screen, at the same time",
            "click: re-key this view",
        );
        if resp.clicked() {
            app.rekey_camera(k);
        }
        let resp = ui.button((icons::TRASH, "Delete"));
        let resp = hinted(resp, &mut app.ui_state, "Remove this view from the camera's track. Ctrl+Z brings it back.", "click: delete this view");
        if resp.clicked() {
            app.remove_path_key_at(k);
            app.ui_state.timeline_camera_key = None;
        }
    });
}

fn draw_new_track(ui: &mut egui::Ui, app: &mut App) {
    let params = app.param_catalogue();
    let tracked: Vec<String> = app.scene.tracks.iter().map(|t| t.target.clone()).collect();
    let taken = |p: &crate::params::Param| {
        tracked.contains(&p.path).then(|| "Already animated: it has a row in the timeline.".to_string())
    };
    let chosen = super::param_picker::picker(
        ui,
        &mut app.ui_state,
        "timeline_new_track",
        "Animate a parameter…",
        "Choose any value in the scene to animate: browse the tree, or type to search. \
         The new track starts with one key at the playhead holding the value the scene \
         has now, so nothing moves until you key it again.",
        &params,
        &taken,
    );
    if let Some(p) = chosen {
        match app.add_track(&p.path, Some(p.value.clone())) {
            Ok(i) => {
                app.ui_state.timeline_track = Some(i);
                app.ui_state.timeline_camera = false;
                app.ui_state.timeline_key = Some(0);
                app.ui_state.timeline_new_error = None;
            }
            Err(e) => app.ui_state.timeline_new_error = Some(e),
        }
    }
    if let Some(e) = &app.ui_state.timeline_new_error {
        ui.label(egui::RichText::new(e).small().color(super::theme::warn(ui.visuals())));
    }
}

/// Each track's parameter as people read it ("ocean › Rotation › z"), from
/// the catalogue; the path itself where the scene doesn't have it any more.
fn track_names(app: &mut App) -> Vec<String> {
    let cat = app.param_catalogue();
    app.scene
        .tracks
        .iter()
        .map(|t| match cat.iter().find(|p| p.path == t.target) {
            // The outermost group ("Transforms") is implied by the rest.
            Some(p) => {
                let crumbs = p.breadcrumb();
                crumbs.split_once(" › ").filter(|_| p.group.len() > 1).map_or(crumbs.clone(), |(_, r)| r.to_string())
            }
            None => t.target.clone(),
        })
        .collect()
}
