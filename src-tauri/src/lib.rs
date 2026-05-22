//! Phoneme tray app — Tauri 2 desktop shell.

mod bridge;
mod commands;
mod config_io;
mod doctor;
mod events;
mod tray;
mod wizard;

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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(bridge.clone())
        .setup(move |app| {
            let _tray = tray::install(app.handle())?;
            if let Some(bridge) = bridge.clone() {
                events::spawn(app.handle().clone(), bridge);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_recordings,
            commands::get_recording,
            commands::delete_recording,
            commands::record_start,
            commands::record_stop,
            commands::record_cancel,
            commands::replay_recording,
            commands::refire_hook,
            commands::update_transcript,
            commands::daemon_status,
            commands::read_config,
            commands::write_config,
            commands::config_exists,
            commands::config_path,
            commands::doctor_local_checks,
            commands::wizard_test_llm,
            commands::wizard_test_hook,
            commands::list_input_devices,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
