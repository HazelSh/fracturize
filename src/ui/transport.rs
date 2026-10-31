//! The transport, in the toolbar: record, return to start, play, the time,
//! loop. Always in the same place and always visible, whether or not the
//! Timeline window is open, because these are the controls a hand reaches
//! for without looking (ANIMATION-PLAN.md §2.1).
//!
//! The time reads in seconds and in frames — frames at the Render job's rate,
//! so a frame here is a frame of the file it would write. Click it while
//! paused to type a moment: `12.5` (seconds), `300f` (frames) or `1:02.5`
//! (minutes and seconds).
//!
//! Play is the camera's Play, as it always was (O, Z, Space): it attaches the
//! camera to the timeline and runs the clock. The Timeline window keeps its
//! own clock-only Play beside the lanes.

use crate::app::App;

use super::hints::hinted;
use super::{icons, num};

/// Characters in the time readout: `9999.99 s 299999 f`.
const TIME_CHARS: usize = 18;

pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    // One group: the gaps inside it are the toolbar's ordinary spacing, and
    // the group sits between separators.
    let armed = app.record_armed;
    let red = ui.visuals().error_fg_color;
    let resp = ui.add(egui::Button::selectable(
        armed,
        egui::RichText::new(icons::RECORD).color(if armed { red } else { ui.visuals().text_color() }),
    ));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if armed {
            "Recording: every edit keys the value it changes at the playhead, making the \
             track if there isn't one. Paused, the inspector and gizmos show the scene at \
             the playhead, so you pose it and the pose is keyed. Click to stop recording."
        } else {
            "Record: arm it, and every edit keys what it changes at the playhead instead of \
             changing the scene for all time. Drag a gizmo, type a value, recolour a map: \
             each becomes a key. Each edit and its keys are one undo step."
        },
        if armed { "click: stop recording" } else { "click: arm record" },
    );
    if resp.clicked() {
        app.record_armed = !armed;
    }

    let resp = ui.button(icons::SKIP_BACK);
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Back to the start of the timeline. Playing carries on from there.",
        "click: return to zero",
    );
    if resp.clicked() {
        app.return_to_start();
    }

    // "Play" and "Pause" aren't the same width, so the button is pinned to
    // the wider or it shifts everything to its right when clicked.
    let moving = app.is_playing();
    let play_width = num::icon_button_width(ui, &[(icons::PLAY, "Play"), (icons::PAUSE, "Pause")]);
    let resp = ui.add(
        egui::Button::new(if moving {
            (icons::PAUSE, "Pause", egui::Atom::grow())
        } else {
            (icons::PLAY, "Play", egui::Atom::grow())
        })
        .min_size(egui::vec2(play_width, 0.0)),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if moving {
            "Pause the timeline (O, Z or Space)"
        } else {
            "Play the timeline from the playhead, with the camera flying its path (O, Z or \
             Space)"
        },
        if moving { "click: pause" } else { "click: play" },
    );
    if resp.clicked() {
        if moving {
            app.toggle_playback();
        } else {
            app.toggle_camera_motion();
        }
    }

    draw_time(ui, app);

    let looping = app.loop_playback;
    let resp = ui.add(egui::Button::selectable(looping, icons::REPEAT));
    let region = app.loop_region;
    let resp = hinted(
        resp,
        &mut app.ui_state,
        match (looping, region) {
            (_, Some((a, b))) => format!(
                "Loop {}: playback goes round the loop region, {a:.2} s to {b:.2} s. Drag in \
                 the strip along the top of the Timeline's ruler to move it.",
                if looping { "on" } else { "off" }
            ),
            (true, None) => "Loop on: playback goes round to the start at the end of the \
                             timeline. Drag in the strip along the top of the Timeline's \
                             ruler to loop a part of it."
                .to_string(),
            (false, None) => "Loop off: playback stops at the end of the timeline".to_string(),
        },
        if looping { "click: stop looping" } else { "click: loop" },
    );
    if resp.clicked() {
        app.loop_playback = !looping;
    }
}

