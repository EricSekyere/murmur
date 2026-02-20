// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(e) = voitex_app_lib::run() {
        eprintln!("Voitex fatal error: {:#}", e);
        std::process::exit(1);
    }
}
