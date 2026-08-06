//! Two safety devices that share a shape: *make the destructive click cost
//! more than one twitch of the hand*.
//!
//! [`Arm`] is the click-wait-click guard used by render-cancel and by the
//! Discard button below. [`draw`] is the unsaved-changes dialog — the one
//! genuinely modal thing in the app, because it stands between you and
//! losing work you cannot get back.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::app::App;

use super::hints::hinted;

/// How long the control spends counting down before the confirm is available.
///
/// Long enough that no double-click, and no impatient burst of clicking, can
/// carry you through it — that is the whole guard. It is spent with the button
/// *visibly disabled and counting*, so the wait is something you watch rather
/// than something you run into.
pub const COUNTDOWN: Duration = Duration::from_secs(3);

/// What an [`Arm`] currently is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArmState {
    /// Not armed. The control is an ordinary button, and a click arms it.
    Idle,
    /// Counting down, with this many whole seconds left to show. The control is
    /// drawn disabled and takes no input at all.
    Counting(u32),
    /// Armed and available. The control is drawn in danger colours and the next
    /// click on it fires.
    Ready,
}

/// A two-stage destructive confirmation.
///
/// The shape, and why each part of it is the way it is:
///
/// 1. Click the button. It becomes **disabled and greyed**, reading
///    `waiting (3)`, and counts down.
/// 2. It takes no input at all while counting. This is the point: a guard that
///    merely *ignores* clicks looks identical to a broken button, and the
///    person it exists to protect is precisely the one clicking fast. A
///    disabled control with a number ticking on it is the affordance everyone
///    already knows from download and install dialogs — it says "not yet, and
///    here is exactly how long".
/// 3. At zero it becomes enabled again in **danger colours**, with a different
///    and more explicit label (`Cancel` → `Abort render`). The second click is
///    on a control that has visibly *become available*, not on one that was
///    silently pretending to be unavailable.
/// 4. **It then sits there.** Nothing expires. An arm window is a clock
///    guessing at whether you are still engaged, and it guesses wrong in the
///    dangerous direction: too short and your confirming click quietly does
///    nothing, too long and a live one-click trigger outlives your attention.
/// 5. **Clicking anywhere else puts it back**, with no other effect. That is
///    the real disengagement signal — you demonstrated your attention went
///    somewhere else — and it needs no timer to read.
///
/// Because nothing expires, the degraded-machine case that shaped the earlier
/// design stops mattering: if the box has stopped repainting you simply wait
/// longer for the frame that draws the confirm, and it is still there when it
/// arrives. Nothing can time out from under you.
#[derive(Default, Clone, Copy)]
pub struct Arm {
    at: Option<Instant>,
}

impl Arm {
    pub fn state(&self) -> ArmState {
        match self.at {
            None => ArmState::Idle,
            Some(t) => {
                let elapsed = t.elapsed();
                if elapsed >= COUNTDOWN {
                    ArmState::Ready
                } else {
                    // Round *up*, so the first frame shows the full count and it
                    // never displays a zero it isn't honouring.
                    let left = (COUNTDOWN - elapsed).as_secs_f32().ceil() as u32;
                    ArmState::Counting(left.max(1))
                }
            }
        }
    }

    pub fn armed(&self) -> bool {
        self.at.is_some()
    }

    pub fn arm(&mut self) {
        self.at = Some(Instant::now());
    }

    pub fn disarm(&mut self) {
        self.at = None;
    }
}