/// The time: a readout while playing, and a field to type a moment into
/// while paused.
fn draw_time(ui: &mut egui::Ui, app: &mut App) {
    let fps = app.frame_rate();
    let width = num::mono_width(ui, TIME_CHARS) + 2.0 * ui.spacing().button_padding.x;
    let height = ui.spacing().interact_size.y;
    let playing = app.is_playing();
    if playing {
        app.ui_state.transport_time_edit = None;
    }
    match app.ui_state.transport_time_edit.take() {
        Some(mut text) => {
            let resp = ui.add_sized(
                egui::vec2(width, height),
                egui::TextEdit::singleline(&mut text).font(egui::TextStyle::Monospace),
            );
            let id = resp.id;
            if !resp.has_focus() && !resp.lost_focus() {
                resp.request_focus();
                // Select the whole thing, so typing replaces it.
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                    let all = egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(text.chars().count()),
                    );
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), id);
                }
            }
            let resp = hinted(
                resp,
                &mut app.ui_state,
                format!(
                    "Type a moment and press Enter: 12.5 is seconds, 300f is frames (at {fps} a \
                     second, the Render job's rate), 1:02.5 is a minute and 2.5 seconds. Esc \
                     leaves it."
                ),
                "type a time · Enter: go there · Esc: leave it",
            );
            if resp.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    match parse_time(&text, fps) {
                        Some(t) => app.scrub_to(t),
                        None => app.set_notice(
                            format!("\"{}\" isn't a time: try 12.5, 300f or 1:02.5", text.trim()),
                            crate::app::Tone::Error,
                        ),
                    }
                }
            } else {
                app.ui_state.transport_time_edit = Some(text);
            }
        }
        None => {
            let t = app.playhead();
            let frame = (t * fps as f64 + 1e-6).floor() as i64;
            let text = format!("{} s {:>6} f", num::cell(t as f32, 2, 7), frame);
            let resp = ui.add_sized(
                egui::vec2(width, height),
                egui::Button::new(egui::RichText::new(text).monospace()).sense(if playing {
                    egui::Sense::hover()
                } else {
                    egui::Sense::click()
                }),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                format!(
                    "The playhead, in seconds and in frames at {fps} a second (the Render \
                     job's rate). {}Left and Right step a frame; Shift+Left and Shift+Right \
                     jump to the key before or after.",
                    if playing { "Pause to type a time. " } else { "Click to type a time. " }
                ),
                if playing { "the playhead" } else { "click: type a time" },
            );
            if resp.clicked() {
                app.ui_state.transport_time_edit = Some(format!("{:.2}", t));
            }
        }
    }
}

/// A moment as a person types it: seconds (`12.5`, `12.5s`), frames
/// (`300f`), or minutes and seconds (`1:02.5`). Never negative.
pub fn parse_time(text: &str, fps: u32) -> Option<f64> {
    let s = text.trim().to_lowercase();
    let t = if let Some(f) = s.strip_suffix('f') {
        f.trim().parse::<f64>().ok()? / fps.max(1) as f64
    } else if let Some((m, sec)) = s.split_once(':') {
        m.trim().parse::<f64>().ok()? * 60.0 + sec.trim().trim_end_matches('s').trim().parse::<f64>().ok()?
    } else {
        s.trim_end_matches('s').trim().parse::<f64>().ok()?
    };
    (t.is_finite() && t >= 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::parse_time;

    #[test]
    fn times_read_the_way_people_type_them() {
        assert_eq!(parse_time("12.5", 30), Some(12.5));
        assert_eq!(parse_time(" 12.5 s ", 30), Some(12.5));
        assert_eq!(parse_time("300f", 30), Some(10.0));
        assert_eq!(parse_time("300 F", 24), Some(12.5));
        assert_eq!(parse_time("1:02.5", 30), Some(62.5));
        assert_eq!(parse_time("-1", 30), None);
        assert_eq!(parse_time("soon", 30), None);
    }
}
