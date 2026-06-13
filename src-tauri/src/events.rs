//! Bridge daemon events to the frontend — the tray's read side of the event
//! stream.
//!
//! `spawn` runs a forever-loop: open a DEDICATED pipe connection (the
//! subscription consumes its connection per the IPC protocol, so the
//! request/response bridge is never used for it), `SubscribeEvents`, and for
//! every received `DaemonEvent` (1) derive a new [`TrayState`] where the
//! event implies one — recording, transcribing, backlog count, whisper
//! error, hook failure, back to idle — and update the tray icon, and (2)
//! re-emit the event verbatim as the Tauri `daemon-event`, which broadcasts
//! to ALL webviews: the main window's stores refresh from it and the
//! overlay drives its show/hide. When the stream ends (daemon restart, lag
//! disconnect), it reconnects on a 2 s loop — by re-subscribing it also
//! satisfies the "re-fetch after lag" contract, since the frontend re-syncs
//! on reconnect.

use crate::bridge::Bridge;
use crate::tray::{self, TrayState};
use futures::StreamExt;
use phoneme_ipc::{DaemonEvent, NamedPipeTransport, Transport};
use tauri::{AppHandle, Emitter};

pub fn spawn(app: AppHandle, bridge: Bridge) {
    tauri::async_runtime::spawn(async move {
        loop {
            match run_once(app.clone(), bridge.clone()).await {
                Ok(()) | Err(_) => {
                    tracing::warn!("event stream ended; reconnecting in 2s");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    });
}

async fn run_once(app: AppHandle, bridge: Bridge) -> anyhow::Result<()> {
    // Open a SEPARATE pipe connection for the subscription (per spec: the
    // pipe gets dedicated to event streaming after SubscribeEvents).
    let mut sub_transport = NamedPipeTransport::connect(&bridge.config.daemon.pipe_name).await?;
    let mut stream = sub_transport.subscribe().await?;

    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => {
                // Update tray state for relevant events.
                if let Some(state) = derive_tray_state(&event) {
                    if let Some(tray) = app.tray_by_id("main") {
                        if let Err(e) = tray::update_state(&tray, state) {
                            tracing::warn!("failed to update tray state: {e}");
                        }
                    }
                }
                // Re-emit to the frontend.
                if let Err(e) = app.emit("daemon-event", &event) {
                    tracing::warn!("failed to emit daemon-event to frontend: {e}");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "subscribe stream error");
                break;
            }
        }
    }
    Ok(())
}

fn derive_tray_state(event: &DaemonEvent) -> Option<TrayState> {
    match event {
        DaemonEvent::RecordingStarted { .. } => Some(TrayState::Recording),
        DaemonEvent::RecordingStopped { .. } => Some(TrayState::Transcribing),
        DaemonEvent::TranscriptionDone { .. } | DaemonEvent::HookDone { .. } => {
            Some(TrayState::Idle)
        }
        DaemonEvent::TranscriptionFailed { .. } => Some(TrayState::WhisperError),
        DaemonEvent::HookFailed { .. } => Some(TrayState::HookFailed),
        DaemonEvent::QueueDepthChanged { pending, .. } if *pending > 0 => {
            Some(TrayState::CatchupBacklog(*pending as u32))
        }
        DaemonEvent::WhisperStatusChanged { reachable: false } => Some(TrayState::WhisperError),
        DaemonEvent::WhisperStatusChanged { reachable: true } => Some(TrayState::Idle),
        DaemonEvent::RecordingCancelled { .. } => Some(TrayState::Idle),
        _ => None,
    }
}
