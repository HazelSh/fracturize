//! Waiting on the GPU without trusting it to come back.
//!
//! An offline render waits for the GPU dozens of times a second, and every one
//! of those waits used to be `device.poll(Wait { timeout: None })` with the
//! result thrown away. That is fine until the driver kills the job's channel:
//! on 2026-11-05 a 4K render hit an NVIDIA Xid 13 (a compute shader fault)
//! 22% in, and the job thread then waited **six minutes** on a fence that was
//! never going to signal — the dialog's bar frozen, Stop unable to land,
//! because it is only checked between waits. When wgpu finally noticed, three
//! more things went wrong in a row: `Device::poll` does not return device loss
//! as an error, it panics (`handle_error_fatal`, wgpu 29); the panic unwound
//! the job thread, dropping every buffer on the dead device; and freeing them
//! segfaulted inside `libnvidia-glcore`, taking the whole app with it.
//!
//! So a wait here does three things a bare poll doesn't:
//!
//! * **It waits in steps** ([`STEP`]), so between steps it can notice that the
//!   device was lost, that the person pressed Stop, or that nothing has come
//!   back for [`STALL`].
//! * **It catches the poll's panic** and treats it as the device loss it is.
//! * **It never unwinds past a dead device.** A render job reports the error
//!   to its dialog and then parks its thread for good, so nothing on the dead
//!   device is ever freed; the CLI prints the error and exits without running
//!   destructors. The memory the job held stays held until the app restarts,
//!   which is a small price against a segfault, and the message says so.
//!
//! The checks only apply on a thread that made an offline device
//! ([`crate::offline::create_device`] calls [`watch`]). On any other thread —
//! the window's — [`idle`] is exactly the plain wait it replaced.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one poll blocks before the wait looks up.
///
/// A responsiveness figure, not a failure threshold: a step that times out is
/// ordinary, and the wait simply polls again. One second is short enough that
/// Stop and device loss are acted on promptly, and long enough that the extra
/// wake-ups cost nothing measurable. Ordinary waits end well inside it: on the
/// GTX 1080 the longest measured is a lap's splat-and-fold at 40M points.
pub const STEP: Duration = Duration::from_secs(1);

/// How long one wait may go with nothing coming back before the job is failed.
///
/// Deliberately far above anything a working GPU takes. The longest waits are
/// a lap's splat of the whole point buffer into a large tile and a tile's
/// final resolve and readback; on the T490's integrated GPU those can run to
/// seconds. A hung channel never comes back at all, so there is no middle
/// ground to tune into: a generous figure only delays the report of a hang,
/// while a tight one fails real renders.
pub const STALL: Duration = Duration::from_secs(60);

/// After this long, a wait the person has asked to Stop is abandoned rather
/// than waited out. Shorter than [`STALL`] because the person is now waiting on
/// it too, and long enough that an ordinary slow wait finishes first.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// What this thread is watching: the device's loss report, and the job (if
/// this is a render job) to tell when the device can't go on.
struct Watch {
    lost: Arc<Mutex<Option<String>>>,
    job: Option<crate::render_job::JobControl>,
}

thread_local! {
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
}

/// Watch `device` for loss, from this thread. Called once per offline device,
/// right after it is made; a later device on the same thread replaces it.
pub fn watch(device: &wgpu::Device) {
    let lost = Arc::new(Mutex::new(None));
    let slot = lost.clone();
    device.set_device_lost_callback(move |reason, msg| {
        // Dropping a device reports `Destroyed`; that is the end of a render,
        // not a failure of one.
        if reason == wgpu::DeviceLostReason::Unknown {
            log::error!("GPU device lost: {msg}");
            if let Ok(mut s) = slot.lock() {
                s.get_or_insert(msg);
            }
        }
    });
    WATCH.with(|w| {
        let job = w.borrow_mut().take().and_then(|old| old.job);
        *w.borrow_mut() = Some(Watch { lost, job });
    });
}

/// Mark this thread as running `job`, so a GPU failure is reported to its
/// dialog rather than to stderr. Call before the job creates its device.
pub fn attach_job(job: crate::render_job::JobControl) {
    WATCH.with(|w| {
        let mut w = w.borrow_mut();
        match w.as_mut() {
            Some(watch) => watch.job = Some(job),
            None => *w = Some(Watch { lost: Arc::new(Mutex::new(None)), job: Some(job) }),
        }
    });
}

