//! Shutdown helpers for background worker threads.
//!
//! Several subsystems (stage sync, Hue, WLED) own a thread that does network
//! I/O in its loop. On app close their owners `join()` the thread — but if the
//! thread is parked in a long/blocking network call (e.g. the stage-sync
//! worker mid-`pull_stage`, which has a 15 s request timeout), a plain
//! `join()` blocks the close for the whole call. That's what makes REACT
//! "freeze" when you quit.
//!
//! These workers have no critical finalization on exit, so the right tradeoff
//! is: ask the thread to stop, wait a brief grace period for a clean exit, and
//! if it's still stuck in a call, abandon it — the process exit reclaims it.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Wait up to `grace` for `handle` to finish, then return. If it finished, it
/// is joined cleanly; if it's still running (stuck in a blocking call), it is
/// ABANDONED — the OS reclaims it when the process exits. Never blocks longer
/// than `grace`, so app close stays snappy.
pub fn join_or_abandon(handle: JoinHandle<()>, grace: Duration) {
    let deadline = Instant::now() + grace;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            // Still in a blocking call — abandon it (don't join; process exit
            // reclaims the thread). Dropping the handle here detaches it.
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = handle.join();
}

/// The grace period app-close gives a worker thread to exit cleanly before
/// abandoning it. Long enough for an idle thread to see its stop message,
/// short enough that quitting never feels frozen.
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(300);
