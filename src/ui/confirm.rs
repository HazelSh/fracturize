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

/// How long after arming the second click starts being accepted.
///
/// Long enough that no double-click can span it (the OS threshold is
/// typically 400-500ms and the *interval* far shorter), short enough not to
/// feel punitive. The whole point of the guard is that the two clicks of a
/// double-click cannot both land, so this floor is the guard.
pub const MIN_WAIT: Duration = Duration::from_secs(1);

/// How long an armed control stays armed before going back to harmless.
///
/// Scaled to the gesture, not to the worst case. Reading a button that has just
/// relabelled itself and clicking it again is a second or so of human time —
/// simple reaction is a quarter of that, and the rest is reading and deciding —
/// so a window of three seconds is already two to three times what the act
/// needs, and the *usable* slot after [`MIN_WAIT`] is the two seconds either
/// side of that. Longer isn't safer: an armed control is a control one click
/// from firing, and holding it in that state for half a minute after the person
/// has looked away is the risk, not the protection.
///
/// The cost is on a machine so degraded it isn't repainting: there, a click can
/// arrive after the window has closed and will re-arm rather than confirm, so
/// the job takes an extra click or two to stop. That is a bounded annoyance,
/// and it is bounded in the right direction — the failure is "it didn't cancel
/// yet", not "it cancelled when I didn't mean it to". If it ever bites, the
/// better fix is to disarm on the pointer *leaving* the button rather than on a
/// clock, which reads disengagement directly instead of guessing at it.
pub const ARM_WINDOW: Duration = Duration::from_secs(3);

/// What an [`Arm`] currently is, as far as the label is concerned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArmState {
    /// Not armed: a click here only arms.
    Idle,
    /// Armed, but inside [`MIN_WAIT`] — a click here is refused.
    Waiting,
    /// Armed and past the wait: the next click fires.
    Ready,
}

/// A click-wait-click confirmation: the first click arms, and the second is
/// accepted only once a wall-clock minimum has passed.
///
/// Everything here is evaluated **at click time against the wall clock**, never
/// against a frame counter and never against "is the button currently *drawn*
/// as ready". On a box that has stopped compositing, the repaint that would
/// have re-labelled the button may not have happened yet even though the timer
/// fired long ago; the click still has to be accepted on its own merits. Two
/// discrete events and a timer survive that. A hold-to-confirm gesture with a
/// fill animation — the obvious alternative — degrades precisely when it is
/// needed most, since both its feedback channel and its input model assume a
/// responsive frame loop.
#[derive(Default)]
pub struct Arm {
    at: Option<Instant>,
}

impl Arm {
    pub fn state(&self) -> ArmState {
        match self.at {
            Some(t) if t.elapsed() >= ARM_WINDOW => ArmState::Idle,
            Some(t) if t.elapsed() >= MIN_WAIT => ArmState::Ready,
            Some(_) => ArmState::Waiting,
            None => ArmState::Idle,
        }
    }

    /// Register a click. `true` means *confirmed, do the thing*.
    ///
    /// A click during [`ArmState::Waiting`] is neither a confirmation nor a
    /// re-arm: it is swallowed, leaving the original arm time in place. Were it
    /// to re-arm, an impatient click every 900ms would hold the control
    /// permanently out of reach.
    pub fn click(&mut self) -> bool {
        match self.state() {
            ArmState::Ready => {
                self.at = None;
                true
            }
            ArmState::Waiting => false,
            ArmState::Idle => {
                self.at = Some(Instant::now());
                false
            }
        }
    }

    pub fn disarm(&mut self) {
        self.at = None;
    }

    /// The button's text for this state.
    ///
    /// Three discrete strings rather than an animation: a label change reads
    /// correctly at one frame per second, and a smooth fill does not.
    pub fn label(&self, idle: &str) -> String {
        match self.state() {
            ArmState::Idle => idle.to_string(),
            ArmState::Waiting => format!("{}? wait…", idle),
            ArmState::Ready => format!("{}? click again", idle),
        }
    }

    /// The tooltip that goes with [`label`](Self::label).
    pub fn hint(&self, idle: &str) -> String {
        match self.state() {
            ArmState::Idle => format!("{} — click once to arm, then again to confirm", idle),
            ArmState::Waiting => "Armed. Click again in a moment to confirm.".to_string(),
            ArmState::Ready => "Click again to confirm. Move away to cancel.".to_string(),
        }
    }
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

            // The one destructive button in the dialog gets the same
            // click-wait-click guard as render-cancel, and for the same reason:
            // it is reached at the exact moment a person is clicking fast.
            let label = app.ui_state.discard_arm.label("Discard");
            let hint = app.ui_state.discard_arm.hint("Discard");
            let resp = ui.button(label);
            let resp = hinted(resp, &mut app.ui_state, hint, "click twice: discard the edits");
            if resp.clicked() && app.ui_state.discard_arm.click() {
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
            app.proceed_with_pending();
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
    fn a_double_click_cannot_confirm() {
        // The whole point: two clicks 20ms apart must not get through.
        let mut arm = Arm::default();
        assert!(!arm.click(), "the first click only arms");
        assert!(!arm.click(), "a double-click's second click is refused");
        assert_eq!(arm.state(), ArmState::Waiting, "and it stays armed, not re-armed");
    }

    #[test]
    fn a_click_after_the_wait_confirms_and_disarms() {
        let mut arm = Arm { at: Some(Instant::now() - MIN_WAIT) };
        assert_eq!(arm.state(), ArmState::Ready);
        assert!(arm.click(), "past the minimum wait, the second click fires");
        assert_eq!(arm.state(), ArmState::Idle, "firing disarms");
    }

    #[test]
    fn an_impatient_click_does_not_push_the_deadline_back() {
        // If a refused click re-armed, clicking every 900ms would hold the
        // control out of reach forever.
        let mut arm = Arm { at: Some(Instant::now() - MIN_WAIT / 2) };
        assert!(!arm.click());
        let waited = arm.at.expect("still armed").elapsed();
        assert!(waited >= MIN_WAIT / 2, "the original arm time survives the refusal");
    }

    #[test]
    fn an_old_arm_has_gone_back_to_harmless() {
        let mut arm = Arm { at: Some(Instant::now() - ARM_WINDOW) };
        assert_eq!(arm.state(), ArmState::Idle);
        assert!(!arm.click(), "an expired arm's next click arms afresh, it does not fire");
    }

    #[test]
    fn the_label_says_which_of_the_three_states_it_is_in() {
        assert_eq!(Arm::default().label("Cancel"), "Cancel");
        assert_eq!(Arm { at: Some(Instant::now()) }.label("Cancel"), "Cancel? wait…");
        assert_eq!(
            Arm { at: Some(Instant::now() - MIN_WAIT) }.label("Cancel"),
            "Cancel? click again"
        );
    }
}
