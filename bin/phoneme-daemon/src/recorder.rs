//! Daemon recorder — owns the active recording (at most one) and ties
//! capture lifecycle to the catalog and inbox.

use crate::app_state::AppState;
use chrono::Local;
use phoneme_audio::device::resolve_input_device;
use phoneme_audio::format::SampleRate;
use phoneme_audio::meeting_align::{align_meeting_tracks, TrackAlignInput};
use phoneme_audio::preroll::PreRollBuffer;
use phoneme_audio::recorder::{Recorder, RecorderConfig};
use phoneme_audio::source::{CpalSource, GeneratorSource, Source};
use phoneme_audio::wav;
use phoneme_core::config::CaptureSource;
use phoneme_core::error::{Error, Result};
use phoneme_core::{
    HookMetadata, HookPayload, MeetingTrack, RecordMode, Recording, RecordingId, RecordingStatus,
};
use phoneme_ipc::DaemonEvent;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// How often the streaming-preview loop transcribes the in-progress recording.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(2000);

/// How long to keep a recording's capture stream alive after a stop is
/// requested, so the OS can hand over audio it had already buffered at stop
/// time instead of discarding it. Without this, manually-stopped recordings can
/// lose the final ~tens-of-milliseconds and sound clipped at the end. Applied
/// only to real recording/meeting sources — not the rolling pre-roll buffer.
const STOP_TAIL_GRACE: Duration = Duration::from_millis(150);

/// Minimum number of *new* samples (beyond the previous preview) before we spend
/// a transcription on a fresh tick. At 16 kHz this is ~1.0 s — below that a
/// re-transcription rarely changes the text enough to be worth the round trip.
const PREVIEW_MIN_NEW_SAMPLES: usize = 16_000;

/// The streaming preview transcribes only the last `PREVIEW_WINDOW_SAMPLES` of
/// captured audio each tick — a rolling "live caption" — so per-tick work stays
/// roughly constant instead of growing with the recording length. Without this
/// the loop re-transcribed the entire (ever-growing) buffer every tick, which is
/// O(n²) over a recording and saturates the CPU / whisper-server on long takes.
/// The authoritative full transcript is still produced from the complete file
/// after the recording stops (see the pipeline), so this only bounds the live
/// preview, not the final result. 15 s at 16 kHz.
const PREVIEW_WINDOW_SAMPLES: usize = 16_000 * 15;

/// Open a [`Source`] for the current recording: returns a real CPAL source in
/// production and a [`GeneratorSource`] when `PHONEME_AUDIO_BACKEND=synthetic`
/// is set (CI / headless tests).
fn make_source(open_cpal: impl FnOnce() -> Result<CpalSource>) -> Result<Box<dyn Source>> {
    if std::env::var("PHONEME_AUDIO_BACKEND").as_deref() == Ok("synthetic") {
        // 1 600 frames = 100 ms blocks at 16 kHz — enough resolution to respond
        // to stop() promptly without the test-harness timing being too tight.
        return Ok(Box::new(GeneratorSource::new(1_600)));
    }
    Ok(Box::new(open_cpal()?))
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
    pub in_place: bool,
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
    /// torn down (mic released) before we proceed, or returned for reuse.
    task: tokio::task::JoinHandle<Option<Box<dyn Source>>>,
}

/// A running streaming-preview loop: periodically transcribes the in-progress
/// recording and emits `TranscriptionPartial` events. Present only while a
/// recording is active *and* `recording.streaming_preview` is enabled.
struct PreviewTask {
    /// Sending (or dropping) tells the loop to stop and exit.
    stop_tx: tokio::sync::oneshot::Sender<()>,
    /// The loop's join handle — awaited on stop so it tears down cleanly.
    task: tokio::task::JoinHandle<()>,
}

/// One track of an in-flight meeting: its catalog id, where the WAV will be
/// written, when it started, the track label, the live recorder handle, and
/// when capture actually began (for timeline alignment).
struct MeetingTrackHandle {
    id: RecordingId,
    audio_path: PathBuf,
    started_at: chrono::DateTime<Local>,
    track: MeetingTrack,
    recorder: Recorder,
    capture_started: Instant,
}

/// An in-flight meeting: the two concurrently-recording tracks (mic + system).
/// Both share `meeting_id`; stopping the meeting finalizes both together.
struct ActiveMeeting {
    meeting_id: String,
    tracks: Vec<MeetingTrackHandle>,
    paused: bool,
    /// Wall-clock instant when the meeting session began (before per-track setup).
    wall_started: Instant,
}

#[derive(Clone, Default)]
pub struct DaemonRecorder {
    active: Arc<Mutex<Option<ActiveRecording>>>,
    handle: Arc<Mutex<Option<Recorder>>>,
    /// Idle pre-roll pre-capture, present only while enabled and not actively
    /// recording. `None` means no continuous capture is running (the default).
    preroll: Arc<Mutex<Option<PreRoll>>>,
    /// Streaming transcription preview loop, present only while recording with
    /// the feature enabled. `None` (the default) means no preview is running.
    preview: Arc<Mutex<Option<PreviewTask>>>,
    /// In-flight meeting (Meeting Mode, v1.6). `None` (the default) means no
    /// meeting is recording. Held separately from `active` so the existing
    /// single-recording path is completely untouched.
    meeting: Arc<Mutex<Option<ActiveMeeting>>>,
    /// Serializes `MeetingToggle`. Without it the toggle is check-then-act
    /// across two separate `meeting` lock acquisitions (read state, then
    /// start/stop), so two near-simultaneous hotkey presses can both observe
    /// "no meeting" and both call `start_meeting` (one then fails), or both
    /// observe "meeting" and race to stop. Held for the whole toggle so the
    /// decision and the action are atomic with respect to other toggles.
    toggle_guard: Arc<Mutex<()>>,
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

