//! `output::main_thread` against a real main thread. libtest runs tests on
//! worker threads, so this binary has no harness: `main` owns the main thread
//! and pumps its run loop, as the app's AppKit event loop does. (Not
//! `dispatch_main`, which parks the main thread and drains the main queue on a
//! pool thread instead.)

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const std::ffi::c_void;
    fn CFRunLoopRunInMode(
        mode: *const std::ffi::c_void,
        seconds: f64,
        return_after_source_handled: u8,
    ) -> i32;
}

#[cfg(target_os = "macos")]
fn main() {
    use murmur_core::output::main_thread::{is_main_thread, run};
    use std::time::{Duration, Instant};

    assert!(is_main_thread(), "test binary main() is the main thread");
    let inline = run(|| 7, Duration::from_millis(50));
    assert_eq!(
        inline.ok(),
        Some(7),
        "a call already on the main thread runs inline instead of queueing behind itself"
    );

    std::thread::spawn(|| {
        let result = run(is_main_thread, Duration::from_secs(5));
        if !matches!(result, Ok(true)) {
            eprintln!("expected work from a worker to run on the main thread, got {result:?}");
            std::process::exit(1);
        }
        std::process::exit(0);
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        // SAFETY: called on the main thread with CoreFoundation's own mode constant.
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0) };
    }
    eprintln!("the worker never finished");
    std::process::exit(1);
}

#[cfg(not(target_os = "macos"))]
fn main() {}
