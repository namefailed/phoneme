//! Daemon recorder — owns the active recording (at most one) and ties
//! capture lifecycle to the catalog and inbox.

use crate::app_state::AppState;
use chrono::Local;
use phoneme_audio::device::resolve_input_device;
use phoneme_audio::recorder::{Recorder, RecorderConfig, RecordingMode as AudioMode};
use phoneme_audio::source::CpalSource;
use phoneme_core::error::{Error, Result};
use phoneme_core::{HookMetadata, HookPayload, Recording, RecordingId, RecordingStatus};
use phoneme_ipc::DaemonEvent;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug)]
pub enum RecordMode {
    Hold,
    Oneshot,
    Duration { secs: u32 },
}

impl From<phoneme_core::RecordMode> for RecordMode {
    fn from(m: phoneme_core::RecordMode) -> Self {
        match m {
            phoneme_core::RecordMode::Hold => RecordMode::Hold,
            phoneme_core::RecordMode::Oneshot => RecordMode::Oneshot,
            phoneme_core::RecordMode::Duration { secs } => RecordMode::Duration { secs },
        }
    }
}

// `mode` and `audio_path` aren't read directly off the snapshot yet, but the
// daemon recorder threads them through start/stop/cancel flows and they'll be
// consumed by the doctor / debug endpoints in later plans.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ActiveRecording {
    pub id: RecordingId,
    pub mode: RecordMode,
    pub audio_path: PathBuf,
    pub started_at: chrono::DateTime<Local>,
    pub paused: bool,
}

#[derive(Clone, Default)]
pub struct DaemonRecorder {
    active: Arc<Mutex<Option<ActiveRecording>>>,
    handle: Arc<Mutex<Option<Recorder>>>,
}

impl DaemonRecorder {
    pub async fn current(&self) -> Option<ActiveRecording> {
        self.active.lock().await.clone()
    }