    pub async fn is_paused(&self) -> bool {
        if let Some(ref active) = *self.active.lock().await {
            active.paused
        } else if let Some(ref meeting) = *self.meeting.lock().await {
            meeting.paused
        } else {
            false
        }
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
        // Pre-roll is mic-only; open the microphone explicitly. Use STOP_TAIL_GRACE
        // so if we reuse this source for a recording, it doesn't clip when stopped.
        let source = match CpalSource::open_kind_with_grace(
            device,
            CaptureSource::Microphone,
            STOP_TAIL_GRACE,
        ) {
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
            // Return the source instead of dropping it, so it can be reused
            // to completely eliminate the initialization gap.
            Some(source)
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
    /// released (or returned), and return the buffered samples (oldest → newest). Returns an
    /// empty Vec and None when no pre-capture was running.
    async fn take_preroll_samples(&self) -> (Vec<i16>, Option<Box<dyn Source>>) {
        let Some(pr) = self.preroll.lock().await.take() else {
            return (Vec::new(), None);
        };
        let PreRoll {
            ring,
            stop_tx,
            task,
        } = pr;
        let _ = stop_tx.send(());
        // Wait for the idle task to exit and return the source it was using.
        // This fully tears down the idle loop before the recording opens its own source,
        // or allows us to reuse the already-running stream to avoid initialization gaps.
        let source = task.await.unwrap_or(None);
        let samples = ring.lock().await.to_vec();
        if !samples.is_empty() {
            tracing::info!(
                samples = samples.len(),
                "pre-roll: prepending buffered audio"
            );
        }
        (samples, source)
    }

    /// Spawn the streaming-preview loop for `id`, if `recording.streaming_preview`
    /// is enabled. No-op (and no task) when the flag is off — that default path
    /// is byte-for-byte the historical behavior. The loop snapshots the live
    /// recorder every `PREVIEW_INTERVAL`, transcribes the audio so far via the
    /// configured provider, and emits `TranscriptionPartial`. It transcribes one
    /// tick at a time (a slow transcription simply means the next tick is
    /// skipped — never two in flight), and stops when told to via `stop_tx`.
    async fn start_preview(&self, state: &AppState, id: RecordingId) {
        if !state.config.load().recording.streaming_preview {
            return;
        }
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let state = state.clone();
        let handle = self.handle.clone();
        let log_id = id.clone();
        let task = tokio::spawn(async move {
            let cfg = state.config.load();
            // The live preview uses its own provider when configured
            // (`preview_whisper`) — a fast local model on a second server, or a
            // cloud API — so it never contends with the final transcription.
            // Falls back to the main provider when unset (unchanged behavior).
            let provider = state.transcription.provider(
                cfg.preview_provider_config(),
                &phoneme_core::config::DiarizationConfig::default(),
            );
            let is_native = provider.is_native();

            // If the provider is native (running directly in our RAM), we can safely
            // drop the interval to 1000ms for real-time streaming without worrying
            // about HTTP/file-write overhead. Cloud providers get longer intervals
            // to avoid overwhelming the API.
            let interval_duration = if is_native {
                std::time::Duration::from_millis(1000)
            } else {
                PREVIEW_INTERVAL
            };

            let tmp_wav =
                std::env::temp_dir().join(format!("phoneme-preview-{}.wav", id.file_stem()));
            let mut interval = tokio::time::interval(interval_duration);
            // If a transcription overruns the interval, skip missed ticks rather
            // than firing a burst — this is the "never two at once" throttle.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Burn the immediate first tick so we don't transcribe near-empty audio.
            interval.tick().await;
            let mut last_len = 0usize;
            let audio_cfg = phoneme_audio::format::AudioConfig::phoneme_default();

            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = interval.tick() => {}
                }

                // Snapshot only the trailing window of audio captured so far (the
                // recorder also tells us the full captured length so we can still
                // throttle on newly-accumulated audio). If the recorder is gone
                // (race with stop), end the loop.
                let (total_len, samples) = {
                    let guard = handle.lock().await;
                    match guard.as_ref() {
                        Some(rec) => match rec.snapshot_tail(PREVIEW_WINDOW_SAMPLES).await {
                            Ok(s) => s,
                            Err(_) => break,
                        },
                        None => break,
                    }
                };
                // Skip until enough *new* audio has accumulated to be worth a tick.
                if total_len < last_len + PREVIEW_MIN_NEW_SAMPLES {
                    continue;
                }
                last_len = total_len;

                // Yield to final transcriptions: only run this preview tick if
                // the whisper-server permit is free *right now*. If a final
                // transcription holds it, skip — the preview must never pile onto
                // the single serial server and starve the real transcription
                // (which previously caused "Whisper timed out after 60s"). The
                // permit is held for the duration of this tick's transcription.
                let _preview_permit = match state.whisper_sem.try_acquire() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Write a temp WAV and transcribe via the configured provider.
                if let Err(e) = wav::write_wav(&tmp_wav, &samples, audio_cfg) {
                    tracing::warn!(error = %e, "streaming preview: failed to write temp WAV; skipping tick");
                    continue;
                }
                let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());

                // Use the cached provider to avoid re-resolving on every tick.
                // Config changes during recording will take effect on the next recording.
                match provider.transcribe(&tmp_wav, language.as_deref()).await {
                    Ok(text) => {
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            state.events.emit(DaemonEvent::TranscriptionPartial {
                                id: id.clone(),
                                text,
                            });
                        }
                    }
                    Err(e) => {
                        // Preview is best-effort: a failed tick is logged at debug
                        // and never surfaces to the user (the final pipeline owns
                        // authoritative success/failure reporting).
                        tracing::debug!(error = %e, "streaming preview: transcription tick failed");
                    }
                }
            }

            // Clean up temp file even if loop exits early
            let _ = tokio::fs::remove_file(&tmp_wav).await;
        });

        *self.preview.lock().await = Some(PreviewTask { stop_tx, task });
        tracing::info!(id = %log_id, "streaming transcription preview started");
    }

    /// Stop the streaming-preview loop (if running) and wait for it to exit so
    /// its temp WAV is cleaned up. No-op when no preview is running.
    async fn stop_preview(&self) {
        let Some(PreviewTask { stop_tx, task }) = self.preview.lock().await.take() else {
            return;
        };
        let _ = stop_tx.send(());
        let _ = task.await;
    }

    /// Start a recording. Returns `AlreadyRecording` if one is in flight.
    pub async fn start(
        &self,
        state: &AppState,
        mode: RecordMode,
        in_place: bool,
    ) -> Result<RecordingId> {
        // A meeting owns both the microphone and the system-audio device. Refuse
        // to start a single-track recording while one is running — otherwise we
        // would open a second microphone stream concurrent with the meeting's
        // mic track and finalize two overlapping recordings. Checked before
        // acquiring `active` (mirrors `start_meeting`'s ordering) so we never
        // hold both locks at once.
        if self.meeting.lock().await.is_some() {
            return Err(Error::AlreadyRecording {
                current: "meeting in progress".into(),
            });
        }
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

        // Reserve the active slot immediately so concurrent starts fail.
        *active = Some(ActiveRecording {
            id: id.clone(),
            mode,
            audio_path: audio_path.clone(),
            started_at,
            paused: false,
            in_place,
        });
        drop(active);

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
            // A normal single-track recording is not part of a meeting.
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place,
            cleanup_model: None,
            diarized: false,
            summary: None,
            summary_model: None,
            tags: vec![],
        };
        if let Err(e) = state.catalog.insert(&row).await {
            *self.active.lock().await = None;
            return Err(e);
        }

        // If idle pre-roll pre-capture is running, stop it and grab the buffered
        // audio to prepend; this also releases the microphone (or returns it) before we reopen
        // it for the recording. Empty when pre-roll is disabled (default path).
        let (prepend, preroll_source) = self.take_preroll_samples().await;
        let app_cfg = state.config.load();
        let kind = app_cfg.recording.source;
        let source = if let Some(s) = preroll_source {
            s
        } else {
            match make_source(|| {
                CpalSource::open_kind_with_grace(
                    resolve_input_device(&app_cfg.recording.input_device)?,
                    kind,
                    STOP_TAIL_GRACE,
                )
            }) {
                Ok(s) => s,
                Err(e) => {
                    *self.active.lock().await = None;
                    if let Err(err) = state.catalog.delete(&id).await {
                        tracing::warn!("failed to rollback catalog row: {err}");
                    }
                    return Err(e);
                }
            }
        };
        let recorder_cfg = RecorderConfig {
            // `RecorderConfig::mode` is `phoneme_core::RecordMode` (re-exported
            // by phoneme-audio as `RecordingMode`), so no conversion is needed.
            mode,
            max_duration_ms: state.config.load().recording.max_duration_secs as u64 * 1000,
            silence_threshold_dbfs: state.config.load().recording.silence_threshold_dbfs,
            silence_window_ms: state.config.load().recording.silence_window_ms,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let recorder =
            match Recorder::start_with_prepend(source, recorder_cfg, Some(tx), prepend).await {
                Ok(r) => r,
                Err(e) => {
                    *self.active.lock().await = None;
                    if let Err(err) = state.catalog.delete(&id).await {
                        tracing::warn!("failed to rollback catalog row: {err}");
                    }
                    return Err(e);
                }
            };
        *self.handle.lock().await = Some(recorder);

        // If it's a self-terminating mode, spawn a task to auto-stop when the recorder task finishes natively.
        if !matches!(mode, RecordMode::Hold) {
            let daemon_recorder = self.clone();
            let state_clone = state.clone();
            tokio::spawn(async move {
                if rx.await.is_ok() {
                    if let Err(e) = daemon_recorder.stop(&state_clone).await {
                        tracing::warn!("auto-stop failed: {e}");
                    }
                }
            });
        }

        // Spawn the live streaming-preview loop. No-op unless
        // `recording.streaming_preview` is enabled (default: off).
        self.start_preview(state, id.clone()).await;

        state.events.emit(DaemonEvent::RecordingStarted {
            id: id.clone(),
            started_at,
            meeting_id: None,
        });
        tracing::info!(id = %id, mode = ?mode, "recording started");
        Ok(id)
    }

    /// Stop the current recording, write WAV, enqueue inbox, mark catalog
    /// row as transcribing.
    pub async fn stop(&self, state: &AppState) -> Result<RecordingId> {
        let mut active_lock = self.active.lock().await;
        let active = active_lock.take().ok_or(Error::NotRecording)?;
        // Stop the streaming-preview loop first so it isn't mid-snapshot when we
        // take the recorder handle. No-op when no preview is running.
        self.stop_preview().await;
        let recorder = self.handle.lock().await.take().ok_or(Error::NotRecording)?;
        let result = recorder.stop_and_finalize(&active.audio_path).await?;

        state
            .catalog
            .update_status_and_duration(
                &active.id,
                RecordingStatus::Transcribing,
                result.duration_ms,
            )
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
        crate::queue_worker::emit_queue_depth(state).await;

        state.events.emit(DaemonEvent::RecordingStopped {
            id: active.id.clone(),
            duration_ms: result.duration_ms,
            audio_path: active.audio_path.to_string_lossy().into_owned(),
            meeting_id: None,
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
        if active_lock.is_none() {
            let mut meeting_lock = self.meeting.lock().await;
            if let Some(meeting) = meeting_lock.take() {
                let id = meeting.tracks[0].id.clone();
                for track_handle in meeting.tracks {
                    if let Err(e) = track_handle.recorder.cancel().await {
                        tracing::warn!(id = %track_handle.id, "failed to cancel meeting track: {e}");
                    }
                    if let Err(e) = state.catalog.delete(&track_handle.id).await {
                        tracing::warn!(id = %track_handle.id, "failed to delete meeting track from catalog: {e}");
                    }
                    state.events.emit(DaemonEvent::RecordingCancelled {
                        id: track_handle.id.clone(),
                    });
                }
                tracing::info!(session = %meeting.meeting_id, "meeting cancelled");

                drop(meeting_lock);
                drop(active_lock);
                self.ensure_preroll(state).await;
                return Ok(id);
            }
            return Err(Error::NotRecording);
        }
        let active = active_lock.take().unwrap();
        // Stop the preview loop before tearing down the recorder. No-op when off.
        self.stop_preview().await;
        if let Some(recorder) = self.handle.lock().await.take() {
            if let Err(e) = recorder.cancel().await {
                tracing::warn!("failed to cancel recorder: {e}");
            }
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
        if active_lock.is_none() {
            let mut meeting_lock = self.meeting.lock().await;
            if let Some(ref mut meeting) = *meeting_lock {
                if meeting.paused {
                    return Ok(meeting.tracks[0].id.clone());
                }
                for track_handle in &meeting.tracks {
                    track_handle
                        .recorder
                        .pause()
                        .await
                        .map_err(|e| Error::Internal(e.to_string()))?;
                    state
                        .catalog
                        .update_status(&track_handle.id, RecordingStatus::Paused)
                        .await?;
                    state.events.emit(DaemonEvent::RecordingPaused {
                        id: track_handle.id.clone(),
                    });
                }
                meeting.paused = true;
                tracing::info!(session = %meeting.meeting_id, "meeting paused");
                return Ok(meeting.tracks[0].id.clone());
            }
            return Err(Error::NotRecording);
        }
        let active = active_lock.as_mut().unwrap();
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
        if active_lock.is_none() {
            let mut meeting_lock = self.meeting.lock().await;
            if let Some(ref mut meeting) = *meeting_lock {
                if !meeting.paused {
                    return Ok(meeting.tracks[0].id.clone());
                }
                for track_handle in &meeting.tracks {
                    track_handle
                        .recorder
                        .resume()
                        .await
                        .map_err(|e| Error::Internal(e.to_string()))?;
                    state
                        .catalog
                        .update_status(&track_handle.id, RecordingStatus::Recording)
                        .await?;
                    state.events.emit(DaemonEvent::RecordingResumed {
                        id: track_handle.id.clone(),
                    });
                }
                meeting.paused = false;
                tracing::info!(session = %meeting.meeting_id, "meeting resumed");
                return Ok(meeting.tracks[0].id.clone());
            }
            return Err(Error::NotRecording);
        }
        let active = active_lock.as_mut().unwrap();
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

    /// Is a meeting currently recording?
    pub async fn meeting_active(&self) -> bool {
        self.meeting.lock().await.is_some()
    }

    /// Atomically toggle Meeting Mode: stop the meeting if one is running,
    /// otherwise start one. Returns `Ok(true)` if a meeting was started and
    /// `Ok(false)` if one was stopped.
    ///
    /// The `toggle_guard` is held for the entire decision-and-action so two
    /// concurrent toggles (e.g. a double-tapped hotkey, or hotkey + UI button)
    /// can't both read the same state and act on it — the second waits, re-reads
    /// the now-updated state, and does the opposite. `start_meeting`/
    /// `stop_meeting` keep their own internal guards, so this composes safely
    /// with the explicit `StartMeeting`/`StopMeeting` requests too.
    pub async fn toggle_meeting(&self, state: &AppState) -> Result<bool> {
        let _guard = self.toggle_guard.lock().await;
        if self.meeting_active().await {
            self.stop_meeting(state).await?;
            Ok(false)
        } else {
            self.start_meeting(state).await?;
            Ok(true)
        }
    }

    /// Start Meeting Mode (v1.6): record the microphone AND the system audio
    /// (WASAPI loopback) concurrently as two separate, linked recordings.
    ///
    /// Opens a mic `CpalSource` and a system-audio (loopback) `CpalSource`,
    /// then delegates to [`Self::start_meeting_with_sources`], which owns the
    /// catalog/inbox orchestration. Tests drive that helper directly with
    /// `SyntheticSource`s; this method is the production entry point that wires
    /// in the real hardware sources.
    ///
    /// Returns the freshly-minted `meeting_id` shared by both tracks.
    pub async fn start_meeting(&self, state: &AppState) -> Result<String> {
        // Refuse to start a meeting while a normal recording is in flight, and
        // refuse to start a second meeting. This keeps the single-recording
        // path's invariants intact (it never has to reason about a meeting).
        if self.active.lock().await.is_some() {
            return Err(Error::AlreadyRecording {
                current: "single recording in progress".into(),
            });
        }
        if self.meeting.lock().await.is_some() {
            return Err(Error::AlreadyRecording {
                current: "meeting already in progress".into(),
            });
        }

        // Stop idle pre-roll pre-capture so the microphone is free for the
        // meeting's own mic source. The buffered audio is discarded — meeting
        // tracks don't use pre-roll. No-op when pre-roll is disabled.
        let (_, preroll_source) = self.take_preroll_samples().await;
        // Since meeting needs two different sources (mic and system loopback)
        // and we cannot safely assume the preroll source matches the mic one
        // without more work, we just drop it to release the microphone cleanly.
        if let Some(mut s) = preroll_source {
            let _ = s.stop().await;
        }

        let cfg = state.config.load();
        let device = resolve_input_device(&cfg.recording.input_device)?;

        // Open both capture sources up front. If either fails we abort before
        // mutating any state, so a failed meeting leaves the daemon idle.
        // `open_kind(.., SystemAudio)` ignores the passed device and opens the
        // default output device in WASAPI loopback mode.
        let mic_source =
            CpalSource::open_kind_with_grace(device, CaptureSource::Microphone, STOP_TAIL_GRACE)
                .map_err(|e| Error::Internal(format!("meeting: open microphone: {e}")))?;
        let system_device = resolve_input_device(&cfg.recording.input_device)?;
        let system_source = CpalSource::open_kind_with_grace(
            system_device,
            CaptureSource::SystemAudio,
            STOP_TAIL_GRACE,
        )
        .map_err(|e| Error::Internal(format!("meeting: open system audio (loopback): {e}")))?;

        let sources: Vec<(MeetingTrack, Box<dyn Source>)> = vec![
            (MeetingTrack::Mic, Box::new(mic_source)),
            (MeetingTrack::System, Box::new(system_source)),
        ];
        self.start_meeting_with_sources(state, sources).await
    }

    /// Roll back a partially-started meeting: cancel each already-started
    /// track's recorder (releasing its capture device) and delete its catalog
    /// row. Used when a later track fails to start, so a mid-start failure
    /// leaves no orphaned `recording`-status rows or live capture tasks behind.
    /// Best-effort — cleanup failures are logged, not propagated.
    async fn abort_partial_meeting(state: &AppState, tracks: Vec<MeetingTrackHandle>) {
        for t in tracks {
            if let Err(e) = t.recorder.cancel().await {
                tracing::warn!(id = %t.id, error = %e, "meeting rollback: cancel recorder failed");
            }
            if let Err(e) = state.catalog.delete(&t.id).await {
                tracing::warn!(id = %t.id, error = %e, "meeting rollback: delete catalog row failed");
            }
        }
    }

    /// Core meeting orchestration, decoupled from hardware so it can be tested
    /// with `SyntheticSource`s.
    ///
    /// For each `(track, source)` it mints a `RecordingId`, inserts a catalog
    /// row at `Recording` status carrying the shared `meeting_id` + track
    /// label, and starts an audio `Recorder` (always `Hold` mode — a meeting
    /// runs until explicitly stopped). All started recorders are tracked
    /// together so `stop_meeting` can finalize them as a unit. If any track
    /// fails to start, every already-started track is rolled back (see
    /// [`Self::abort_partial_meeting`]) and the error is returned.
    pub async fn start_meeting_with_sources(
        &self,
        state: &AppState,
        sources: Vec<(MeetingTrack, Box<dyn Source>)>,
    ) -> Result<String> {
        let mut meeting_lock = self.meeting.lock().await;
        if meeting_lock.is_some() {
            return Err(Error::AlreadyRecording {
                current: "meeting already in progress".into(),
            });
        }

        let meeting_id = format!("meeting-{}", RecordingId::new());
        let mut tracks = Vec::with_capacity(sources.len());

        // Wall-clock anchor for the whole meeting — both tracks are padded to this
        // elapsed duration on stop so mic and system stay time-aligned.
        let wall_started = Instant::now();
        // Catalog timestamp shared by both tracks.
        let started_at = Local::now();

        for (track, source) in sources {
            let id = RecordingId::new();
            let audio_path = state
                .paths
                .audio_dir
                .join(id.day_folder())
                .join(format!("{}.wav", id.file_stem()));

            let row = Recording {
                id: id.clone(),
                started_at,
                duration_ms: 0,
                audio_path: audio_path.to_string_lossy().into_owned(),
                in_place: false,
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
                meeting_id: Some(meeting_id.clone()),
                meeting_name: None,
                track: Some(track.as_str().to_string()),
                cleanup_model: None,
                diarized: false,
                summary: None,
                summary_model: None,
                tags: vec![],
            };
            // Insert the catalog row. If it fails, roll back every track already
            // started so we never leave orphaned `recording`-status rows or live
            // capture tasks behind.
            if let Err(e) = state.catalog.insert(&row).await {
                Self::abort_partial_meeting(state, tracks).await;
                return Err(e);
            }

            // A meeting always records in Hold mode — it ends only when the
            // user stops it (no silence auto-stop, no fixed duration).
            let recorder_cfg = RecorderConfig {
                mode: RecordMode::Hold,
                max_duration_ms: state.config.load().recording.max_duration_secs as u64 * 1000,
                silence_threshold_dbfs: state.config.load().recording.silence_threshold_dbfs,
                silence_window_ms: state.config.load().recording.silence_window_ms,
            };
            // Start the audio recorder. If it fails, delete the row we just
            // inserted *and* roll back the earlier tracks before bailing out.
            let capture_started = Instant::now();
            let recorder = match Recorder::start(source, recorder_cfg, None).await {
                Ok(r) => r,
                Err(e) => {
                    if let Err(del) = state.catalog.delete(&id).await {
                        tracing::warn!(id = %id, error = %del, "meeting rollback: delete catalog row failed");
                    }
                    Self::abort_partial_meeting(state, tracks).await;
                    return Err(e);
                }
            };

            state.events.emit(DaemonEvent::RecordingStarted {
                id: id.clone(),
                started_at,
                meeting_id: Some(meeting_id.clone()),
            });
            tracing::info!(id = %id, track = track.as_str(), session = %meeting_id, "meeting track started");

            tracks.push(MeetingTrackHandle {
                id,
                audio_path,
                started_at,
                track,
                recorder,
                capture_started,
            });
        }

        *meeting_lock = Some(ActiveMeeting {
            meeting_id: meeting_id.clone(),
            tracks,
            paused: false,
            wall_started,
        });
        tracing::info!(session = %meeting_id, "meeting started");
        Ok(meeting_id)
    }

    /// Stop the active meeting: finalize every track (write its WAV, mark the
    /// catalog row `Transcribing`, enqueue it for the normal pipeline) and emit
    /// a `RecordingStopped` for each. Returns the session id that was stopped.
    pub async fn stop_meeting(&self, state: &AppState) -> Result<String> {
        let meeting = self
            .meeting
            .lock()
            .await
            .take()
            .ok_or(Error::NotRecording)?;
        let meeting_id = meeting.meeting_id.clone();
        let wall_started = meeting.wall_started;
        // Snapshot meeting wall-clock length before stopping recorders (stop/drain can take time).
        let stop_at = Instant::now();
        let target_duration_ms = stop_at.duration_since(wall_started).as_millis() as i64;
        let sample_rate = phoneme_audio::format::SampleRate::HZ_16K.as_u32();

        tracing::info!(
            target_duration_ms = target_duration_ms,
            "meeting wall-clock duration for track alignment"
        );

        // Stop every recorder at once so one track doesn't keep capturing while
        // the other is draining (which skews sample counts vs wall-clock time).
        let stop_results =
            futures::future::join_all(meeting.tracks.into_iter().map(|handle| async move {
                let MeetingTrackHandle {
                    id,
                    audio_path,
                    started_at,
                    track,
                    recorder,
                    capture_started,
                } = handle;
                let track_late_by_ms =
                    capture_started.duration_since(wall_started).as_millis() as i64;
                let stop_result = recorder.stop_and_get_samples().await;
                (
                    id,
                    audio_path,
                    started_at,
                    track,
                    track_late_by_ms,
                    stop_result,
                )
            }))
            .await;

        struct StoppedTrack {
            id: RecordingId,
            audio_path: std::path::PathBuf,
            started_at: chrono::DateTime<Local>,
            track: MeetingTrack,
            track_late_by_ms: i64,
            first_content_from_wall_ms: Option<i64>,
            raw_samples: Vec<i16>,
        }

        let mut stopped: Vec<StoppedTrack> = Vec::new();

        for (id, audio_path, started_at, track, track_late_by_ms, stop_result) in stop_results {
            match stop_result {
                Ok((raw_samples, _duration_ms, first_non_silent_at)) => {
                    let first_content_from_wall_ms = first_non_silent_at
                        .map(|t| t.duration_since(wall_started).as_millis() as i64);
                    stopped.push(StoppedTrack {
                        id,
                        audio_path,
                        started_at,
                        track,
                        track_late_by_ms,
                        first_content_from_wall_ms,
                        raw_samples,
                    });
                }
                Err(e) => {
                    tracing::error!(id = %id, track = track.as_str(), error = %e, "meeting track finalize failed");
                    if let Err(err) = state
                        .catalog
                        .update_status(&id, RecordingStatus::TranscribeFailed)
                        .await
                    {
                        tracing::warn!(id = %id, error = %err, "failed to mark track as failed");
                    }
                }
            }
        }

        let align_inputs: Vec<TrackAlignInput> = stopped
            .iter()
            .map(|t| TrackAlignInput {
                samples: t.raw_samples.clone(),
                track_late_by_ms: t.track_late_by_ms,
                first_content_from_wall_ms: t.first_content_from_wall_ms,
                // The mic is continuous (dense); only the system/loopback track
                // may be sparse and need wall-clock first-content relocation.
                dense: matches!(t.track, MeetingTrack::Mic),
            })
            .collect();
        let aligned = align_meeting_tracks(&align_inputs, target_duration_ms, sample_rate);

        struct FinalizedTrack {
            id: RecordingId,
            audio_path: std::path::PathBuf,
            started_at: chrono::DateTime<Local>,
            track: MeetingTrack,
            samples: Vec<i16>,
            duration_ms: i64,
        }

        let mut track_data: Vec<FinalizedTrack> = Vec::new();

        for (meta, aligned_track) in stopped.into_iter().zip(aligned) {
            let capture_window_ms = (target_duration_ms - meta.track_late_by_ms).max(0);
            let expected_raw =
                phoneme_audio::meeting_align::ms_to_samples(capture_window_ms, sample_rate);

            tracing::info!(
                id = %meta.id,
                track = meta.track.as_str(),
                raw_samples = meta.raw_samples.len(),
                expected_raw_samples = expected_raw,
                aligned_samples = aligned_track.samples.len(),
                track_late_by_ms = meta.track_late_by_ms,
                first_content_from_wall_ms = ?meta.first_content_from_wall_ms,
                sparse = aligned_track.sparse,
                placement_ms = aligned_track.placement_ms,
                "aligned meeting track to wall-clock timeline"
            );

            track_data.push(FinalizedTrack {
                id: meta.id,
                audio_path: meta.audio_path,
                started_at: meta.started_at,
                track: meta.track,
                samples: aligned_track.samples,
                duration_ms: target_duration_ms,
            });
        }

        for FinalizedTrack {
            id,
            audio_path,
            started_at,
            track,
            samples,
            duration_ms: final_duration_ms,
        } in track_data
        {
            // Write the timeline-aligned samples to WAV.
            let audio_cfg = phoneme_audio::format::AudioConfig::phoneme_default();
            if let Err(e) = phoneme_audio::wav::write_wav(&audio_path, &samples, audio_cfg) {
                tracing::error!(id = %id, track = track.as_str(), error = %e, "failed to write WAV");
                if let Err(err) = state
                    .catalog
                    .update_status(&id, RecordingStatus::TranscribeFailed)
                    .await
                {
                    tracing::warn!(id = %id, error = %err, "failed to mark track as failed");
                }
                continue;
            }

            // Update catalog with the (possibly padded) duration
            state
                .catalog
                .update_status_and_duration(&id, RecordingStatus::Transcribing, final_duration_ms)
                .await?;

            let payload = HookPayload {
                id: id.clone(),
                timestamp: started_at,
                transcript: String::new(),
                audio_path: audio_path.to_string_lossy().into_owned(),
                duration_ms: final_duration_ms,
                model: String::new(),
                metadata: HookMetadata::current(),
            };
            state.inbox.enqueue(&payload).await?;
            crate::queue_worker::emit_queue_depth(state).await;

            state.events.emit(DaemonEvent::RecordingStopped {
                id: id.clone(),
                duration_ms: final_duration_ms,
                audio_path: audio_path.to_string_lossy().into_owned(),
                meeting_id: Some(meeting_id.clone()),
            });
            tracing::info!(id = %id, track = track.as_str(), ms = final_duration_ms, "meeting track stopped");
        }

        tracing::info!(session = %meeting_id, "meeting stopped");

        // Resume idle pre-capture now the meeting released the microphone.
        // No-op when pre-roll is disabled.
        self.ensure_preroll(state).await;
        Ok(meeting_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use phoneme_audio::format::AudioConfig;
    use phoneme_audio::source::{GeneratorSource, SyntheticSource};
    use phoneme_core::{Config, ListFilter};

    /// Build an `AppState` whose catalog/inbox/audio all live under a temp dir,
    /// so meeting orchestration can be tested without touching the real install.
    async fn test_state(tmp: &std::path::Path) -> AppState {
        // Redirect inbox/catalog/logs away from the real per-user AppData.
        std::env::set_var("PHONEME_DATA_LOCAL", tmp.join("data"));
        let mut cfg = Config::default();
        cfg.recording.audio_dir = tmp.join("audio").to_string_lossy().into_owned();
        // Disable idle pre-roll: it opens a real microphone via cpal, which
        // crashes (STATUS_ACCESS_VIOLATION) on a headless CI runner with no audio
        // device — the long-standing CI failure. Tests use synthetic sources and
        // must never touch real capture hardware.
        cfg.recording.pre_roll_ms = 0;
        AppState::new(cfg).await.expect("build test AppState")
    }

    #[tokio::test]
    async fn start_meeting_with_sources_produces_two_linked_recordings() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // Two synthetic sources stand in for the mic + system-audio captures.
        let audio_cfg = AudioConfig::phoneme_default();
        let (mic_src, mic_sink) = SyntheticSource::new(audio_cfg);
        let (sys_src, sys_sink) = SyntheticSource::new(audio_cfg);

        let meeting_id = state
            .recorder
            .start_meeting_with_sources(
                &state,
                vec![
                    (MeetingTrack::Mic, Box::new(mic_src)),
                    (MeetingTrack::System, Box::new(sys_src)),
                ],
            )
            .await
            .expect("start meeting");

        assert!(
            state.recorder.meeting_active().await,
            "meeting should be active"
        );

        // Feed a little audio into each track, then close the sinks so the
        // recorders can drain and finalize on stop.
        mic_sink.push(vec![100i16; 8_000]).await.unwrap();
        sys_sink.push(vec![200i16; 8_000]).await.unwrap();
        mic_sink.close();
        sys_sink.close();

        let stopped = state
            .recorder
            .stop_meeting(&state)
            .await
            .expect("stop meeting");
        assert_eq!(stopped, meeting_id);
        assert!(
            !state.recorder.meeting_active().await,
            "meeting should be cleared"
        );

        // Two catalog rows exist, both carrying the shared meeting_id and the
        // two distinct track labels.
        let rows = state.catalog.list(&ListFilter::default()).await.unwrap();
        let meeting_rows: Vec<_> = rows
            .iter()
            .filter(|r| r.meeting_id.as_deref() == Some(meeting_id.as_str()))
            .collect();
        assert_eq!(
            meeting_rows.len(),
            2,
            "meeting must produce exactly two recordings"
        );

        let mut tracks: Vec<&str> = meeting_rows
            .iter()
            .filter_map(|r| r.track.as_deref())
            .collect();
        tracks.sort_unstable();
        assert_eq!(tracks, vec!["mic", "system"]);

        // Both were enqueued for transcription (status flipped to Transcribing).
        for r in &meeting_rows {
            assert_eq!(
                r.status,
                RecordingStatus::Transcribing,
                "each meeting track must be enqueued for transcription"
            );
        }

        // Both WAVs were written to disk.
        for r in &meeting_rows {
            assert!(
                std::path::Path::new(&r.audio_path).exists(),
                "expected WAV written at {}",
                r.audio_path
            );
        }
    }

    #[tokio::test]
    async fn cannot_start_two_meetings_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        let audio_cfg = AudioConfig::phoneme_default();

        let (s1, _k1) = SyntheticSource::new(audio_cfg);
        state
            .recorder
            .start_meeting_with_sources(&state, vec![(MeetingTrack::Mic, Box::new(s1))])
            .await
            .expect("first meeting starts");

        let (s2, _k2) = SyntheticSource::new(audio_cfg);
        let err = state
            .recorder
            .start_meeting_with_sources(&state, vec![(MeetingTrack::Mic, Box::new(s2))])
            .await
            .expect_err("second meeting must be rejected");
        assert!(matches!(err, Error::AlreadyRecording { .. }));
    }

    #[tokio::test]
    async fn toggle_meeting_stops_an_active_meeting() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        let audio_cfg = AudioConfig::phoneme_default();

        // Stand up an active meeting via the synthetic-source path.
        let (mic_src, mic_sink) = SyntheticSource::new(audio_cfg);
        let (sys_src, sys_sink) = SyntheticSource::new(audio_cfg);
        state
            .recorder
            .start_meeting_with_sources(
                &state,
                vec![
                    (MeetingTrack::Mic, Box::new(mic_src)),
                    (MeetingTrack::System, Box::new(sys_src)),
                ],
            )
            .await
            .expect("meeting starts");
        assert!(state.recorder.meeting_active().await);

        // Drain a little audio so the tracks can finalize cleanly on stop.
        mic_sink.push(vec![100i16; 8_000]).await.unwrap();
        sys_sink.push(vec![200i16; 8_000]).await.unwrap();
        mic_sink.close();
        sys_sink.close();

        // Toggling while a meeting is active must stop it and report `false`
        // (no longer running) rather than trying to start a second meeting.
        let started = state
            .recorder
            .toggle_meeting(&state)
            .await
            .expect("toggle stops the meeting");
        assert!(!started, "toggle should report the meeting stopped");
        assert!(
            !state.recorder.meeting_active().await,
            "meeting should be cleared after toggle"
        );
    }

    #[tokio::test]
    async fn cannot_start_recording_while_meeting_active() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        let audio_cfg = AudioConfig::phoneme_default();

        // Occupy the recorder with a meeting (one synthetic track is enough).
        let (s, _sink) = SyntheticSource::new(audio_cfg);
        state
            .recorder
            .start_meeting_with_sources(&state, vec![(MeetingTrack::Mic, Box::new(s))])
            .await
            .expect("meeting starts");

        // A single-track recording must be refused while the meeting holds the
        // capture devices. The guard runs before any audio device is opened, so
        // this is safe to assert without real hardware.
        let err = state
            .recorder
            .start(&state, RecordMode::Hold, false)
            .await
            .expect_err("recording must be rejected during a meeting");
        assert!(matches!(err, Error::AlreadyRecording { .. }));
    }

    #[tokio::test]
    async fn cannot_start_meeting_while_single_recording_active() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // Simulate an in-flight single recording by populating `active`
        // directly (starting a real one would require audio hardware). The
        // active-recording guard in `start_meeting` runs before any device open.
        *state.recorder.active.lock().await = Some(ActiveRecording {
            id: RecordingId::new(),
            mode: RecordMode::Hold,
            audio_path: tmp.path().join("x.wav"),
            started_at: Local::now(),
            paused: false,
            in_place: false,
        });

        let err = state
            .recorder
            .start_meeting(&state)
            .await
            .expect_err("meeting must be rejected during a single recording");
        assert!(matches!(err, Error::AlreadyRecording { .. }));
    }

    #[tokio::test]
    async fn abort_partial_meeting_cancels_recorders_and_deletes_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        let audio_cfg = AudioConfig::phoneme_default();

        // Stand up one already-started track — a catalog row at `recording`
        // status plus a live recorder — exactly as the meeting loop leaves the
        // first track when a later track fails to start.
        let id = RecordingId::new();
        let audio_path = tmp.path().join("track.wav");
        let row = Recording {
            id: id.clone(),
            started_at: Local::now(),
            duration_ms: 0,
            audio_path: audio_path.to_string_lossy().into_owned(),
            in_place: false,
            transcript: None,
            model: None,
            status: RecordingStatus::Recording,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            meeting_name: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: Some("meeting-test".to_string()),
            track: Some(MeetingTrack::Mic.as_str().to_string()),
            cleanup_model: None,
            diarized: false,
            summary: None,
            summary_model: None,
            tags: vec![],
        };
        state.catalog.insert(&row).await.unwrap();

        let (src, _sink) = SyntheticSource::new(audio_cfg);
        let recorder = Recorder::start(
            Box::new(src),
            RecorderConfig {
                mode: RecordMode::Hold,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

        let handle = MeetingTrackHandle {
            id: id.clone(),
            audio_path,
            started_at: Local::now(),
            track: MeetingTrack::Mic,
            recorder,
            capture_started: Instant::now(),
        };

        // Roll back: the orphaned catalog row must be gone afterward, and the
        // cancelled recorder must not have written a WAV.
        DaemonRecorder::abort_partial_meeting(&state, vec![handle]).await;

        assert!(
            state.catalog.get(&id).await.unwrap().is_none(),
            "rollback must delete the orphaned recording row"
        );
        assert!(
            !std::path::Path::new(&row.audio_path).exists(),
            "cancelled recorder must not write a WAV"
        );
    }

    #[tokio::test]
    async fn meeting_tracks_match_wall_clock_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // GeneratorSource produces blocks at real-time rate — mimics continuous capture.
        let mic = GeneratorSource::new(1_600);
        let system = GeneratorSource::new(1_600);

        let meeting_id = state
            .recorder
            .start_meeting_with_sources(
                &state,
                vec![
                    (MeetingTrack::Mic, Box::new(mic)),
                    (MeetingTrack::System, Box::new(system)),
                ],
            )
            .await
            .expect("start meeting");

        tokio::time::sleep(Duration::from_millis(500)).await;

        state
            .recorder
            .stop_meeting(&state)
            .await
            .expect("stop meeting");

        let rows = state.catalog.list(&ListFilter::default()).await.unwrap();
        let meeting_rows: Vec<_> = rows
            .iter()
            .filter(|r| r.meeting_id.as_deref() == Some(meeting_id.as_str()))
            .collect();

        assert_eq!(meeting_rows.len(), 2);
        let durations: Vec<i64> = meeting_rows.iter().map(|r| r.duration_ms).collect();
        assert_eq!(
            durations[0], durations[1],
            "both tracks must share the same duration"
        );
        assert!(
            durations[0] >= 400,
            "expected at least ~500 ms of wall-clock audio, got {durations:?}"
        );

        let sample_counts: Vec<usize> = meeting_rows
            .iter()
            .map(|r| {
                let data = std::fs::read(&r.audio_path).expect("read wav");
                // WAV header is 44 bytes for our canonical format.
                (data.len().saturating_sub(44)) / 2
            })
            .collect();
        assert_eq!(
            sample_counts[0], sample_counts[1],
            "WAV sample counts must match"
        );
    }
}
