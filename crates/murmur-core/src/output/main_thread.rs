//! Run work on the main thread.
//!
//! Since macOS 27, the Text Input Source APIs trap (`EXC_BREAKPOINT` in
//! `_dispatch_assert_queue_fail`) when called off the main thread. enigo calls
//! them to turn a character such as the `v` of Cmd+V into a key code, so every
//! keystroke Murmur synthesizes has to happen on the main thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use dispatch2::DispatchQueue;

const PENDING: u8 = 0;
const STARTED: u8 = 1;
const ABANDONED: u8 = 2;

unsafe extern "C" {
    fn pthread_main_np() -> std::ffi::c_int;
}

/// Whether the calling thread is the process's main thread.
pub fn is_main_thread() -> bool {
    // SAFETY: takes no arguments and only reads the calling thread's identity.
    unsafe { pthread_main_np() == 1 }
}

/// Run `work` on the main thread and return its result.
///
/// Runs inline when already on the main thread. Otherwise the work is queued on
/// the main dispatch queue, which the app's event loop drains.
///
/// # Errors
/// When the main thread does not start the work within `timeout`, e.g. a CLI
/// whose main thread never services its queue. The work is then abandoned and
/// never runs, so a caller can fall back without doing it twice.
pub fn run<R, F>(work: F, timeout: Duration) -> Result<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    if is_main_thread() {
        return Ok(work());
    }

    let state = Arc::new(AtomicU8::new(PENDING));
    let job_state = Arc::clone(&state);
    let (sender, receiver) = mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        if job_state
            .compare_exchange(PENDING, STARTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // The receiver waits for any job that started, so this only fails
            // if the caller's thread is already gone.
            let _ = sender.send(work());
        }
    });

    match receiver.recv_timeout(timeout) {
        Ok(result) => Ok(result),
        Err(RecvTimeoutError::Timeout) => {
            let abandoned = state
                .compare_exchange(PENDING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            if abandoned {
                bail!("the main thread did not run queued work within {timeout:?}");
            }
            // It started just as the wait ran out: it is typing now, so wait for
            // it rather than let the caller fall back and output twice.
            receiver
                .recv()
                .context("main-thread work ended without a result")
        }
        Err(RecvTimeoutError::Disconnected) => {
            bail!("main-thread work ended without a result")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn gives_up_instead_of_hanging_when_nothing_services_the_main_queue() {
        // libtest's main thread only waits on test results; nothing drains the
        // main dispatch queue, as in a CLI whose main thread is busy.
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let result = std::thread::spawn(move || {
            run(
                move || flag.store(true, Ordering::SeqCst),
                Duration::from_millis(50),
            )
        })
        .join()
        .unwrap();

        assert!(result.is_err());
        assert!(!ran.load(Ordering::SeqCst));
    }
}
