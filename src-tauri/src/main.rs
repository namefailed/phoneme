//! Phoneme tray app entrypoint — release builds use the windows subsystem
//! (no console window); everything real lives in `phoneme_tray_lib::run`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    phoneme_tray_lib::run();
}