/// Draw a two-stage destructive confirmation as one button. Returns the new
/// arm state and whether *this* click confirmed.
///
/// `Arm` is passed and returned by value rather than borrowed because the state
/// it guards usually lives on `App` next to `ui_state`, and the two can't both
/// be borrowed mutably. It is two words; copying it is free.
///
/// `idle` is the resting label (`Cancel`); `danger` is the confirm label
/// (`Abort render`). Deliberately different words: the confirm is a different
/// act from the request, and reusing the label would invite the second click to
/// be made on the memory of the first.
pub fn danger_button(
    ui: &mut egui::Ui,
    ui_state: &mut super::UiState,
    mut arm: Arm,
    idle: &str,
    danger: &str,
    idle_tip: &str,
    danger_tip: &str,
) -> (Arm, bool) {
    let state = arm.state();
    let counting = format!("waiting ({})", COUNTDOWN.as_secs());

    let (text, enabled) = match state {
        ArmState::Idle => (idle.to_string(), true),
        ArmState::Counting(n) => (format!("waiting ({})", n), false),
        ArmState::Ready => (danger.to_string(), true),
    };

    // One width for all three states, so the row doesn't re-lay-out under the
    // pointer at the exact moment the pointer is aimed at it — see `ui::num`.
    let w = [idle, counting.as_str(), danger]
        .iter()
        .map(|t| text_width(ui, t))
        .fold(0.0f32, f32::max)
        + 2.0 * ui.spacing().button_padding.x
        + 6.0;
    let h = ui.spacing().interact_size.y;

    let mut button = egui::Button::new(match state {
        // White on red: this is the one control in the app allowed to shout.
        ArmState::Ready => egui::RichText::new(text).strong().color(egui::Color32::WHITE),
        _ => egui::RichText::new(text),
    });
    if state == ArmState::Ready {
        button = button.fill(ui.visuals().error_fg_color.gamma_multiply(0.55));
    }

    let resp = ui
        .add_enabled_ui(enabled, |ui| ui.add_sized([w, h], button))
        .inner;
    let resp = super::hints::hinted(
        resp,
        ui_state,
        match state {
            ArmState::Idle => idle_tip.to_string(),
            ArmState::Counting(_) => format!(
                "{}\n\nAvailable in a moment. Click anywhere else to call it off.",
                danger_tip
            ),
            ArmState::Ready => format!("{}\n\nClick anywhere else to call it off.", danger_tip),
        },
        match state {
            ArmState::Idle => "click: arm this, then confirm",
            ArmState::Counting(_) => "waiting — click anywhere else to call it off",
            ArmState::Ready => "click: confirm · click anywhere else: call it off",
        },
    );

    let mut fired = false;
    if resp.clicked() {
        match state {
            ArmState::Idle => arm.arm(),
            ArmState::Ready => {
                arm.disarm();
                fired = true;
            }
            // Unreachable: the control is disabled while counting.
            ArmState::Counting(_) => {}
        }
    } else if arm.armed()
        && ui.input(|i| i.pointer.any_pressed())
        && !resp.contains_pointer()
    {
        // Attention went elsewhere. Note this is checked on the *press*, while
        // `clicked()` lands on the release — so the press that arms the control
        // can't disarm it on the same frame, because that press is over it.
        arm.disarm();
    }

    // Keep the seconds ticking even if nothing else asks for a frame.
    if matches!(state, ArmState::Counting(_)) {
        ui.ctx().request_repaint();
    }

    (arm, fired)
}

/// Width of `s` in the button text style.
fn text_width(ui: &egui::Ui, s: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(s.to_owned(), font, egui::Color32::WHITE).size().x
    })
}

/// Something that would throw away unsaved work, waiting on an answer.
#[derive(Clone, PartialEq, Debug)]
pub enum Pending {
    /// Leave the application.
    Quit,
    /// Open this scene file, replacing what's loaded.
    Load(PathBuf),
}