    /// Start a recording. Returns `AlreadyRecording` if one is in flight.
    pub async fn start(&self, state: &AppState, mode: RecordMode) -> Result<RecordingId> {
        let mut active = self.active.lock().await;
        if let Some(a) = active.as_ref() {
            return Err(Error::AlreadyRecording {
                current: a.id.to_string(),
            });
        }
        let id = RecordingId::new();
        let started_at = Local::now();
        let audio_path = state
            .paths
            .audio_dir
            .join(id.day_folder())
            .join(format!("{}.wav", id.file_stem()));

        // Insert the catalog row at status=recording.
        let row = Recording {
            id: id.clone(),
            started_at,
            duration_ms: 0,
            audio_path: audio_path.to_string_lossy().into_owned(),
            transcript: None,
            model: None,
            status: RecordingStatus::Recording,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
        };
        state.catalog.insert(&row).await?;

        // Open the CPAL device and the audio Recorder.
        let device = resolve_input_device(&state.config.load().recording.input_device)?;
        let source = CpalSource::open(device)?;
        let audio_mode = match mode {
            RecordMode::Hold => AudioMode::Hold,
            RecordMode::Oneshot => AudioMode::Oneshot,
            RecordMode::Duration { secs } => AudioMode::Duration { secs },
        };
        let recorder_cfg = RecorderConfig {
            mode: audio_mode,
            max_duration_ms: state.config.load().recording.max_duration_secs as u64 * 1000,
            silence_threshold_dbfs: state.config.load().recording.silence_threshold_dbfs,
            silence_window_ms: state.config.load().recording.silence_window_ms,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let recorder = Recorder::start(Box::new(source), recorder_cfg, Some(tx)).await?;
        *self.handle.lock().await = Some(recorder);

        *active = Some(ActiveRecording {
            id: id.clone(),
            mode,
            audio_path,
            started_at,
            paused: false,
        });

        // If it's a self-terminating mode, spawn a task to auto-stop when the recorder task finishes natively.
        if !matches!(mode, RecordMode::Hold) {
            let daemon_recorder = self.clone();
            let state_clone = state.clone();
            tokio::spawn(async move {
                if rx.await.is_ok() {
                    let _ = daemon_recorder.stop(&state_clone).await;
                }
            });
        }

        state.events.emit(DaemonEvent::RecordingStarted {
            id: id.clone(),
            started_at,
        });
        tracing::info!(id = %id, mode = ?mode, "recording started");
        Ok(id)
    }

    /// Stop the current recording, write WAV, enqueue inbox, mark catalog
    /// row as transcribing.
    pub async fn stop(&self, state: &AppState) -> Result<RecordingId> {
        let mut active_lock = self.active.lock().await;
        let active = active_lock.take().ok_or(Error::NotRecording)?;
        let recorder = self.handle.lock().await.take().ok_or(Error::NotRecording)?;
        let result = recorder.stop_and_finalize(&active.audio_path).await?;

        state
            .catalog
            .update_status(&active.id, RecordingStatus::Transcribing)
            .await?;
        state
            .catalog
            .update_duration(&active.id, result.duration_ms)
            .await?;

        let payload = HookPayload {
            id: active.id.clone(),
            timestamp: active.started_at,
            transcript: String::new(),
            audio_path: active.audio_path.to_string_lossy().into_owned(),
            duration_ms: result.duration_ms,
            model: String::new(),
            metadata: HookMetadata::current(),
        };
        state.inbox.enqueue(&payload).await?;

        state.events.emit(DaemonEvent::RecordingStopped {
            id: active.id.clone(),
            duration_ms: result.duration_ms,
            audio_path: active.audio_path.to_string_lossy().into_owned(),
        });
        tracing::info!(id = %active.id, ms = result.duration_ms, "recording stopped");
        Ok(active.id)
    }

    /// Cancel the current recording: discard samples, delete catalog row, no
    /// WAV, no inbox.
    pub async fn cancel(&self, state: &AppState) -> Result<RecordingId> {
        let mut active_lock = self.active.lock().await;
        let active = active_lock.take().ok_or(Error::NotRecording)?;
        if let Some(recorder) = self.handle.lock().await.take() {
            let _ = recorder.cancel().await;
        }
        state.catalog.delete(&active.id).await?;
        state.events.emit(DaemonEvent::RecordingCancelled {
            id: active.id.clone(),
        });
        tracing::info!(id = %active.id, "recording cancelled");
        Ok(active.id)
    }

    /// Pause the active recording.
    pub async fn pause(&self, state: &AppState) -> Result<RecordingId> {
        let mut active_lock = self.active.lock().await;
        let active = active_lock.as_mut().ok_or(Error::NotRecording)?;
        if active.paused {
            return Ok(active.id.clone());
        }

        if let Some(recorder) = self.handle.lock().await.as_ref() {
            recorder
                .pause()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }

        active.paused = true;
        state
            .catalog
            .update_status(&active.id, RecordingStatus::Paused)
            .await?;

        state.events.emit(DaemonEvent::RecordingPaused {
            id: active.id.clone(),
        });
        tracing::info!(id = %active.id, "recording paused");
        Ok(active.id.clone())
    }

    /// Resume the active recording.
    pub async fn resume(&self, state: &AppState) -> Result<RecordingId> {
        let mut active_lock = self.active.lock().await;
        let active = active_lock.as_mut().ok_or(Error::NotRecording)?;
        if !active.paused {
            return Ok(active.id.clone());
        }

        if let Some(recorder) = self.handle.lock().await.as_ref() {
            recorder
                .resume()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }

        active.paused = false;
        state
            .catalog
            .update_status(&active.id, RecordingStatus::Recording)
            .await?;

        state.events.emit(DaemonEvent::RecordingResumed {
            id: active.id.clone(),
        });
        tracing::info!(id = %active.id, "recording resumed");
        Ok(active.id.clone())
    }
}
