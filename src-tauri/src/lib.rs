//! Phoneme tray app — the Tauri 2 desktop shell around `phoneme-daemon`.
//!
//! The tray does no recording or transcription itself; it spawns and talks
//! to the daemon (`auto_spawn` + `bridge`), forwards the WebView's `invoke`
//! calls as IPC requests (`commands`, `similar`), re-emits the daemon's
//! event stream to every webview (`events`), and owns the desktop chrome:
//! tray icon + menu (`tray`), the system-wide live-preview overlay window
//! (`overlay`), global hotkeys, and the first-run wizard's downloads with
//! pinned checksums (`wizard`, `checksums`). Config is read/written
//! atomically by `config_io`; `doctor` re-exports the shared check
//! implementations from phoneme-core.
//!
//! Boot sequence (`run`):
//! 1. Build the tokio runtime, read config, `auto_spawn` the daemon, and
//!    try one bridge connect — failure is tolerated, the `BridgeSlot`
//!    lazily reconnects on the first action.
//! 2. Window chrome: window-state plugin (geometry remembered, visibility
//!    deliberately NOT), titlebar strip, show-on-startup, tray icon.
//! 3. Register the global hotkeys (record / meeting / in-place) via the
//!    shared `commands::register_hotkeys`, and pre-create the hidden
//!    overlay when enabled — none of this needs the daemon, only config,
//!    so a down-at-launch daemon costs nothing but the bridge.
//! 4. Attach the daemon event stream (`events::spawn`) — immediately when
//!    the bridge is up, otherwise from a background retry loop the moment
//!    it connects, so the UI never stays event-dead until a restart.
//! 5. Hand the invoke handler the full command surface and run; the exit
//!    hook sends the daemon a last-resort `Shutdown` honoring
//!    `interface.quit_stops_daemon` and the tray Quit chain's
//!    already-stopped flag (see `tray`).
//!
//! The hotkey handler is sync: it peeks the slot (`current()`), kicks a
//! background connect when empty, and spawns the actual request — pressing
//! a hotkey must never block on a dial.

mod auto_spawn;
mod bridge;
mod checksums;
mod commands;
mod config_io;
mod doctor;
mod events;
mod indicator;
mod overlay;
mod similar;
mod tray;
mod wizard;

