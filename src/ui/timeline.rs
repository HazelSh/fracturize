//! The Timeline window (FEATURE-PLANS.md item 8): the one clock the camera and
//! every `[[track]]` share, drawn wide and low.
//!
//! A ruler in seconds with the playhead on it; under it a row for the camera
//! and a row per track. Drag along the ruler or an empty stretch of a row to
//! scrub. Each track's row draws its keys as diamonds and its value as a faint
//! curve behind them, scaled to the row — the curve view the plan asks for,
//! small. Click a row to select its track, a diamond to select its key; drag a
//! diamond to retime it. The eye mutes a track; right-click a row for its menu.
//!
//! Only the ruler scrubs. A press on a row selects it and moves nothing else,
//! so choosing a track never shifts the camera.
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
//! **No auto-key.** Editing a tracked value in the inspector edits the scene's
//! own value — which a rate or cycle moves about, and which keys override.
//! Whether an inspector edit should instead key the playhead is Hazel's call
//! (FEATURE-PLANS item 10), so key values are edited here.
//!
//! Not yet: zooming the ruler — it always shows one timeline's length, paging
//! along when the playhead runs past it.
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
const RULER_H: f32 = 18.0;
const ROW_H: f32 = 22.0;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.timeline_open;
    if !open {
        return;
    }
    super::window(ctx, app, super::WindowKey::Timeline, "Timeline")
        .open(&mut open)
        .show(ctx, |ui| {
            draw_transport(ui, app);
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
                    egui::ScrollArea::vertical().id_salt("timeline_lanes").auto_shrink([false, false]).show(ui, |ui| {
                        draw_lanes(ui, app);
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

fn draw_transport(ui: &mut egui::Ui, app: &mut App) {
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
             is attached (the link on the camera row). O, Z or Space fly the camera \
             instead: they attach it and play.",
            if playing { "click: pause" } else { "click: play from the playhead" },
        );
        if resp.clicked() {
            app.toggle_playback();
        }

        let len = app.timeline_length();
        let resp = num::text_cell(
            ui,
            &format!("{} / {} s", num::cell(app.playhead() as f32, 2, 7), num::cell(len as f32, 2, 7)),
            19,
        );
        hinted(
            resp,
            &mut app.ui_state,
            "The playhead, and the timeline's length: the camera path or the tracks, \
             whichever runs longer. An animation render plays exactly this.",
            "the playhead and the timeline's length",
        );

        let has_tracks = !app.scene.tracks.is_empty();
        let mut looping = app.loop_playback;
        let resp = ui.add_enabled(has_tracks, egui::Checkbox::new(&mut looping, (icons::REPEAT, "Loop")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if has_tracks {
                "Go round to the start at the end of the timeline, rather than stopping \
                 there. Not saved: this is how you are watching, not the scene."
            } else {
                "This scene has no tracks, so the camera path's own loop setting (Camera \
                 window) is what decides whether playback goes round"
            },
            "toggle: loop playback",
        );
        if resp.changed() {
            app.loop_playback = looping;
        }

        if let Some(who) = app.track_invalid() {
            ui.label(
                egui::RichText::new(format!("{} {who} has taken a value the scene can't use; holding the last good frame", icons::X))
                    .small()
                    .color(super::theme::warn(ui.visuals())),
            );
        } else if let Some(e) = app.track_error() {
            ui.label(egui::RichText::new(format!("{} {e}", icons::X)).small().color(super::theme::warn(ui.visuals())));
        } else if !has_tracks {
            ui.label(
                egui::RichText::new("No tracks yet: \"Animate a parameter\" on the right starts one")
                    .small()
                    .weak(),
            );
        }
    });
}

/// The seconds the lanes show: one timeline's length, paged so the playhead
/// is always on it.
fn visible_span(app: &App) -> (f64, f64) {
    let len = app.timeline_length().max(0.1);
    let page = (app.playhead() / len).floor().max(0.0);
    (page * len, page * len + len)
}

fn draw_lanes(ui: &mut egui::Ui, app: &mut App) {
    // Frozen while a key is dragged: the timeline's length follows its last
    // key, and the ruler pages by the playhead, so a ruler that rescaled under
    // the drag fed the key's own motion back into where the pointer maps to —
    // a key dragged left past the playhead jumped right.
    let dragging = app.ui_state.timeline_drag_key.is_some() || app.ui_state.timeline_drag_camera.is_some();
    let (t0, t1) = match app.ui_state.timeline_span {
        Some(span) if dragging => span,
        _ => visible_span(app),
    };
    let full_w = ui.available_width();
    let lane_w = (full_w - LABEL_W).max(60.0);
    let row_count = 1 + app.scene.tracks.len();
    let lanes_h = RULER_H + ROW_H * row_count as f32;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(full_w, lanes_h), egui::Sense::click_and_drag());
    let lane = egui::Rect::from_min_size(egui::pos2(rect.left() + LABEL_W, rect.top()), egui::vec2(lane_w, lanes_h));
    let x_of = |t: f64| lane.left() + ((t - t0) / (t1 - t0)) as f32 * lane.width();
    let t_of = |x: f32| t0 + ((x - lane.left()) / lane.width()).clamp(0.0, 1.0) as f64 * (t1 - t0);

    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(lane, 0.0, visuals.extreme_bg_color);

    // Ruler: a tick a second, a label where they have room.
    let ruler = egui::Rect::from_min_size(lane.min, egui::vec2(lane.width(), RULER_H));
    let span = t1 - t0;
    let step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0]
        .into_iter()
        .find(|s| (lane.width() as f64) / (span / s) >= 48.0)
        .unwrap_or(300.0);
    let font = egui::FontId::monospace(10.0);
    let mut t = (t0 / step).ceil() * step;
    while t <= t1 + 1e-9 {
        let x = x_of(t);
        painter.line_segment(
            [egui::pos2(x, ruler.bottom() - 5.0), egui::pos2(x, ruler.bottom())],
            visuals.widgets.noninteractive.fg_stroke,
        );
        painter.text(
            egui::pos2(x + 2.0, ruler.top() + 1.0),
            egui::Align2::LEFT_TOP,
            num::fixed(t as f32, if step < 1.0 { 2 } else { 0 }),
            font.clone(),
            visuals.weak_text_color(),
        );
        t += step;
    }

    // Rows: the camera, then each track, named as the parameter picker names
    // them — the -S path only where the scene no longer has the parameter.
    let names = track_names(app);
    let mut hovered_row = None;
    let pointer = ui.ctx().pointer_hover_pos();
    for row in 0..row_count {
        let top = rect.top() + RULER_H + row as f32 * ROW_H;
        let row_rect = egui::Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(full_w, ROW_H));
        let lane_row = egui::Rect::from_min_max(egui::pos2(lane.left(), top), egui::pos2(lane.right(), top + ROW_H));
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
        if selected {
            painter.rect_filled(row_rect, 0.0, visuals.selection.bg_fill.gamma_multiply(0.35));
        }
        if row == 0 {
            draw_camera_row(&painter, app, row_rect, lane_row, &x_of, &visuals);
        } else {
            let key = if selected { app.ui_state.timeline_key } else { None };
            draw_track_row(&painter, &app.scene.tracks[row - 1], &names[row - 1], row_rect, lane_row, (t0, t1), &x_of, &visuals, key);
        }
    }

    // The playhead, over everything.
    let px = x_of(app.playhead());
    painter.line_segment(
        [egui::pos2(px, lane.top()), egui::pos2(px, lane.bottom())],
        egui::Stroke::new(1.5, visuals.selection.stroke.color),
    );

    // A press on the ruler scrubs, and only there: the playhead is the
    // ruler's. A press on a row selects it — a track's, and the diamond under
    // the pointer, which a drag then retimes; or the camera's — and moves
    // nothing else, so picking a track never shifts the camera.
    let row_at = |y: f32| -> Option<usize> {
        let r = ((y - rect.top() - RULER_H) / ROW_H).floor();
        (r >= 0.0 && (r as usize) < row_count).then_some(r as usize)
    };
    if resp.drag_started() || resp.clicked() {
        // Where the press *began*: by the time egui calls it a drag the
        // pointer is already past its threshold, and a diamond is smaller.
        let origin = ui.input(|i| i.pointer.press_origin());
        if let Some(p) = origin.or(resp.interact_pointer_pos()) {
            app.ui_state.timeline_drag_key = None;
            app.ui_state.timeline_drag_camera = None;
            if row_at(p.y) == Some(0) {
                app.ui_state.timeline_camera = true;
                app.ui_state.timeline_track = None;
                app.ui_state.timeline_key = None;
                if !app.path_is_default() {
                    let hit = camera_key_times(app, lane, &x_of)
                        .into_iter()
                        .map(|o| (o, (x_of(o.t) - p.x).abs()))
                        .filter(|&(_, d)| d <= KEY_GRAB)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(o, _)| o);
                    app.ui_state.timeline_camera_key = hit.map(|o| o.key);
                    if let Some(o) = hit.filter(|o| o.primary) {
                        app.ui_state.timeline_drag_camera = Some(o.key);
                        app.ui_state.timeline_span = Some((t0, t1));
                    }
                }
            }
            if let Some(r) = row_at(p.y).filter(|&r| r > 0) {
                let i = r - 1;
                app.ui_state.timeline_camera = false;
                if app.ui_state.timeline_track != Some(i) {
                    app.ui_state.timeline_key = None;
                }
                app.ui_state.timeline_track = Some(i);
                if let Source::Keys { keys, .. } = &app.scene.tracks[i].source {
                    let hit = keys
                        .iter()
                        .enumerate()
                        .map(|(k, key)| (k, (x_of(key.t) - p.x).abs()))
                        .filter(|&(_, d)| d <= KEY_GRAB)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(k, _)| k);
                    if hit.is_some() {
                        app.ui_state.timeline_key = hit;
                        app.ui_state.timeline_drag_key = hit.map(|k| (i, k));
                        app.ui_state.timeline_span = Some((t0, t1));
                    }
                }
            }
        }
    }
    let on_ruler = ui
        .input(|i| i.pointer.press_origin())
        .is_some_and(|o| o.y < rect.top() + RULER_H && o.x >= lane.left());
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            match (app.ui_state.timeline_drag_key, app.ui_state.timeline_drag_camera) {
                (Some((i, k)), _) if resp.dragged() => app.set_key_time(i, k, t_of(p.x)),
                (None, Some(k)) if resp.dragged() => app.set_camera_key_time(k, t_of(p.x)),
                (Some(_), _) | (None, Some(_)) => {}
                (None, None) if on_ruler => app.scrub_to(t_of(p.x)),
                (None, None) => {}
            }
        }
    }
    if resp.drag_stopped() {
        app.ui_state.timeline_drag_key = None;
        app.ui_state.timeline_drag_camera = None;
    }
    let row_hint = match hovered_row {
        Some(0) => "the camera · click a diamond: select that view · drag it: retime it · link: attach or detach".to_string(),
        Some(r) => format!("{} · click: select · drag a diamond: retime it · right-click: menu", names[r - 1]),
        None => "drag: scrub the playhead".to_string(),
    };
    let tooltip = match hovered_row {
        Some(0) => camera_description(app),
        Some(r) => format!("{}\n{}", names[r - 1], track_description(&app.scene.tracks[r - 1])),
        None => "The ruler, in seconds. Drag along it to scrub: the tracks follow the \
                 playhead, and so does the camera if it is attached. Scrubbing isn't an edit."
            .to_string(),
    };
    let resp = hinted(resp, &mut app.ui_state, tooltip, &row_hint);

    // Per-row controls in the label column, drawn as real widgets over the
    // painted rows so each gets its own hint: the camera's attach link, and
    // each track's mute eye.
    {
        let link_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 2.0, rect.top() + RULER_H + 1.0),
            egui::vec2(20.0, ROW_H - 2.0),
        );
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
        let top = rect.top() + RULER_H + (i + 1) as f32 * ROW_H;
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

    // Right-click a track's row for its menu.
    let menu_row = hovered_row.filter(|&r| r > 0);
    resp.context_menu(|ui| {
        let Some(r) = menu_row.or(app.ui_state.timeline_menu_row) else {
            ui.close();
            return;
        };
        app.ui_state.timeline_menu_row = Some(r);
        let i = r - 1;
        let Some(track) = app.scene.tracks.get(i) else {
            ui.close();
            return;
        };
        let muted = track.mute;
        ui.label(egui::RichText::new(names.get(i).cloned().unwrap_or_default()).small().strong());
        let resp = ui.button(if muted { (icons::EYE, "Unmute") } else { (icons::EYE_SLASH, "Mute") });
        let resp = hinted(resp, &mut app.ui_state, "Mute or unmute this track", "click: mute / unmute");
        if resp.clicked() {
            app.set_track_muted(i, !muted);
            ui.close();
        }
        let resp = ui.button((icons::TRASH, "Delete track"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Remove this track from the scene; the value goes back to what the scene authors. \
             Ctrl+Z brings it back.",
            "click: delete this track",
        );
        if resp.clicked() {
            app.delete_track(i);
            ui.close();
        }
    });
    if !resp.context_menu_opened() {
        app.ui_state.timeline_menu_row = None;
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
        if attached { visuals.text_color() } else { visuals.weak_text_color() },
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
    selected_key: Option<usize>,
) {
    let text = if track.mute { visuals.weak_text_color() } else { visuals.text_color() };
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
