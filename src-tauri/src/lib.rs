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
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Fatal error: failed to build tokio runtime: {e}");
            std::process::exit(1);
        }
    };

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

    // Clone before builder chain — setup closure takes ownership of `bridge`.
    let exit_bridge = bridge.clone();

    let builder = tauri::Builder::default()
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let config = phoneme_core::Config::read_or_default();
                if config.tray.minimize_to_tray {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    use phoneme_core::RecordMode;
                    use tauri::Manager;
                    use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};

                    let bridge = app.state::<Option<Bridge>>().inner().clone();
                    if let Some(bridge) = bridge {
                        // Read live config so toggle/combo changes apply immediately.
                        let current_config = phoneme_core::Config::read_or_default();

                        // If the fired shortcut is the (enabled) meeting hotkey,
                        // toggle a meeting on press and we're done. Meetings are
                        // always toggle — Hold mode makes no sense for them.
                        let meeting_combo = if current_config.meeting_hotkey.enabled {
                            current_config.meeting_hotkey.combo.parse::<Shortcut>().ok()
                        } else {
                            None
                        };
                        if meeting_combo.as_ref() == Some(shortcut) {
                            if event.state() == ShortcutState::Pressed {
                                let bridge = bridge.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Err(e) =
                                        bridge.request(phoneme_ipc::Request::MeetingToggle).await
                                    {
                                        tracing::error!(
                                            "failed to toggle meeting from hotkey: {e}"
                                        );
                                    }
                                });
                            }
                            return;
                        }

                        let in_place_combo = if current_config.in_place_hotkey.enabled {
                            current_config
                                .in_place_hotkey
                                .combo
                                .parse::<Shortcut>()
                                .ok()
                        } else {
                            None
                        };
                        if in_place_combo.as_ref() == Some(shortcut) {
                            let mode = current_config.in_place_hotkey.mode;
                            match event.state() {
                                ShortcutState::Pressed => {
                                    tauri::async_runtime::spawn(async move {
                                        if mode == phoneme_core::config::HotkeyMode::Toggle {
                                            if let Err(e) = bridge
                                                .request(phoneme_ipc::Request::RecordToggle {
                                                    in_place: true,
                                                })
                                                .await
                                            {
                                                tracing::error!(
                                                    "failed to toggle in-place record: {e}"
                                                );
                                            }
                                        } else {
                                            if let Err(e) = bridge
                                                .request(phoneme_ipc::Request::RecordStart {
                                                    mode: RecordMode::Hold,
                                                    in_place: true,
                                                })
                                                .await
                                            {
                                                tracing::error!(
                                                    "failed to start in-place record: {e}"
                                                );
                                            }
                                        }
                                    });
                                }
                                ShortcutState::Released => {
                                    tauri::async_runtime::spawn(async move {
                                        if mode == phoneme_core::config::HotkeyMode::Hold {
                                            if let Err(e) = bridge
                                                .request(phoneme_ipc::Request::RecordStop)
                                                .await
                                            {
                                                tracing::error!(
                                                    "failed to stop in-place record: {e}"
                                                );
                                            }
                                        }
                                    });
                                }
                            }
                            return;
                        }

                        let mode = current_config.hotkey.mode;

                        match event.state() {
                            ShortcutState::Pressed => {
                                tauri::async_runtime::spawn(async move {
                                    if mode == phoneme_core::config::HotkeyMode::Toggle {
                                        if let Err(e) = bridge
                                            .request(phoneme_ipc::Request::RecordToggle {
                                                in_place: false,
                                            })
                                            .await
                                        {
                                            tracing::error!(
                                                "failed to toggle record from hotkey: {e}"
                                            );
                                        }
                                    } else {
                                        if let Err(e) = bridge
                                            .request(phoneme_ipc::Request::RecordStart {
                                                mode: RecordMode::Hold,
                                                in_place: false,
                                            })
                                            .await
                                        {
                                            tracing::error!(
                                                "failed to start record from hotkey: {e}"
                                            );
                                        }
                                    }
                                });
                            }
                            ShortcutState::Released => {
                                tauri::async_runtime::spawn(async move {
                                    if mode == phoneme_core::config::HotkeyMode::Hold {
                                        if let Err(e) =
                                            bridge.request(phoneme_ipc::Request::RecordStop).await
                                        {
                                            tracing::error!(
                                                "failed to stop record from hotkey: {e}"
                                            );
                                        }
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

                if bridge.config.interface.strip_titlebar {
                    use tauri::Manager;
                    if let Some(window) = app.handle().get_webview_window("main") {
                        let _ = window.set_decorations(false);
                    }
                }

                if bridge.config.tray.show_on_startup {
                    use tauri::Manager;
                    if let Some(window) = app.handle().get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }

                // Register all enabled global hotkeys via the shared helper, so
                // startup and config-save/profile-switch stay in lockstep.
                commands::register_hotkeys(app.handle(), &bridge.config);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_recordings,
            commands::semantic_search,
            commands::get_recording,
            commands::list_meeting,
            commands::delete_recording,
            commands::record_start,
            commands::record_stop,
            commands::record_cancel,
            commands::start_meeting,
            commands::stop_meeting,
            commands::record_pause,
            commands::record_resume,
            commands::record_status,
            commands::retranscribe_recording,
            commands::import_recording,
            commands::refire_hook,
            commands::rerun_cleanup,
            commands::update_transcript,
            commands::update_meeting_name,
            commands::get_original_transcript,
            commands::update_notes,
            commands::daemon_status,
            commands::read_config,
            commands::write_config,
            commands::config_exists,
            commands::config_path,
            commands::list_profiles,
            commands::save_profile,
            commands::switch_profile,
            commands::delete_profile,
            commands::doctor_local_checks,
            commands::doctor_backend_checks,
            commands::start_daemon,
            commands::wizard_test_whisper,
            commands::wizard_test_hook,
            commands::list_input_devices,
            commands::list_tags,
            commands::list_all_tags,
            commands::add_tag,
            commands::update_tag,
            commands::delete_tag,
            commands::attach_tag,
            commands::detach_tag,
            commands::tags_for,
            commands::wizard_download_model,
            commands::wizard_download_semantic_model,
            commands::wizard_download_diarization_model,
            commands::wizard_get_system_info,
            commands::wizard_list_downloaded_models,
            commands::wizard_download_server,
            commands::wizard_ping_ollama,
            commands::wizard_detect_deps,
            commands::wizard_pull_ollama_model,
            commands::wizard_download_file,
            commands::wizard_run_installer,
            commands::reveal_file,
            commands::open_file,
            commands::open_hooks_folder,
            commands::read_file_string,
        ]);

    let builder = builder
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Fatal error while building tauri application: {e}");
            std::process::exit(1);
        });

    builder.run(move |_app, event| {
        if let tauri::RunEvent::Exit = event {
            // Send a clean Shutdown to the daemon before the process exits.
            // This tells the daemon to stop whisper-server and flush any
            // in-flight queue work before it exits. We give it 3 seconds;
            // if it doesn't respond in time we exit anyway.
            if let Some(ref b) = exit_bridge {
                let b = b.clone();
                let _ = runtime.block_on(async move {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        b.request(phoneme_ipc::Request::Shutdown),
                    )
                    .await
                    .ok()
                });
            }
        }
    });
}
