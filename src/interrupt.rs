//! Ctrl-C that keeps the work.
//!
//! An accumulating render is an anytime algorithm: the histogram at lap 400 of
//! 1000 is the same picture as at lap 1000, noisier. So killing one partway
//! should hand back what it has — the PNG at whatever sample count it reached,
//! and the checkpoint to carry on from — rather than throwing away an hour.
//!
//! Without this, `--checkpoint`'s promise of writing on abort was only
//! reachable from the render-job dialog, which has its own cancel button. From
//! a terminal, Ctrl-C killed the process before anything could be written,
//! which is exactly the case the feature exists for.
//!
//! ## How it behaves
//!
//! * **First Ctrl-C**: sets a flag. The render finishes the lap it is in, then
//!   stops, saves, and reports itself as partial.
//! * **Second Ctrl-C**: exits immediately. If the save itself is wedged — a
//!   full disk, a network mount — the user must still be able to get out, and
//!   a program that ignores a second interrupt is a program you have to kill
//!   from another terminal.
//!
//! The handler only sets an atomic and, on the second signal, calls `_exit`.
//! Both are async-signal-safe; formatting a message or touching a lock would
//! not be.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU32 = AtomicU32::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle(_sig: i32) {
    // Second one wins immediately: see the module note on being escapable.
    if COUNT.fetch_add(1, Ordering::SeqCst) >= 1 {
        // Not `std::process::exit`: that runs atexit handlers and destructors,
        // which is not async-signal-safe from inside a handler.
        unsafe { libc::_exit(130) };
    }
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Install the handler. Idempotent, and a no-op off unix.
///
/// Called by the long-running render paths only. A `--info` dump or a 0.3 s
/// draft render has nothing to protect and should keep the default behaviour,
/// where Ctrl-C simply ends it.
pub fn install() {
    #[cfg(unix)]
    {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: `handle` only stores to atomics and may call `_exit`, both of
        // which are async-signal-safe.
        unsafe {
            // Via a pointer rather than casting the fn item straight to an
            // integer, which rustc warns about as a footgun in general.
            let h = handle as *const () as libc::sighandler_t;
            libc::signal(libc::SIGINT, h);
            // SIGTERM too: a `kill` or a session teardown should also save.
            libc::signal(libc::SIGTERM, h);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = &INSTALLED;
    }
}

/// Whether an interrupt has been requested since `install`.
pub fn requested() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Forget any interrupt, so one render's Ctrl-C cannot stop the next.
///
/// Matters for the animation and sheet paths, which run many renders in one
/// process: a flag left set would make every subsequent frame stop instantly
/// and report itself partial.
#[allow(dead_code)]
pub fn clear() {
    INTERRUPTED.store(false, Ordering::SeqCst);
    COUNT.store(0, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installing twice must not re-register, and must not lose a flag that is
    /// already set — a second long render in one process would otherwise clear
    /// an interrupt the user had already asked for.
    #[test]
    fn install_is_idempotent_and_does_not_clear() {
        clear();
        install();
        INTERRUPTED.store(true, Ordering::SeqCst);
        install();
        assert!(requested(), "install() must not reset an interrupt already asked for");
        clear();
        assert!(!requested());
    }
}
