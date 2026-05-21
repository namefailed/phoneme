//! Phoneme tray app entrypoint.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    phoneme_tray_lib::run();
}
