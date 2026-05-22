//! Bridge daemon events to the frontend via Tauri's emit().

use crate::bridge::Bridge;
use crate::tray::{self, TrayState};
use futures::StreamExt;
use phoneme_ipc::{DaemonEvent, NamedPipeTransport, Transport};
use tauri::{AppHandle, Emitter};

pub fn spawn(app: AppHandle, bridge: Bridge) {
    tauri::async_runtime::spawn(async move {
        loop {
            match run_once(app.clone(), bridge.clone()).await {
                Ok(()) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "event stream ended; reconnecting in 2s");
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
                        let _ = tray::update_state(&tray, state);
                    }
                }
                // Re-emit to the frontend.
                let _ = app.emit("daemon-event", &event);
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
        DaemonEvent::TranscriptionFailed { .. } => Some(TrayState::LlmError),
        DaemonEvent::HookFailed { .. } => Some(TrayState::HookFailed),
        DaemonEvent::QueueDepthChanged { pending, .. } if *pending > 0 => {
            Some(TrayState::CatchupBacklog(*pending as u32))
        }
        DaemonEvent::LlmStatusChanged { reachable: false } => Some(TrayState::LlmError),
        DaemonEvent::LlmStatusChanged { reachable: true } => Some(TrayState::Idle),
        _ => None,
    }
}
