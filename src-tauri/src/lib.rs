//! Phoneme tray app — Tauri 2 desktop shell.
//!
//! Plan 4 Task 1 stub: bare Tauri builder with the shell plugin. Subsequent
//! tasks add the IPC bridge (Task 2), Tauri commands (Task 3), tray (Task 4),
//! and event subscription bridge (Task 5).

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