use bridge::{Bridge, BridgeSlot};

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

    // The slot is what commands talk to: it retries the connect lazily, so a
    // daemon that was down at launch heals on the first action.
    let bridge = BridgeSlot::new(bridge);
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
        // Remember window positions/sizes but NEVER visibility: the overlay is
        // created hidden and shown only by recording events / the Preview
        // button, and the main window's visibility belongs to the tray logic.
        // (With VISIBLE tracked, a state save taken while the overlay was up
        // made it pop open on every app start.)
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Track everything EXCEPT visibility and decorations. VISIBLE made
                // the overlay pop open on every start. DECORATIONS would persist a
                // "strip system titlebar" → recreate the window frameless on the
                // next launch, and Windows can't re-add a native frame at runtime,
                // so the title bar never came back even after turning the setting
                // off. Decorations are owned by tauri.conf (`decorations: true`)
                // plus the live `setDecorations` strip in App.ts instead.
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::VISIBLE
                        & !tauri_plugin_window_state::StateFlags::DECORATIONS,
                )
                .build(),
        )
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

                    let slot = app.state::<BridgeSlot>().inner().clone();
                    if slot.current().is_none() {
                        // Daemon was down at launch — kick a background connect
                        // so the NEXT hotkey press has a bridge to talk to
                        // (this handler is sync; it can't await the connect).
                        let retry = slot.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = retry.get_or_connect().await;
                        });
                    }
                    if let Some(bridge) = slot.current() {
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
                                                    recipe_id: None,
                                                    whisper_model: None,

                                                    source: None,
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
                                                    recipe_id: None,
                                                    whisper_model: None,

                                                    source: None,
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

                        // Custom keybinds (`config.hotkeys`): match the fired combo
                        // against each enabled binding and dispatch its action +
                        // mode, carrying the binding's recipe + whisper-model so the
                        // daemon resolves THAT recipe / model for the recording it
                        // creates. Checked after the TWO built-ins handled above
                        // (meeting + in-place) — so a custom binding can't shadow
                        // those — but BEFORE the main-record fallthrough below.
                        // The main-record hotkey is that fallthrough, so a custom
                        // binding sharing the main-record combo wins over it
                        // (this loop `return`s before the fallthrough is reached).
                        // `Meeting` bindings toggle a meeting (its recipe/model
                        // apply per-track via the daemon's normal meeting path, not
                        // the single-recording ledger — scoped out here, same as the
                        // built-in meeting hotkey).
                        use phoneme_core::config::{HotkeyAction, HotkeyMode};
                        for binding in &current_config.hotkeys {
                            if !binding.enabled {
                                continue;
                            }
                            let combo = match binding.combo.parse::<Shortcut>() {
                                Ok(c) => c,
                                Err(_) => continue,
                            };
                            if &combo != shortcut {
                                continue;
                            }
                            let recipe_id = {
                                let r = binding.recipe_id.trim();
                                (!r.is_empty()).then(|| r.to_string())
                            };
                            let whisper_model = {
                                let m = binding.whisper_model.trim();
                                (!m.is_empty()).then(|| m.to_string())
                            };
                            // Per-binding capture source (None = the global
                            // [recording].source). Meeting bindings ignore it.
                            let source = binding.source;
                            let action = binding.action;
                            let mode = binding.mode;
                            let in_place = action == HotkeyAction::InPlace;
                            let bridge = bridge.clone();
                            match action {
                                HotkeyAction::Meeting => {
                                    // A meeting binding ignores recipe_id /
                                    // whisper_model / source: a meeting toggles via
                                    // the daemon's normal multi-track path, which
                                    // records both tracks and applies recipe/model
                                    // per-track itself rather than through the
                                    // single-recording ledger. Warn so a user who
                                    // set them on a meeting binding isn't silently
                                    // surprised they had no effect (the field docs
                                    // note this too).
                                    if !binding.recipe_id.trim().is_empty()
                                        || !binding.whisper_model.trim().is_empty()
                                        || binding.source.is_some()
                                    {
                                        tracing::warn!(
                                            binding = %binding.id,
                                            recipe_id = %binding.recipe_id,
                                            whisper_model = %binding.whisper_model,
                                            source = ?binding.source,
                                            "meeting hotkey binding ignores recipe_id / whisper_model / source (meetings resolve these per-track via the daemon, not the single-recording ledger)"
                                        );
                                    }
                                    // Meetings are always toggle (Hold makes no sense).
                                    if event.state() == ShortcutState::Pressed {
                                        tauri::async_runtime::spawn(async move {
                                            if let Err(e) = bridge
                                                .request(phoneme_ipc::Request::MeetingToggle)
                                                .await
                                            {
                                                tracing::error!(
                                                    "failed to toggle meeting from custom keybind: {e}"
                                                );
                                            }
                                        });
                                    }
                                }
                                HotkeyAction::Record | HotkeyAction::InPlace => {
                                    match event.state() {
                                        ShortcutState::Pressed => {
                                            tauri::async_runtime::spawn(async move {
                                                let req = if mode == HotkeyMode::Toggle {
                                                    phoneme_ipc::Request::RecordToggle {
                                                        in_place,
                                                        recipe_id,
                                                        whisper_model,
                                                        source,
                                                    }
                                                } else {
                                                    phoneme_ipc::Request::RecordStart {
                                                        mode: RecordMode::Hold,
                                                        in_place,
                                                        recipe_id,
                                                        whisper_model,
                                                        source,
                                                    }
                                                };
                                                if let Err(e) = bridge.request(req).await {
                                                    tracing::error!(
                                                        "failed to start/toggle from custom keybind: {e}"
                                                    );
                                                }
                                            });
                                        }
                                        ShortcutState::Released => {
                                            // Hold bindings stop on release; the stop
                                            // carries no overrides (they were attached
                                            // on the start half).
                                            if mode == HotkeyMode::Hold {
                                                tauri::async_runtime::spawn(async move {
                                                    if let Err(e) = bridge
                                                        .request(phoneme_ipc::Request::RecordStop)
                                                        .await
                                                    {
                                                        tracing::error!(
                                                            "failed to stop from custom keybind: {e}"
                                                        );
                                                    }
                                                });
                                            }
                                        }
                                    }
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
                                                recipe_id: None,
                                                whisper_model: None,

                                                source: None,
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
                                                recipe_id: None,
                                                whisper_model: None,

                                                source: None,
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
            // None of the startup chrome below actually needs the daemon —
            // it only needs CONFIG. It used to live inside the bridge if-let,
            // so a down-at-launch daemon also cost the titlebar pref, the
            // startup window, every global hotkey, and the overlay (its
            // wider blast radius). Read config directly instead.
            let startup_cfg = phoneme_core::Config::read_or_default();

            if startup_cfg.interface.strip_titlebar {
                use tauri::Manager;
                if let Some(window) = app.handle().get_webview_window("main") {
                    let _ = window.set_decorations(false);
                }
            }

            if startup_cfg.tray.show_on_startup {
                use tauri::Manager;
                if let Some(window) = app.handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            // Register all enabled global hotkeys via the shared helper, so
            // startup and config-save/profile-switch stay in lockstep.
            commands::register_hotkeys(app.handle(), &startup_cfg);

            // Pre-create the system-wide live-preview overlay (hidden) when
            // the setting is on, so the first recording can reveal it with no
            // cold-start lag. No-op when the setting is off — the window is
            // only built when the user opts in. `overlay.ts` then drives its
            // visibility from the daemon event stream.
            overlay::sync(app.handle(), startup_cfg.interface.preview_overlay);

            // Pre-create the recording-indicator window (hidden) when its setting
            // is on — a separate, independent always-on-top pill (record dot +
            // waveform + timer, no captions) that `indicator.ts` shows while
            // recording. No-op when off. Independent of the caption overlay above.
            indicator::sync(app.handle(), startup_cfg.interface.recording_indicator);

            // The daemon event stream needs a live bridge. Attach now when we
            // have one; otherwise keep retrying in the background and attach
            // the moment the daemon comes up, so the UI doesn't stay
            // event-dead until an app restart.
            if let Some(b) = bridge.current() {
                events::spawn(app.handle().clone(), b);
            } else {
                let slot = bridge.clone();
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if let Some(b) = slot.get_or_connect().await {
                            events::spawn(handle, b);
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_recordings,
            commands::semantic_search,
            similar::more_like_this,
            commands::reembed_all,
            commands::get_recording,
            commands::list_ai_activity,
            commands::list_meeting,
            commands::get_segments,
            commands::get_words,
            commands::delete_recording,
            commands::delete_session,
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
            commands::reimport_from_disk,
            commands::rebuild_catalog,
            commands::refire_hook,
            commands::rerun_cleanup,
            commands::rerun_summary,
            commands::list_queue,
            commands::cancel_queued,
            commands::reorder_queue,
            commands::set_queue_paused,
            commands::queue_paused,
            commands::queue_counts,
            commands::clear_failed,
            commands::dismiss_failed,
            commands::cancel_all_queued,
            commands::cancel_processing,
            commands::run_doctor,
            commands::update_transcript,
            commands::update_meeting_name,
            commands::get_original_transcript,
            commands::get_clean_transcript,
            commands::update_notes,
            commands::set_favorite,
            commands::set_recording_title,
            commands::export_captions,
            commands::export_recording_json,
            commands::save_text_export,
            commands::export_library_zip,
            commands::restart_whisper,
            commands::save_window_state,
            commands::set_preview_source,
            commands::skip_current_stage,
            commands::suggest_tags,
            commands::approve_tag_suggestion,
            commands::clear_all_tag_suggestions,
            commands::dismiss_tag_suggestion,
            commands::set_speaker_name,
            commands::daemon_status,
            commands::read_config,
            commands::set_overlay,
            commands::write_config,
            commands::config_exists,
            commands::config_path,
            commands::list_profiles,
            commands::save_profile,
            commands::switch_profile,
            commands::delete_profile,
            commands::list_profiles_detailed,
            commands::rename_profile,
            commands::doctor_local_checks,
            commands::doctor_backend_checks,
            commands::start_daemon,
            commands::wizard_test_whisper,
            commands::list_input_devices,
            commands::list_tags,
            commands::list_all_tags,
            commands::add_tag,
            commands::update_tag,
            commands::delete_tag,
            commands::attach_tag,
            commands::detach_tag,
            commands::tags_for,
            commands::tag_usage_counts,
            commands::kind_counts,
            commands::merge_tags,
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
            commands::tail_log,
        ]);

    let builder = builder
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Fatal error while building tauri application: {e}");
            std::process::exit(1);
        });

    builder.run(move |_app, event| {
        if let tauri::RunEvent::Exit = event {
            // Last-resort daemon stop for exits that bypass the tray menu's
            // Quit chain (which already sent Shutdown and waited — see
            // `tray::stop_daemon_for_exit`). Gated on the same
            // `interface.quit_stops_daemon` knob: when it's off, the daemon
            // deliberately outlives every tray exit (headless setups). Peek
            // without connecting — there is no point dialing a daemon just to
            // tell it to shut down.
            let cfg = phoneme_core::Config::read_or_default();
            if tray::should_stop_daemon_on_exit(
                cfg.interface.quit_stops_daemon,
                tray::daemon_stop_done(),
            ) {
                if let Some(b) = exit_bridge.current() {
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
        }
    });
}
