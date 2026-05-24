//! Phoneme tray app — Tauri 2 desktop shell.

mod auto_spawn;
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
        let config = config_io::read().unwrap_or_default();
        if let Err(e) = auto_spawn::ensure_running(&config).await {
            tracing::warn!(error = %e, "could not auto-spawn daemon");
        }
        match Bridge::connect(config).await {
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
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    use phoneme_core::RecordMode;
                    use tauri::Manager;
                    use tauri_plugin_global_shortcut::ShortcutState;

                    let bridge = app.state::<Option<Bridge>>().inner().clone();
                    if let Some(bridge) = bridge {
                        let config = bridge.config.clone();
                        let _hotkey_enabled = config.hotkey.enabled;
                        let _hotkey_combo = config.hotkey.combo.clone();

                        // We only care if they match the configured shortcut
                        // Since we register exactly one shortcut below, it should match.
                        match event.state() {
                            ShortcutState::Pressed => {
                                tauri::async_runtime::spawn(async move {
                                    if let Err(e) = bridge
                                        .request(phoneme_ipc::Request::RecordStart {
                                            mode: RecordMode::Hold,
                                        })
                                        .await
                                    {
                                        tracing::error!("failed to start record from hotkey: {e}");
                                    }
                                });
                            }
                            ShortcutState::Released => {
                                tauri::async_runtime::spawn(async move {
                                    if let Err(e) =
                                        bridge.request(phoneme_ipc::Request::RecordStop).await
                                    {
                                        tracing::error!("failed to stop record from hotkey: {e}");
                                    }
                                });
                            }
                        }
                    }
                })
                .build(),
        )
        .manage(bridge.clone())
        .setup(move |app| {
            let _tray = tray::install(app.handle())?;
            if let Some(bridge) = bridge.clone() {
                events::spawn(app.handle().clone(), bridge.clone());

                if bridge.config.tray.show_on_startup {
                    use tauri::Manager;
                    if let Some(window) = app.handle().get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }

                if bridge.config.hotkey.enabled {
                    use std::str::FromStr;
                    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
                    if let Ok(shortcut) = Shortcut::from_str(&bridge.config.hotkey.combo) {
                        if let Err(e) = app.handle().global_shortcut().register(shortcut) {
                            tracing::error!("failed to register global hotkey: {e}");
                        }
                    }
                }
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
            commands::wizard_test_whisper,
            commands::wizard_test_hook,
            commands::list_input_devices,
            commands::list_tags,
            commands::add_tag,
            commands::attach_tag,
            commands::detach_tag,
            commands::tags_for,
            commands::wizard_download_model,
            commands::wizard_download_server,
            commands::reveal_file,
            commands::open_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
