//! Daemon recorder — owns the active recording (at most one) and ties
//! capture lifecycle to the catalog and inbox.

use crate::app_state::AppState;
use chrono::Local;
use phoneme_audio::device::resolve_input_device;
use phoneme_audio::format::SampleRate;
use phoneme_audio::preroll::PreRollBuffer;
use phoneme_audio::recorder::{Recorder, RecorderConfig, RecordingMode as AudioMode};
use phoneme_audio::source::{CpalSource, Source};
use phoneme_core::config::CaptureSource;
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

/// A running idle pre-roll pre-capture: a background task pulls canonical
/// microphone blocks into a shared ring buffer that retains the last
/// `pre_roll_ms` of audio. The task runs *between* recordings; it is stopped
/// (and its ring drained) when a recording starts, and restarted afterwards.
struct PreRoll {
    /// Shared ring buffer the idle task feeds; snapshotted on RecordStart.
    ring: Arc<Mutex<PreRollBuffer>>,
    /// Dropping/sending tells the idle task to stop pulling and exit.
    stop_tx: tokio::sync::oneshot::Sender<()>,
    /// The idle task handle — joined when stopping so the `CpalSource` is fully
    /// torn down (mic released) before we proceed.
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Default)]
pub struct DaemonRecorder {
    active: Arc<Mutex<Option<ActiveRecording>>>,
    handle: Arc<Mutex<Option<Recorder>>>,
    /// Idle pre-roll pre-capture, present only while enabled and not actively
    /// recording. `None` means no continuous capture is running (the default).
    preroll: Arc<Mutex<Option<PreRoll>>>,
}

/// Whether pre-roll should be active for the current config: opt-in
/// (`pre_roll_ms > 0`) and microphone-only (loopback/system-audio is skipped).
fn preroll_enabled(cfg: &phoneme_core::Config) -> bool {
    cfg.recording.pre_roll_ms > 0 && cfg.recording.source == CaptureSource::Microphone
}

impl DaemonRecorder {
    pub async fn current(&self) -> Option<ActiveRecording> {
        self.active.lock().await.clone()
    }

    /// Start idle pre-roll pre-capture if it's enabled for the current config
    /// and not already running. Safe to call repeatedly (idempotent) and
    /// whenever the daemon is idle (startup, after a recording finishes).
    ///
    /// When pre-roll is disabled this is a no-op, so the default path keeps the
    /// microphone closed between recordings exactly as before.
    pub async fn ensure_preroll(&self, state: &AppState) {
        let cfg = state.config.load();
        if !preroll_enabled(&cfg) {
            return;
        }
        // Don't pre-capture while a recording is in flight.
        if self.active.lock().await.is_some() {
            return;
        }
        let mut slot = self.preroll.lock().await;
        if slot.is_some() {
            return; // already running
        }

        let device = match resolve_input_device(&cfg.recording.input_device) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "pre-roll: could not resolve input device; skipping");
                return;
            }
        };
        // Pre-roll is mic-only; open the microphone explicitly.
        let source = match CpalSource::open_kind(device, CaptureSource::Microphone) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "pre-roll: could not open microphone; skipping");
                return;
            }
        };

        let ring = Arc::new(Mutex::new(PreRollBuffer::with_duration_ms(
            cfg.recording.pre_roll_ms,
            SampleRate::HZ_16K,
        )));
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let ring_task = ring.clone();
        let task = tokio::spawn(async move {
            let mut source: Box<dyn Source> = Box::new(source);
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    block = source.next_block() => {
                        match block {
                            Ok(Some(b)) => ring_task.lock().await.push(&b),
                            // Source drained/closed unexpectedly — stop idling.
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(error = %e, "pre-roll: capture error; stopping idle pre-capture");
                                break;
                            }
                        }
                    }
                }
            }
            // Release the microphone cleanly.
            let _ = source.stop().await;
        });

        *slot = Some(PreRoll {
            ring,
            stop_tx,
            task,
        });
        tracing::info!(
            pre_roll_ms = cfg.recording.pre_roll_ms,
            "pre-roll idle pre-capture started"
        );
    }

    /// Reconcile the running idle pre-capture against the current config: start
    /// it if pre-roll is (now) enabled, stop it if it was disabled or the source
    /// switched away from the microphone. Call after a config reload and at
    /// daemon startup. No-op while a recording is active.
    pub async fn sync_preroll(&self, state: &AppState) {
        if self.active.lock().await.is_some() {
            return;
        }
        let enabled = preroll_enabled(&state.config.load());
        let running = self.preroll.lock().await.is_some();
        match (enabled, running) {
            (true, false) => self.ensure_preroll(state).await,
            (false, true) => {
                // Drop the buffered audio — nothing is persisted.
                let _ = self.take_preroll_samples().await;
                tracing::info!("pre-roll disabled; idle pre-capture stopped");
            }
            _ => {}
        }
    }

    /// Stop idle pre-capture (if running), join its task so the microphone is
    /// released, and return the buffered samples (oldest → newest). Returns an
    /// empty Vec when no pre-capture was running.
    async fn take_preroll_samples(&self) -> Vec<i16> {
        let Some(pr) = self.preroll.lock().await.take() else {
            return Vec::new();
        };
        let PreRoll {
            ring,
            stop_tx,
            task,
        } = pr;
        let _ = stop_tx.send(());
        // Wait for the idle task to exit so the CpalSource (and the mic) is
        // fully torn down before the recording opens its own source.
        let _ = task.await;
        let samples = ring.lock().await.to_vec();
        if !samples.is_empty() {
            tracing::info!(
                samples = samples.len(),
                "pre-roll: prepending buffered audio"
            );
        }
        samples
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
            notes: None,
        };
        state.catalog.insert(&row).await?;

        // If idle pre-roll pre-capture is running, stop it and grab the buffered
        // audio to prepend; this also releases the microphone before we reopen
        // it for the recording. Empty when pre-roll is disabled (default path).
        let prepend = self.take_preroll_samples().await;

        // Open the CPAL device and the audio Recorder.
        let app_cfg = state.config.load();
        let device = resolve_input_device(&app_cfg.recording.input_device)?;
        let source = CpalSource::open_kind(device, app_cfg.recording.source)?;
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
        let recorder =
            Recorder::start_with_prepend(Box::new(source), recorder_cfg, Some(tx), prepend).await?;
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

        // Release the active lock before resuming idle pre-capture
        // (`ensure_preroll` re-acquires it). No-op when pre-roll is disabled.
        let id = active.id;
        drop(active_lock);
        self.ensure_preroll(state).await;
        Ok(id)
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

        // Resume idle pre-capture after releasing the active lock. No-op when
        // pre-roll is disabled.
        let id = active.id;
        drop(active_lock);
        self.ensure_preroll(state).await;
        Ok(id)
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
