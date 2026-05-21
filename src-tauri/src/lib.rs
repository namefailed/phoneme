//! Phoneme tray app — Tauri 2 desktop shell.

mod bridge;

use bridge::Bridge;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let bridge = runtime.block_on(async {
        match Bridge::connect(phoneme_core::Config::default()).await {
            Ok(b) => Some(b),
            Err(e) => {
                tracing::warn!(error = %e, "could not connect to daemon at startup; will retry on first action");
                None
            }
        }
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(bridge)
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
