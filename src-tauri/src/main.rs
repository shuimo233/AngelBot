#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = angelbot::window_capture_helper_exit_code() {
        std::process::exit(code);
    }
    angelbot::run()
}