/// The unsaved-changes prompt: Save / Discard / Cancel.
///
/// Genuinely modal — it dims and blocks — which is the right call for exactly
/// this one dialog and no other in the app: it is the point at which the answer
/// determines whether an hour of work survives, so there is nothing else you
/// could usefully be doing in the window behind it.
pub fn draw(ctx: &egui::Context, app: &mut App) {
    let Some(pending) = app.pending_action.clone() else {
        app.ui_state.discard_arm.disarm();
        return;
    };

    let verb = match &pending {
        Pending::Quit => "quitting".to_string(),
        Pending::Load(p) => format!("opening {}", p.display()),
    };
    // Say what the button does, not just what it throws away — the two acts
    // are one click here, and "Discard" alone doesn't say you're also leaving.
    let discard_label = match &pending {
        Pending::Quit => "Discard & quit",
        Pending::Load(_) => "Discard & open",
    };
    let scene = app.scene.name.clone();

    let mut resolve: Option<Resolution> = None;

    egui::Modal::new(egui::Id::new("fracturize_unsaved_changes")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading("Unsaved changes");
        ui.add_space(4.0);
        ui.label(format!(
            "“{}” has edits that aren't written to disk. They'll be lost by {}.",
            scene, verb
        ));

        // A save that didn't work says so *here*, next to the button that was
        // supposed to have done it, rather than only in the log.
        if let Some(err) = app.last_save_error().map(str::to_string) {
            ui.add_space(6.0);
            // The error already says what went wrong ("Failed to write scene
            // file: Permission denied"); a prefix would only restate it.
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
            ui.label(
                egui::RichText::new(
                    "Nothing has been lost yet. Try “Save as…” somewhere else, \
                     or Cancel and sort it out.",
                )
                .small()
                .weak(),
            );
        }

        ui.add_space(10.0);

        ui.horizontal(|ui| {
            let resp = ui.button("Save");
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Write the scene to its file, then continue",
                "click: save, then continue",
            );
            if resp.clicked() {
                resolve = Some(Resolution::Save);
            }

            // A plain button, deliberately — *not* the countdown guard that
            // render-cancel uses. What already protects this case is that the
            // dialog is a second click in a *different place* from the one that
            // started the quit, and that getting out is free: Escape, Cancel,
            // or a click anywhere outside. A three-second countdown on top of
            // that is slow and distracting for something you'll do often, and
            // it spends the person's patience on the wrong risk. Abandoning a
            // render job is the bigger deal: it's minutes or hours of GPU time
            // with nothing on disk, where this is edits you just made and can
            // see.
            let resp = ui.button(discard_label);
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Throw the edits away and continue. There is no undo for this.",
                "click: discard the edits",
            );
            if resp.clicked() {
                resolve = Some(Resolution::Discard);
            }

            let resp = ui.button("Cancel");
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Go back to the scene and leave it open",
                "click: go back",
            );
            if resp.clicked() {
                resolve = Some(Resolution::Cancel);
            }
        });

        // Escape is *cancel* here, the way it is everywhere else now.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            resolve = Some(Resolution::Cancel);
        }
    });

    match resolve {
        Some(Resolution::Save) => {
            app.save_scene();
            // Only leave if the write actually happened. A save can fail for
            // entirely ordinary reasons, and quitting on the *assumption* that
            // it worked would lose the work in the one path where the person
            // did everything right — which is the failure this whole dialog
            // exists to prevent. A successful save clears the dirty flag; if
            // it's still set, the dialog stays up and says why.
            if !app.is_dirty() {
                app.proceed_with_pending();
            }
        }
        Some(Resolution::Discard) => app.proceed_with_pending(),
        Some(Resolution::Cancel) => {
            app.pending_action = None;
            app.ui_state.discard_arm.disarm();
        }
        None => {}
    }
}

enum Resolution {
    Save,
    Discard,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arming_starts_a_full_countdown() {
        let mut arm = Arm::default();
        assert_eq!(arm.state(), ArmState::Idle);
        arm.arm();
        assert_eq!(
            arm.state(),
            ArmState::Counting(COUNTDOWN.as_secs() as u32),
            "the first frame shows the whole count, not one less",
        );
    }

    #[test]
    fn the_countdown_never_shows_a_zero_it_is_not_honouring() {
        // A hair before the end is still "1": showing "0" on a control that
        // hasn't become available yet is the one number it must not display.
        let arm = Arm { at: Some(Instant::now() - COUNTDOWN + Duration::from_millis(10)) };
        assert_eq!(arm.state(), ArmState::Counting(1));
    }

    #[test]
    fn the_confirm_becomes_available_at_zero() {
        let arm = Arm { at: Some(Instant::now() - COUNTDOWN) };
        assert_eq!(arm.state(), ArmState::Ready);
    }

    #[test]
    fn an_armed_control_never_expires() {
        // The whole reason there is no arm window: a clock guessing at whether
        // you are still engaged guesses wrong in the dangerous direction. It
        // sits in `Ready` until it fires or something else is clicked.
        let arm = Arm { at: Some(Instant::now() - Duration::from_secs(60 * 60)) };
        assert_eq!(arm.state(), ArmState::Ready, "an hour later it is still waiting for you");
    }

    #[test]
    fn disarming_puts_it_all_the_way_back() {
        let mut arm = Arm::default();
        arm.arm();
        assert!(arm.armed());
        arm.disarm();
        assert_eq!(arm.state(), ArmState::Idle);
        assert!(!arm.armed());
    }

    #[test]
    fn a_double_click_cannot_reach_the_confirm() {
        // The guard, stated: the control spends the whole double-click interval
        // disabled, so the second click of a double-click has nothing to hit.
        let mut arm = Arm::default();
        arm.arm();
        assert!(
            matches!(arm.state(), ArmState::Counting(_)),
            "still counting an instant later, so the confirm isn't there to be hit",
        );
        assert!(
            COUNTDOWN >= Duration::from_millis(600),
            "the countdown must outlast any plausible double-click interval",
        );
    }
}