/// Wait until all submitted work on `device` has finished.
///
/// Returns only when it has. If it can't — the device was lost, nothing came
/// back for [`STALL`], or the person stopped a job whose GPU isn't answering —
/// this does not return at all; see the module docs for why.
pub fn idle(device: &wgpu::Device) {
    let watched = WATCH.with(|w| w.borrow().as_ref().map(|w| w.lost.clone()));
    let Some(lost) = watched else {
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        return;
    };
    let started = Instant::now();
    let mut warned = false;
    loop {
        if let Some(msg) = lost.lock().ok().and_then(|s| s.clone()) {
            fail(lost_message(&msg));
        }
        let poll = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(STEP) })
        }));
        let waited = started.elapsed();
        match step(poll, waited, stop_requested()) {
            Step::Done => {
                if waited >= Duration::from_millis(100) {
                    log::debug!("GPU wait took {:.3}s", waited.as_secs_f64());
                }
                return;
            }
            Step::Again => {
                if !warned && waited >= Duration::from_secs(10) {
                    warned = true;
                    log::warn!("still waiting on the GPU after {:.0}s", waited.as_secs_f64());
                }
            }
            Step::Fail(msg) => {
                // A panic in poll is the device going; prefer the driver's own
                // words if the callback has them by now.
                let msg = lost.lock().ok().and_then(|s| s.clone()).map(|m| lost_message(&m)).unwrap_or(msg);
                fail(msg)
            }
        }
    }
}

/// What one step of [`idle`] concluded. Separate so the decision can be
/// tested without a GPU that misbehaves on request.
#[derive(Debug, PartialEq)]
enum Step {
    Done,
    Again,
    Fail(String),
}

fn step(
    poll: std::thread::Result<Result<wgpu::PollStatus, wgpu::PollError>>,
    waited: Duration,
    stop_requested: bool,
) -> Step {
    match poll {
        Ok(Ok(_)) => Step::Done,
        Ok(Err(wgpu::PollError::Timeout)) => {
            if waited >= STALL {
                Step::Fail(format!(
                    "the GPU stopped responding: nothing came back for {}s. This is \
                     usually a driver fault; the system log (journalctl -k) may say \
                     more. Restart Fracturize to free the memory the render held.",
                    STALL.as_secs()
                ))
            } else if stop_requested && waited >= STOP_GRACE {
                Step::Fail(format!(
                    "stopped while the GPU wasn't responding ({:.0}s with nothing back), \
                     so nothing was written. Restart Fracturize to free the memory the \
                     render held.",
                    waited.as_secs_f64()
                ))
            } else {
                Step::Again
            }
        }
        Ok(Err(e)) => Step::Fail(format!("waiting on the GPU failed: {e}")),
        Err(_) => Step::Fail(lost_message("the device was lost")),
    }
}

fn lost_message(detail: &str) -> String {
    format!(
        "the GPU device was lost ({detail}), usually a driver fault; the system log \
         (journalctl -k) may say more. Restart Fracturize to free the memory the \
         render held."
    )
}

fn stop_requested() -> bool {
    WATCH.with(|w| {
        w.borrow()
            .as_ref()
            .and_then(|w| w.job.as_ref())
            .is_some_and(|j| j.cancel.load(std::sync::atomic::Ordering::Relaxed))
    })
}

/// End this thread's render without unwinding through the dead device.
fn fail(msg: String) -> ! {
    log::error!("Render failed: {msg}");
    let job = WATCH.with(|w| w.borrow().as_ref().and_then(|w| w.job.clone()));
    match job {
        Some(job) => {
            let _ = job.events.send(crate::render_job::JobEvent::Done(Err(msg)));
            // Parked, not returned: returning would drop every buffer the job
            // holds on a device the driver has killed, and that is what
            // segfaulted. The thread and its memory go when the app does.
            loop {
                std::thread::park();
            }
        }
        None => {
            eprintln!("error: {msg}");
            // `exit` runs no destructors, which is the point.
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed_out() -> std::thread::Result<Result<wgpu::PollStatus, wgpu::PollError>> {
        Ok(Err(wgpu::PollError::Timeout))
    }

    #[test]
    fn a_finished_wait_is_done() {
        assert_eq!(step(Ok(Ok(wgpu::PollStatus::QueueEmpty)), STALL * 2, true), Step::Done);
    }

    #[test]
    fn a_slow_wait_is_waited_out() {
        assert_eq!(step(timed_out(), STALL - STEP, false), Step::Again);
    }

    #[test]
    fn a_stalled_wait_fails() {
        assert!(matches!(step(timed_out(), STALL, false), Step::Fail(m) if m.contains("stopped responding")));
    }

    #[test]
    fn stop_waits_out_the_grace_first() {
        assert_eq!(step(timed_out(), STOP_GRACE - STEP, true), Step::Again);
        assert!(matches!(step(timed_out(), STOP_GRACE, true), Step::Fail(m) if m.contains("nothing was written")));
    }

    #[test]
    fn a_panicking_poll_is_device_loss() {
        let poll = std::panic::catch_unwind(|| -> Result<wgpu::PollStatus, wgpu::PollError> {
            panic!("Parent device is lost")
        });
        assert!(matches!(step(poll, Duration::ZERO, false), Step::Fail(m) if m.contains("device was lost")));
    }

    #[test]
    fn the_steps_fit_inside_the_stall() {
        assert!(STEP < STOP_GRACE && STOP_GRACE < STALL);
    }
}
