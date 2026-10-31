//! The transport, in the toolbar: return to start, play, record, loop, then
//! the time. That is a DAW's order (Logic's, Reaper's): to start and Play are
//! where a hand expects them, so they come first, and record and loop, which
//! say how play behaves, sit after. Always in the same place and always
//! visible, whether or not the Timeline window is open, because these are the
//! controls a hand reaches for without looking (ANIMATION-PLAN.md §2.1).
//!
//! The time reads in seconds and in frames: frames at the Render job's rate,
//! so a frame here is a frame of the file it would write. It is a number box
//! like any other: drag it to move the playhead a little at a time, which is
//! how a moment is usually found, or click it to type one: `12.5` (seconds),
//! `300f` (frames) or `1:02.5` (minutes and seconds).
//!
//! Play is the camera's Play, as it always was (O, Z, Space): it attaches the
//! camera to the timeline and runs the clock. The Timeline window keeps its
//! own clock-only Play beside the lanes.

use crate::app::App;

use super::hints::hinted;
use super::{icons, num};

/// Characters in the time readout: `999.99 s 99999 f`, which covers a
/// timeline of 16 minutes and 55 minutes of frames at 30 a second. Every
/// character here is toolbar width, which is what the squeeze ladder spends.
const TIME_CHARS: usize = 16;
/// ...and without its frame count: `999.99 s`.
const TIME_CHARS_SHORT: usize = 8;

/// Play and Pause, the states the one Play button pins across.
const PLAY_STATES: [(&str, &str); 2] = [(icons::PLAY, "Play"), (icons::PAUSE, "Pause")];

/// How much narrower the time readout is without its frame count, for the
/// toolbar's squeeze ladder.
pub fn frames_width(ui: &egui::Ui) -> f32 {
    num::mono_width(ui, TIME_CHARS) - num::mono_width(ui, TIME_CHARS_SHORT)
}

/// `frames`: whether the time readout carries the frame count. It's the
/// first thing the toolbar gives up when it runs out of room.
pub fn draw(ui: &mut egui::Ui, app: &mut App, frames: bool) {
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

    let moving = app.is_playing();
    let shown = if moving { PLAY_STATES[1] } else { PLAY_STATES[0] };
    // Lit while playing, in the colour that means "on": the transport's one
    // Play, and the one state of it you most need to read at a glance.
    let resp = ui.add(num::Pinned::new(shown, &PLAY_STATES).selected(moving));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        if moving {
            "Pause the scene (O, Z or Space). The camera stays attached, so scrubbing \
             still moves it."
        } else {
            "Play the scene from the playhead: every track moves, and the camera flies its \
             path if it is attached (O, Z or Space)"
        },
        if moving { "click: pause" } else { "click: play" },
    );
    if resp.clicked() {
        app.toggle_play();
    }

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

    draw_time(ui, app, frames);
}

/// The time, as a number box: drag to move the playhead by a little, click
/// to type a moment. A drag moves it a frame for every two points of travel
/// (the house drag gain), so a short drag is a few frames, which is what
/// finding a moment by eye wants; typing is for jumping somewhere exact.
fn draw_time(ui: &mut egui::Ui, app: &mut App, frames: bool) {
    let fps = app.frame_rate().max(1);
    let chars = if frames { TIME_CHARS } else { TIME_CHARS_SHORT };
    let width = num::mono_width(ui, chars) + 2.0 * ui.spacing().button_padding.x + 4.0;
    let height = ui.spacing().interact_size.y;
    let mut t = app.playhead();
    let resp = ui.add_sized(
        [width, height],
        num::Drag::new(&mut t)
            .speed(1.0 / fps as f64)
            .range(0.0..=36_000.0)
            .custom_formatter(move |t, _| {
                let frame = (t * fps as f64 + 1e-6).floor() as i64;
                if frames {
                    format!("{} s {:>5} f", num::cell(t as f32, 2, 6), frame)
                } else {
                    format!("{} s", num::cell(t as f32, 2, 6))
                }
            })
            .custom_parser(move |text| parse_time(text, fps)),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        format!(
            "The playhead, in seconds and in frames at {fps} a second (the Render job's \
             rate). Drag to move it a little at a time, or click to type a moment: 12.5 is \
             seconds, 300f is frames, 1:02.5 is a minute and 2.5 seconds. Left and Right \
             step a frame; Shift+Left and Shift+Right jump to the key before or after."
        ),
        "drag: move the playhead · click: type a time",
    );
    if resp.changed() {
        app.scrub_to(t);
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
