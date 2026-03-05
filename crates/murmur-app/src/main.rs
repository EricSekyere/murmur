// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Install a global panic hook BEFORE anything else.
    // This catches panics on ANY thread (audio worker, streaming, rdev, etc.)
    // and writes them to crash.log + shows a dialog in release mode.
    #[cfg(debug_assertions)]
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");

        let payload = if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else {
            "unknown panic payload".to_string()
        };

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let msg = format!(
            "Murmur panicked!\n\nThread: {}\nLocation: {}\nMessage: {}",
            thread_name, location, payload
        );

        // Always write to crash log
        write_crash_log(&msg);

        // Show error dialog in release mode so the user knows why it crashed.
        // In debug mode, the default hook will print to console.
        #[cfg(not(debug_assertions))]
        {
            show_error_dialog(&msg);
        }

        // Only call the default hook in debug builds.
        // In Windows release GUI mode, stderr may be closed and default_hook can panic
        // with "failed printing to stderr".
        #[cfg(debug_assertions)]
        {
            default_hook(info);
        }
    }));

    if let Err(e) = murmur_app_lib::run() {
        let msg = format!("Murmur fatal error:\n\n{:#}", e);
        write_stderr_safe(&msg);
        write_crash_log(&msg);
        show_error_dialog(&msg);
        std::process::exit(1);
    }
}

/// Write crash details to a log file so users can report bugs.
fn write_crash_log(msg: &str) {
    // Use APPDATA on Windows, HOME/.local/share on Unix
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("XDG_DATA_HOME"))
        .or_else(|_| std::env::var("HOME").map(|h| format!("{}/.local/share", h)));

    if let Ok(base_dir) = base {
        let log_dir = std::path::PathBuf::from(base_dir).join("murmur");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("crash.log");
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = format!("[{}] {}\n", timestamp, msg);
        // Append rather than overwrite so we capture multiple crashes
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = file.write_all(entry.as_bytes());
        }
    }
}

/// Best-effort stderr write that never panics if stderr is unavailable.
fn write_stderr_safe(msg: &str) {
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{}", msg);
}

/// Show a native error dialog so the user sees what went wrong.
#[cfg(windows)]
fn show_error_dialog(msg: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    unsafe extern "system" {
        fn MessageBoxW(hwnd: *const (), text: *const u16, caption: *const u16, utype: u32) -> i32;
    }

    let text: Vec<u16> = OsStr::new(msg).encode_wide().chain(Some(0)).collect();
    let caption: Vec<u16> = OsStr::new("Murmur Error")
        .encode_wide()
        .chain(Some(0))
        .collect();

    unsafe {
        MessageBoxW(std::ptr::null(), text.as_ptr(), caption.as_ptr(), 0x10);
    }
}

#[cfg(not(windows))]
fn show_error_dialog(_msg: &str) {
    // On non-Windows, eprintln is sufficient (console is visible)
}
