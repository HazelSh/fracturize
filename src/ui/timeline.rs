//! The Timeline window (FEATURE-PLANS.md item 8): the one clock the camera and
//! every `[[track]]` share, drawn wide and low.
//!
//! A ruler in seconds with the playhead on it; under it a row for the camera
//! and a row per track. Drag anywhere along the ruler or the rows to scrub.
//! Each track's row draws its keys as diamonds and its value as a faint curve
//! behind them, scaled to the row — the curve view the plan asks for, small.
//! The eye mutes a track (an edit: it is in the file); right-click a row for
//! its menu.
//!
//! What it doesn't do yet, on purpose, so the model under it could settle
//! first: add, drag or retime keys (tracks are authored in the scene file, or
//! by an agent with `--time-grid` to look at them), and zoom the ruler — it
//! always shows one timeline's length, paging along when the playhead runs
//! past it.
//!
//! The playhead moves because time moves, which is content, not chrome; the
//! zero-animation rule still holds for everything else here.

use crate::app::App;
use crate::track::{KeyValue, Source, Track};

use super::hints::hinted;
use super::{icons, num};

const LABEL_W: f32 = 220.0;
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
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                draw_lanes(ui, app);
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
            "Play or pause the timeline: the camera's path and every track together \
             (O, Z or Space)",
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

        if let Some(e) = app.track_error() {
            ui.label(egui::RichText::new(format!("{} {e}", icons::X)).small().color(ui.visuals().warn_fg_color));
        } else if !has_tracks {
            ui.label(
                egui::RichText::new(
                    "No tracks: add [[track]] tables to the scene file to animate any -S path",
                )
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
    let (t0, t1) = visible_span(app);
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

    // Rows: the camera, then each track.
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
        if row == 0 {
            draw_camera_row(&painter, app, row_rect, lane_row, &x_of, &visuals);
        } else {
            draw_track_row(&painter, &app.scene.tracks[row - 1], row_rect, lane_row, (t0, t1), &x_of, &visuals);
        }
    }

    // The playhead, over everything.
    let px = x_of(app.playhead());
    painter.line_segment(
        [egui::pos2(px, lane.top()), egui::pos2(px, lane.bottom())],
        egui::Stroke::new(1.5, visuals.selection.stroke.color),
    );

    // Scrubbing: a press or a drag on the lanes moves the playhead; the label
    // column is for the rows' own controls.
    if let Some(p) = resp.interact_pointer_pos() {
        if (resp.dragged() || resp.clicked()) && p.x >= lane.left() {
            app.scrub_to(t_of(p.x));
        }
    }
    let row_hint = match hovered_row {
        Some(0) => "the camera's path on the timeline · drag: scrub".to_string(),
        Some(r) => format!("{} · drag: scrub · right-click: menu", app.scene.tracks[r - 1].target),
        None => "drag: scrub the playhead".to_string(),
    };
    let tooltip = match hovered_row {
        Some(0) => camera_description(app),
        Some(r) => track_description(&app.scene.tracks[r - 1]),
        None => "The ruler, in seconds. Drag to scrub: the scene and the camera follow the \
                 playhead. Scrubbing isn't an edit."
            .to_string(),
    };
    let resp = hinted(resp, &mut app.ui_state, tooltip, &row_hint);

    // Per-row controls in the label column: the mute eye. Drawn as real
    // widgets over the painted rows so each gets its own hint.
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
        ui.label(egui::RichText::new(track.target.clone()).monospace().small());
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
    painter.text(
        egui::pos2(row.left() + 24.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{} camera", icons::VIDEO_CAMERA),
        egui::FontId::proportional(13.0),
        visuals.text_color(),
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
    for t in camera_key_times(app, lane, x_of) {
        diamond(painter, egui::pos2(x_of(t), lane.center().y), colour);
    }
}

/// When the camera passes each of its keys, over the visible span.
fn camera_key_times(app: &App, lane: egui::Rect, x_of: &dyn Fn(f64) -> f32) -> Vec<f64> {
    let p = app.camera_path();
    let n = p.keys.len();
    let segs = p.segments().max(1) as f64;
    let d = p.duration() as f64;
    let eased = p.ease.unwrap_or_else(|| p.loops.eases_by_default());
    // Path parameter of each key over one loop.
    let mut params: Vec<f64> = (0..n).map(|i| i as f64 / segs).filter(|&u| u <= 1.0 + 1e-9).collect();
    if p.loops == crate::path::Loop::PingPong {
        let back: Vec<f64> = params.iter().rev().skip(1).map(|u| 1.0 - u / 2.0).collect();
        params = params.iter().map(|u| u / 2.0).chain(back).collect();
    }
    // Eased paths reach a key later or sooner than its parameter says; the
    // ease is a smoothstep on loop time, so invert it.
    let unease = |u: f64| {
        if !eased {
            return u;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..40 {
            let m = 0.5 * (lo + hi);
            if m * m * (3.0 - 2.0 * m) < u { lo = m } else { hi = m }
        }
        0.5 * (lo + hi)
    };
    let mut out = Vec::new();
    let laps = if p.wraps() { 64 } else { 1 };
    for lap in 0..laps {
        for &u in &params {
            let t = (lap as f64 + unease(u)) * d;
            let x = x_of(t);
            if x > lane.right() + 1.0 {
                return out;
            }
            if x >= lane.left() - 1.0 {
                out.push(t);
            }
        }
    }
    out
}

fn draw_track_row(
    painter: &egui::Painter,
    track: &Track,
    row: egui::Rect,
    lane: egui::Rect,
    (t0, t1): (f64, f64),
    x_of: &dyn Fn(f64) -> f32,
    visuals: &egui::Visuals,
) {
    let text = if track.mute { visuals.weak_text_color() } else { visuals.text_color() };
    // The target, shortened from the left: the end of a path is the part that
    // says what moves.
    let label = shorten_left(&track.target, 28);
    painter.text(
        egui::pos2(row.left() + 24.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(11.0),
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
                KeyValue::Other(_) => None,
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
        for k in keys {
            let x = x_of(k.t);
            if x >= lane.left() - 1.0 && x <= lane.right() + 1.0 {
                diamond(painter, egui::pos2(x, lane.center().y), colour);
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
        "{}\n\n{}.\n{}.{}",
        t.target,
        what,
        cost,
        if t.mute { "\nMuted." } else { "" }
    )
}
