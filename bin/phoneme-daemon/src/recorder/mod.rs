//! Daemon recorder — first link in the chain. Owns the active capture and
//! ties its lifecycle to the catalog, the inbox queue, and the event bus.
//!
//! A recording is born here: `start` inserts the catalog row (status
//! `recording`), opens the audio source, and emits `RecordingStarted`;
//! `stop` finalizes the WAV, flips the row to `transcribing`, and hands the
//! work item to the durable inbox queue — where `queue_worker` →
//! `pipeline` take over. In-place dictations branch to `in_place`'s fast
//! lane (or a type-first pass) instead of the queue; `cancel` deletes the
//! row and keeps nothing.
//!
//! Invariants this module owns:
//! - **At most one capture** — a single recording (`active`) OR a two-track
//!   meeting (`meeting`), never both; starts cross-check the other slot
//!   before reserving theirs, always in the same lock order (`meeting` →
//!   `active`) so the two paths can't deadlock or double-open the mic.
//! - **Toggle atomicity** — `toggle_meeting` holds `toggle_guard` across its
//!   read+act so a double-tapped hotkey can't race two starts or two stops.
//! - **No slow await under a state lock** — `stop`/`cancel` take the slot
//!   and recorder handle in one short critical section and release the locks
//!   before preview teardown / finalization, keeping `RecordStatus` and
//!   other control IPC responsive mid-stop.
//! - **Idle pre-roll** — between recordings an optional background task
//!   feeds a ring buffer holding the last `pre_roll_ms` of mic audio; start
//!   snapshots and prepends it, then reuses (or reopens) the source.
//! - **Live preview** — while recording (and `streaming_preview` is on), a
//!   loop transcribes a rolling tail window and emits
//!   `TranscriptionPartial`; it only runs a tick when the shared
//!   `whisper_sem` permit is free, so it can never starve a final
//!   transcription. The stitcher (see [`preview`]) keeps the displayed
//!   caption forward-growing as the window slides.
//! - **Meetings** — both tracks record concurrently, share a `meeting_id`,
//!   and are wall-clock aligned on stop; a partial start failure aborts
//!   cleanly, and a partial stop failure still finalizes the healthy track.
//!
//! The live-preview machinery lives in [`preview`]; the Meeting Mode machinery
//! lives in [`meeting`]. The single-recording lifecycle and the shared
//! `DaemonRecorder` state stay here.

mod meeting;
mod preview;

use crate::app_state::AppState;
use chrono::Local;
use phoneme_audio::device::resolve_input_device;
use phoneme_audio::format::SampleRate;
use phoneme_audio::preroll::PreRollBuffer;
use phoneme_audio::recorder::{Recorder, RecorderConfig};
use phoneme_audio::source::{CpalSource, GeneratorSource, Source};
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

/// How long to keep a recording's capture stream alive after a stop is
/// requested, so the OS can hand over audio it had already buffered at stop
/// time instead of discarding it. Without this, manually-stopped recordings can
/// lose the final ~tens-of-milliseconds and sound clipped at the end. Applied
/// only to real recording/meeting sources — not the rolling pre-roll buffer.
pub(super) const STOP_TAIL_GRACE: Duration = Duration::from_millis(150);

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

#[derive(Debug, Clone)]
pub struct ActiveRecording {
    pub id: RecordingId,
    // Threaded through the start/stop/cancel flows but not read off the snapshot
    // yet; kept for the doctor / debug endpoints.
    #[allow(dead_code)]
    pub mode: RecordMode,
    pub audio_path: PathBuf,
    pub started_at: chrono::DateTime<Local>,
    pub paused: bool,
    pub in_place: bool,
    /// Lowercased executable stem of the window focused when the recording
    /// started (e.g. `"code"`), captured only for in-place dictations. Drives
    /// the per-app type/paste/off override at typing time. `None` off Windows,
    /// when no window was focused, or when this isn't a dictation.
    pub focused_app: Option<String>,
    /// Title of the focused window at start, captured only for in-place
    /// dictations AND only when `[in_place].app_context` is on and the app
    /// isn't denylisted. Potentially sensitive — used solely in the LLM cleanup
    /// prompt, never logged or persisted. `None` whenever context is off.
    pub focused_window_title: Option<String>,
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

/// Which kind of preview loop a [`PreviewTask`] is, so a meeting source-swap can
/// tear down just the caption loop(s) and leave the cheap waveform loop running.
/// Tearing down both would permanently kill the "it hears me" pill on the first
/// toggle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewKind {
    /// A `TranscriptionPartial` caption loop (whisper). Stopped on a source-swap.
    Caption,
    /// The cheap `AudioLevelSample` waveform loop (no whisper). Survives a
    /// source-swap; only torn down when the whole recording/meeting stops.
    Level,
}

/// A running streaming-preview loop: periodically transcribes the in-progress
/// recording and emits `TranscriptionPartial` events (caption), or samples mic
/// RMS for the waveform (level). Present only while a recording is active *and*
/// the relevant feature is enabled.
pub(super) struct PreviewTask {
    /// What this loop is — caption vs waveform. See [`PreviewKind`].
    pub(super) kind: PreviewKind,
    /// Sending (or dropping) tells the loop to stop and exit.
    pub(super) stop_tx: tokio::sync::oneshot::Sender<()>,
    /// The loop's join handle — awaited on stop so it tears down cleanly.
    pub(super) task: tokio::task::JoinHandle<()>,
}

/// One track of an in-flight meeting: its catalog id, where the WAV will be
/// written, when it started, the track label, the live recorder handle, and
/// when capture actually began (for timeline alignment).
pub(super) struct MeetingTrackHandle {
    pub(super) id: RecordingId,
    pub(super) audio_path: PathBuf,
    pub(super) started_at: chrono::DateTime<Local>,
    pub(super) track: MeetingTrack,
    pub(super) recorder: Recorder,
    pub(super) capture_started: Instant,
}

#[derive(Clone, Default)]
pub struct DaemonRecorder {
    pub(super) active: Arc<Mutex<Option<ActiveRecording>>>,
    handle: Arc<Mutex<Option<Recorder>>>,
    /// Idle pre-roll pre-capture, present only while enabled and not actively
    /// recording. `None` means no continuous capture is running (the default).
    preroll: Arc<Mutex<Option<PreRoll>>>,
    /// Streaming transcription preview loops, present only while recording with
    /// the feature enabled. Empty (the default) means no preview is running.
    /// Single recordings and meetings in "toggle" mode run one loop; meetings
    /// in "both" mode run one per track.
    pub(super) preview: Arc<Mutex<Vec<PreviewTask>>>,
    /// The active meeting's preview sources — (recording id, track label,
    /// snapshot handle) per track — kept so `SetPreviewSource` can switch which
    /// track feeds the preview mid-meeting ("toggle" mode). Cleared when the
    /// meeting stops/cancels.
    pub(super) meeting_preview_sources:
        Arc<Mutex<Vec<(RecordingId, String, phoneme_audio::recorder::SnapshotHandle)>>>,
    /// In-flight meeting (Meeting Mode, v1.6). `None` (the default) means no
    /// meeting is recording. Held separately from `active` so the existing
    /// single-recording path is completely untouched.
    pub(super) meeting: Arc<Mutex<Option<meeting::ActiveMeeting>>>,
    /// Serializes `MeetingToggle`. Without it the toggle is check-then-act
    /// across two separate `meeting` lock acquisitions (read state, then
    /// start/stop), so two near-simultaneous hotkey presses can both observe
    /// "no meeting" and both call `start_meeting` (one then fails), or both
    /// observe "meeting" and race to stop. Held for the whole toggle so the
    /// decision and the action are atomic with respect to other toggles.
    pub(super) toggle_guard: Arc<Mutex<()>>,
}

/// Whether pre-roll should be active for the current config: opt-in
/// (`pre_roll_ms > 0`) and microphone-only (loopback/system-audio is skipped).
fn preroll_enabled(cfg: &phoneme_core::Config) -> bool {
    cfg.recording.pre_roll_ms > 0 && cfg.recording.source == CaptureSource::Microphone
}

/// Whether a just-stopped recording takes the dictation FAST LANE
/// ([`crate::in_place::spawn_fast_lane`]) rather than the queued full pipeline.
///
/// Only in-place dictations are ever eligible. Of those, `[in_place].full_pipeline`
/// already routes the recording through the full pipeline (transcribe → recipe →
/// type). The added gate is `has_recipe`: a custom-hotkey in-place binding that
/// names a non-empty recipe must run the full pipeline too, because the fast lane
/// never enters `pipeline::run` and so would silently drop the recipe. Pure, so
/// the routing decision is unit-testable without a live recorder.
fn wants_fast_lane(in_place: bool, full_pipeline: bool, has_recipe: bool) -> bool {
    in_place && !full_pipeline && !has_recipe
}

/// Whether the recorder should fire a "type-first" pass — typing the quick
/// transcription the instant it is ready, ahead of the queued full pipeline.
///
/// Only in-place dictations with `[in_place].type_first` qualify, and only when
/// they have no recipe. A recipe reshapes the text (summarize, polish, …), so
/// the quick raw transcription is the wrong thing to type; a recipe-bearing
/// in-place recording instead gets its single insertion at the end of the
/// pipeline (the recipe's result — see `pipeline::pipeline_should_type`). This
/// gate is the exact inverse of the condition `pipeline_should_type` suppresses
/// on, so the text lands exactly once on every in-place path. Pure, so the
/// decision is unit-testable without a live recorder.
fn wants_type_first(in_place: bool, type_first: bool, has_recipe: bool) -> bool {
    in_place && type_first && !has_recipe
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

    /// Whether a single recording OR a meeting is currently in flight. Used to
    /// refuse destructive whole-catalog operations (the rebuild) while capture
    /// is live, so we never wipe the row of a recording that's still being
    /// written.
    pub async fn is_busy(&self) -> bool {
        self.active.lock().await.is_some() || self.meeting.lock().await.is_some()
    }

    /// Start idle pre-roll pre-capture if it's enabled for the current config
    /// and not already running. Safe to call repeatedly (idempotent) and
    /// whenever the daemon is idle (startup, after a recording finishes).
    ///
    /// When pre-roll is disabled this is a no-op, so the default path keeps the
    /// microphone closed between recordings.
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
    pub(super) async fn take_preroll_samples(&self) -> (Vec<i16>, Option<Box<dyn Source>>) {
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
        let source = match task.await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "pre-roll idle task panicked; starting without pre-roll source");
                None
            }
        };
        let samples = ring.lock().await.to_vec();
        if !samples.is_empty() {
            tracing::info!(
                samples = samples.len(),
                "pre-roll: prepending buffered audio"
            );
        }
        (samples, source)
    }

    /// Start a recording. Returns `AlreadyRecording` if one is in flight.
    pub async fn start(
        &self,
        state: &AppState,
        mode: RecordMode,
        in_place: bool,
        source_override: Option<CaptureSource>,
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

        // Capture the foreground app for in-place dictations, at start — this is
        // the window the user is dictating into, before the brief recording can
        // shift focus. The process stem keys the per-app type/paste/off override;
        // the window title is read only when app-aware context is opted in and
        // the app isn't denylisted (privacy-first — off by default reads nothing).
        // Both Win32 calls and `config.load()` are synchronous (no await), so
        // doing this under the `active` guard is sound.
        let (focused_app, focused_window_title) = if in_place {
            let cfg = state.config.load();
            let app = phoneme_core::foreground::foreground_app();
            let exe = app.as_ref().map(|a| a.exe_name.clone());
            let title = app.filter(|a| {
                !a.window_title.is_empty()
                    && cfg
                        .in_place
                        .may_read_window_title(Some(a.exe_name.as_str()))
            });
            (exe, title.map(|a| a.window_title))
        } else {
            (None, None)
        };

        // Whether this dictation streams its text live (`[in_place].stream_type`):
        // in-place, the flag on, and the focused app resolves to typed (not paste/
        // off) delivery. Computed here, before `focused_app` moves into the active
        // slot below; the stop reconcile re-checks the same conditions so both
        // agree on whether streaming happened.
        let stream_type = {
            let c = state.config.load();
            in_place
                && c.in_place.stream_type
                && c.in_place.resolve_type_mode(focused_app.as_deref()) == "type"
        };

        // Per-app tone: pick the cleanup recipe (and so the LLM's register) by the
        // app focused at record START. Resolve `[in_place].app_recipes` against the
        // foreground stem now, while `focused_app` is still in hand; the resulting
        // id is seeded into the `pending_recipe` ledger only after the catalog row
        // commits (below), so an insert failure can't leak an entry.
        //
        // Seeding the same ledger a custom-hotkey recipe uses makes the per-app
        // recipe behave identically for the rest of the lifecycle: it forces the
        // full pipeline at `stop()` via `has_recipe`, is claimed+resolved by
        // `pipeline::run`, and is dropped by `cancel()` — with no other code change.
        //
        // Precedence is binding-wins: this seed happens inside `start()`, BEFORE the
        // `RecordStart`/`RecordToggle` handler calls `stash_hotkey_overrides`, which
        // overwrites the entry with the binding's own non-empty recipe. So a
        // deliberately-bound per-key chain always wins; the per-app map only fills
        // in a recipe when the binding left one empty. With the default empty
        // `app_recipes`, `resolve_app_recipe` returns `None` and nothing is seeded —
        // today's behavior, byte-for-byte.
        let app_recipe: Option<String> = if in_place {
            state
                .config
                .load()
                .in_place
                .resolve_app_recipe(focused_app.as_deref())
                .map(str::to_string)
        } else {
            None
        };

        // Reserve the active slot immediately so concurrent starts fail.
        *active = Some(ActiveRecording {
            id: id.clone(),
            mode,
            audio_path: audio_path.clone(),
            started_at,
            paused: false,
            in_place,
            focused_app,
            focused_window_title,
        });
        drop(active);

        // Resolve the capture source: a custom hotkey can override the global
        // `[recording].source` per-binding (`source_override`). Record which it was
        // on the row's `track` so the list's Source column reflects the real source
        // instead of assuming "single == microphone". Loaded once here and reused
        // for the capture stream below.
        let app_cfg = state.config.load();
        let kind = source_override.unwrap_or(app_cfg.recording.source);
        let track = Some(
            match kind {
                CaptureSource::SystemAudio => "system",
                CaptureSource::Microphone => "mic",
            }
            .to_string(),
        );

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
            track,
            in_place,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            pinned: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            entities_model: None,
            chapters_model: None,
            tasks_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            mean_confidence: None,
            detected_language: None,
            ext_ref: None,
            tags: vec![],
            entities: vec![],
            tasks: vec![],
            speaker_names: vec![],
        };
        if let Err(e) = state.catalog.insert(&row).await {
            *self.active.lock().await = None;
            return Err(e);
        }

        // Seed the per-app tone recipe (resolved above) now that the row is
        // committed — so an insert failure that bailed out above never leaves a
        // stray `pending_recipe` entry. From here it rides the existing recipe
        // lifecycle (see the `app_recipe` resolution comment above).
        if let Some(recipe) = app_recipe {
            state
                .pending_recipe
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.clone(), recipe);
        }

        // If idle pre-roll pre-capture is running, stop it and grab the buffered
        // audio to prepend; this also releases the microphone (or returns it) before we reopen
        // it for the recording. Empty when pre-roll is disabled (default path).
        let (prepend, preroll_source) = self.take_preroll_samples().await;
        // Pre-roll is captured with the global source (and only ever the
        // microphone). If a per-keybind override switched this recording to a
        // different source, that buffered audio is from the wrong device — drop it
        // and open a fresh stream for the override source below, so the recording
        // actually captures (and is labelled as) the source the binding asked for.
        let (prepend, preroll_source) = if kind == app_cfg.recording.source {
            (prepend, preroll_source)
        } else {
            (Vec::new(), None)
        };
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
            // The auto-stop budget measures freshly captured audio only: the
            // recorder excludes the prepended pre-roll from both the
            // `Duration { secs }` auto-stop and this `max_duration_ms` ceiling.
            // So a `max_duration_secs` of N yields N seconds of live capture with
            // the pre-roll lead-in added on top — the requested length is never
            // shortened by pre-roll, and the cap bounds the same amount of speech
            // whether or not pre-roll is enabled.
            max_duration_ms: state.config.load().recording.max_duration_secs as u64 * 1000,
            silence_threshold_dbfs: state.config.load().recording.silence_threshold_dbfs,
            silence_window_ms: state.config.load().recording.silence_window_ms,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut recorder =
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
        // Peak-normalize the finalized WAV when enabled (off by default). The
        // ceiling is applied at finalize time from the recorder's own field, so
        // setting it on the owned instance now — before it moves into the active
        // slot — is sufficient. The live preview snapshot is never normalized.
        {
            let snap = state.config.load();
            if snap.recording.normalize {
                recorder.set_normalize(Some(snap.recording.normalize_target_dbfs));
            }
        }
        // Clone a snapshot handle before moving the recorder into the slot, so
        // the preview loop can read this recording's audio.
        let preview_snapshot = recorder.snapshot_handle();
        // A second handle for the cheap audio-level loop (waveform pill); it
        // never touches whisper, so it runs independently of the caption preview.
        let level_snapshot = recorder.snapshot_handle();
        *self.handle.lock().await = Some(recorder);

        // Spawn a task to auto-finalize when the recorder task ends on its own:
        // a self-terminating mode hitting its stop condition (Oneshot silence /
        // Duration elapsed), or, for any mode including Hold, the capture device
        // failing mid-recording (the mic was unplugged). `on_done` fires whenever
        // the loop ends, so this single task covers both: it calls `stop`, which
        // finalizes the partial take and (when the source flagged a device loss)
        // emits `DeviceLost`. A normal user `RecordStop`/cancel consumes the
        // recorder first, so this task then finds the slot empty and `stop`
        // returns `NotRecording` — a benign "already stopped", not a failure, so
        // it's swallowed rather than logged.
        let daemon_recorder = self.clone();
        let state_clone = state.clone();
        tokio::spawn(async move {
            if rx.await.is_ok() {
                match daemon_recorder.stop(&state_clone).await {
                    Ok(_) | Err(Error::NotRecording) => {}
                    Err(e) => tracing::warn!("auto-stop failed: {e}"),
                }
            }
        });

        // Spawn the live streaming-preview loop. No-op unless
        // `recording.streaming_preview` is enabled (default: off). Runs for
        // in-place dictation too: the dictation overlay shows the same caption,
        // and the loop self-throttles around the latency-critical paste — each
        // tick only transcribes when it can `try_acquire` the single serial
        // whisper permit, so the dictation's own transcribe always wins it and
        // an in-flight tick simply skips. The dictation stop path tears this
        // preview down without awaiting (see `stop`), so a preview tick can
        // never delay the paste ("constantly listening, never pastes").
        // Streaming-type (`stream_type`, computed above before `focused_app`
        // moved into the active slot): force the preview loop on; it then types
        // committed words live and the stop reconcile patches them to the final.
        // Reset the rolling typed state at the start of every recording (not just
        // streaming ones), so the stop path can use "is it non-empty?" as the
        // did-we-stream signal. That stays correct even if the user toggles
        // stream_type mid-recording, and it clears any leaked write from a prior
        // dictation.
        *state.stream_typed.lock().await = String::new();
        self.start_preview(state, id.clone(), preview_snapshot, false, stream_type)
            .await;
        // The live audio-level waveform runs for every capture (including
        // in-place dictation), gated only on `recording.preview_waveform`. It's
        // cheap (RMS of a tiny tail, no whisper permit) so it never reintroduces
        // the preview's record-time lag.
        self.start_level_loop(state, id.clone(), level_snapshot)
            .await;

        state.events.emit(DaemonEvent::RecordingStarted {
            id: id.clone(),
            started_at,
            meeting_id: None,
            track: None,
        });
        tracing::info!(id = %id, mode = ?mode, "recording started");
        Ok(id)
    }

    /// Stop the current recording, write the WAV, and mark the catalog row
    /// Transcribing. Normal recordings enqueue into the inbox for the serial
    /// pipeline; in-place dictations (unless `[in_place].full_pipeline`) hand
    /// off to the dictation fast lane instead — and with `full_pipeline` +
    /// `type_first`, a type-only pass runs in addition to the enqueue so the
    /// text lands before the pipeline finishes. See `in_place.rs`.
    pub async fn stop(&self, state: &AppState) -> Result<RecordingId> {
        // Take the active slot and the recorder handle in one short critical
        // section, then drop both guards before any slow await. The preview
        // teardown below can block on an in-flight transcription tick (up to
        // the provider timeout), and holding `active` through it would stall
        // every status/control IPC (RecordStatus, pause, cancel, …) for that
        // long. Taking the recorder together with the slot keeps stop/start
        // ordering sound: a concurrent start sees the slot free only after the
        // old recorder handle is already ours, so a freshly-started recorder
        // can never be grabbed and finalized to the old recording's path.
        let (active, recorder) = {
            let mut active_lock = self.active.lock().await;
            let active = active_lock.take().ok_or(Error::NotRecording)?;
            let recorder = self.handle.lock().await.take().ok_or(Error::NotRecording)?;
            (active, recorder)
        };
        // Stop the streaming-preview loop before finalizing so it isn't
        // mid-snapshot of a recorder being consumed. No-op when no preview is
        // running.
        //
        // In-place dictation aborts the loop instead of awaiting it: the
        // dictation transcribe-and-paste is on the latency path, and awaiting an
        // in-flight preview tick (which can hold the whisper permit for up to
        // the provider timeout) would add that tick's duration to stop→paste.
        // Aborting releases the held permit immediately so the detached fast
        // lane (`spawn_fast_lane`) can grab it right away. Abort skips the
        // loop's own temp-WAV cleanup, so we remove this recording's preview WAV
        // best-effort below. Normal recordings/meetings keep the graceful,
        // await-based teardown (which deletes its own temp WAV); aborting those
        // would leak their preview WAVs.
        self.stop_preview(active.in_place).await;
        if active.in_place {
            let preview_wav =
                std::env::temp_dir().join(format!("phoneme-preview-{}.wav", active.id.file_stem()));
            let _ = tokio::fs::remove_file(&preview_wav).await;
        }
        let result = recorder.stop_and_finalize(&active.audio_path).await?;

        state
            .catalog
            .update_status_and_duration(
                &active.id,
                RecordingStatus::Transcribing,
                result.duration_ms,
            )
            .await?;

        // Dictation fast lane: an in-place recording skips the serial inbox
        // queue and the full pipeline (cleanup/summary/tags/hooks) unless
        // `[in_place].full_pipeline` opts back in — transcribe → polish →
        // type, with persistence off the latency path. See `in_place.rs`.
        //
        // A custom-hotkey in-place binding that names a non-empty recipe is the
        // exception: the fast lane never enters `pipeline::run`, so its recipe
        // would never execute. Such a recording takes the full pipeline instead
        // (so the recipe runs and reshapes the text), and the pipeline's
        // end-of-run typing then inserts the recipe's result in place — exactly
        // the `full_pipeline = true` flow, just opted into per-binding. The
        // recipe ledger is only peeked here; `pipeline::run` claims and removes
        // it (and the model override) the same way it does for a normal queued
        // recording.
        let in_place_cfg = state.config.load().in_place.clone();
        let has_recipe = active.in_place
            && state
                .pending_recipe
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&active.id)
                .is_some_and(|r| !r.trim().is_empty());
        let fast_lane = wants_fast_lane(active.in_place, in_place_cfg.full_pipeline, has_recipe);
        if fast_lane {
            // A genuine fast-lane dictation never reaches `pipeline::run`, the
            // sole place the per-recording ledgers are claimed. Claim them here
            // so nothing leaks keyed by this (soon-dead) id: the model override
            // (if the binding carried one) is threaded into the fast-lane
            // transcription, and any stray recipe entry is dropped. A true fast
            // lane never has a recipe entry — a recipe forces the full pipeline
            // above — but the defensive remove keeps the contract airtight.
            let fast_lane_model = state
                .pending_overrides
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&active.id);
            state
                .pending_recipe
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&active.id);
            crate::in_place::spawn_fast_lane(
                state.clone(),
                active.id.clone(),
                active.audio_path.clone(),
                active.focused_app.clone(),
                active.focused_window_title.clone(),
                fast_lane_model,
            );
        } else {
            // A non-fast-lane in-place dictation (full pipeline, or a
            // recipe-bearing binding) types its result from the end of
            // `pipeline::run`. The fast lane passes `focused_app` directly to its
            // typing; the pipeline can't see this recording's foreground app, so
            // stash it in the side-channel keyed by id — `pipeline::run` claims it
            // and resolves the per-app type/paste/off override, exactly like the
            // fast lane. Only for an in-place recording with a known foreground
            // app; mirrors how `pending_recipe` is populated. (Don't populate for
            // the fast lane — it already has `focused_app` in hand.)
            if active.in_place {
                if let Some(app) = active.focused_app.as_ref().filter(|a| !a.trim().is_empty()) {
                    state
                        .pending_focused_app
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(active.id.clone(), app.clone());
                }
            }
            // Full-pipeline dictation with `type_first`: the text shouldn't
            // wait for the queue, so a type-only pass types the quick
            // transcription right away, alongside the normal enqueue below — the
            // pipeline still runs every step for the library copy, but skips
            // its own end-of-run typing so the text lands exactly once.
            //
            // A recipe-bearing in-place binding is excluded: its recipe reshapes
            // the text (summarize, polish, …), so the quick raw transcription is
            // the wrong thing to type. For it the pipeline owns the single
            // insertion of the recipe's result at the end (see
            // `pipeline_should_type`) — type-first here would either land the
            // raw text twice or land the raw text instead of the recipe output.
            if wants_type_first(active.in_place, in_place_cfg.type_first, has_recipe) {
                crate::in_place::spawn_type_first(
                    state.clone(),
                    active.id.clone(),
                    active.audio_path.clone(),
                    active.focused_app.clone(),
                    active.focused_window_title.clone(),
                );
            }
            let payload = HookPayload {
                id: active.id.clone(),
                timestamp: active.started_at,
                transcript: String::new(),
                audio_path: active.audio_path.to_string_lossy().into_owned(),
                duration_ms: result.duration_ms,
                model: String::new(),
                metadata: HookMetadata::current(),
            };
            // Mark it Queued, not Transcribing: it's waiting behind anything
            // ahead of it in the serial inbox, and the pipeline (`run`) flips
            // it to Transcribing the moment the worker actually claims it. The
            // fast lane above keeps Transcribing — it transcribes immediately.
            state
                .catalog
                .update_status(&active.id, RecordingStatus::Queued)
                .await?;
            state.inbox.enqueue(&payload).await?;
            crate::queue_worker::emit_queue_depth(state).await;
        }

        state.events.emit(DaemonEvent::RecordingStopped {
            id: active.id.clone(),
            duration_ms: result.duration_ms,
            audio_path: active.audio_path.to_string_lossy().into_owned(),
            meeting_id: None,
        });
        // Capture ended because the input device failed mid-recording (the mic
        // was unplugged). The partial take above was saved and enqueued exactly
        // as normal; this extra event just lets the UI tell the user why it
        // stopped, linking to the saved partial via `id`. Emitted only on a
        // genuine device loss — a normal stop / auto-stop leaves it false.
        if result.device_lost {
            tracing::warn!(id = %active.id, ms = result.duration_ms, "recording ended: capture device lost (saved the partial)");
            state.events.emit(DaemonEvent::DeviceLost {
                id: active.id.clone(),
                captured_ms: result.duration_ms,
            });
        }
        tracing::info!(id = %active.id, ms = result.duration_ms, "recording stopped");

        // Resume idle pre-capture (`ensure_preroll` re-acquires the active
        // lock, which was released above). No-op when pre-roll is disabled.
        let id = active.id;
        self.ensure_preroll(state).await;
        Ok(id)
    }

    /// Cancel the current recording: discard samples, delete catalog row, no
    /// WAV, no inbox.
    pub async fn cancel(&self, state: &AppState) -> Result<RecordingId> {
        // What cancel found to tear down, moved out of the state mutexes so
        // the slow awaits below (preview teardown, recorder cancel) run with
        // no lock held — same reasoning as `stop`.
        enum Taken {
            Single(ActiveRecording, Option<Recorder>),
            Meeting(Box<meeting::ActiveMeeting>),
        }
        let taken = {
            let mut active_lock = self.active.lock().await;
            match active_lock.take() {
                Some(active) => {
                    let recorder = self.handle.lock().await.take();
                    Taken::Single(active, recorder)
                }
                None => {
                    let mut meeting_lock = self.meeting.lock().await;
                    match meeting_lock.take() {
                        Some(meeting) => Taken::Meeting(Box::new(meeting)),
                        None => return Err(Error::NotRecording),
                    }
                }
            }
        };
        match taken {
            Taken::Meeting(meeting) => {
                let meeting = *meeting;
                // Tear down the live-preview loop before cancelling the track
                // recorders. No-op when no preview is running.
                self.stop_preview(false).await;
                self.meeting_preview_sources.lock().await.clear();
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

                self.ensure_preroll(state).await;
                Ok(id)
            }
            Taken::Single(active, recorder) => {
                // Stop the preview loop before tearing down the recorder. No-op
                // when off.
                self.stop_preview(false).await;
                // Clear the rolling streaming-type state so a cancelled
                // streaming-type dictation can't leak its live-typed words into a
                // later recording's reconcile (the stop path keys "did we stream?"
                // off this being non-empty). Deliberately no auto-backspace of the
                // already-typed text on cancel: the focus may have moved since the
                // words were typed, so a blind backspace could delete from the
                // wrong window (the same wrong-window risk the stop reconcile guards
                // against, and the focus guard lives in another lane). Any live text
                // the user already saw typed is intentionally left in place; only the
                // rolling state is reset here.
                *state.stream_typed.lock().await = String::new();
                if let Some(recorder) = recorder {
                    if let Err(e) = recorder.cancel().await {
                        tracing::warn!("failed to cancel recorder: {e}");
                    }
                }
                state.catalog.delete(&active.id).await?;
                // A custom-hotkey recording canceled mid-capture never reaches
                // `pipeline::run` — the sole place the per-recording ledgers are
                // otherwise claimed — so its `pending_recipe` / `pending_overrides`
                // entry would leak (bounded, never misroutes thanks to unique ids,
                // but still a leak). Drop both here keyed by the (now-dead) id,
                // recovering from a poisoned lock like the stash path does.
                state
                    .pending_overrides
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&active.id);
                state
                    .pending_recipe
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&active.id);
                // The focused-app side-channel is only ever populated for a
                // non-fast-lane in-place dictation at enqueue time, so a cancel
                // here normally has nothing to drop — but mirror the recipe /
                // overrides removals defensively so no terminal path can leak it.
                state
                    .pending_focused_app
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&active.id);
                state.events.emit(DaemonEvent::RecordingCancelled {
                    id: active.id.clone(),
                });
                tracing::info!(id = %active.id, "recording cancelled");

                // Resume idle pre-capture. No-op when pre-roll is disabled.
                let id = active.id;
                self.ensure_preroll(state).await;
                Ok(id)
            }
        }
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
                // Mark when (on the meeting's wall clock) this pause began, so
                // stop_meeting can fold the paused span out of the timeline.
                meeting.paused_at_ms = Some(
                    Instant::now()
                        .duration_since(meeting.wall_started)
                        .as_millis() as i64,
                );
                tracing::info!(session = %meeting.meeting_id, "meeting paused");
                return Ok(meeting.tracks[0].id.clone());
            }
            return Err(Error::NotRecording);
        }
        let active = active_lock
            .as_mut()
            .expect("active is Some; the is_none() case returned above");
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
                // Close the open pause span on the meeting's wall clock.
                if let Some(start_ms) = meeting.paused_at_ms.take() {
                    let end_ms = Instant::now()
                        .duration_since(meeting.wall_started)
                        .as_millis() as i64;
                    meeting.pause_spans_ms.push((start_ms, end_ms));
                }
                tracing::info!(session = %meeting.meeting_id, "meeting resumed");
                return Ok(meeting.tracks[0].id.clone());
            }
            return Err(Error::NotRecording);
        }
        let active = active_lock
            .as_mut()
            .expect("active is Some; the is_none() case returned above");
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app_state::AppState;
    use phoneme_core::Config;

    /// RAII guard that sets an env var for a test and restores its prior value (or
    /// removes it) on drop — even if the test panics. Honors the repo's
    /// no-bare-`set_var` convention: a forgotten/early-aborted `remove_var` can't
    /// leak a global into a sibling test. Hold it in a `let _guard = …;` binding for
    /// the duration of the test.
    struct EnvVarGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// Build an `AppState` whose catalog/inbox/audio all live under a temp dir,
    /// so meeting orchestration can be tested without touching the real install.
    pub(crate) async fn test_state(tmp: &std::path::Path) -> AppState {
        let mut cfg = Config::default();
        cfg.recording.audio_dir = tmp.join("audio").to_string_lossy().into_owned();
        // Disable idle pre-roll: it opens a real microphone via cpal, which
        // crashes (STATUS_ACCESS_VIOLATION) on a headless CI runner with no audio
        // device. Tests use synthetic sources and must never touch real capture
        // hardware.
        cfg.recording.pre_roll_ms = 0;
        // Explicit data-local (no global `set_var`) so parallel tests don't race
        // on the shared `PHONEME_DATA_LOCAL` env var — see `AppState::new_in`.
        AppState::new_in(cfg, Some(tmp.join("data")))
            .await
            .expect("build test AppState")
    }

    #[tokio::test]
    async fn stop_keeps_status_queries_responsive_during_preview_teardown() {
        // `stop` must not hold the active-recording lock across the preview
        // teardown — a slow in-flight preview tick (here a stand-in task that
        // takes 1.5 s to wind down) would otherwise block every status and
        // control IPC for its whole duration.
        let _backend = EnvVarGuard::set("PHONEME_AUDIO_BACKEND", "synthetic");
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        state
            .recorder
            .start(&state, RecordMode::Hold, false, None)
            .await
            .expect("start synthetic recording");

        // Inject a preview task that ignores its stop signal for 1.5 s — the
        // shape of a preview loop stuck inside a slow transcription tick.
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = stop_rx.await;
            tokio::time::sleep(Duration::from_millis(1_500)).await;
        });
        state.recorder.preview.lock().await.push(PreviewTask {
            kind: PreviewKind::Caption,
            stop_tx,
            task,
        });

        let recorder = state.recorder.clone();
        let stop_state = state.clone();
        let stop_task = tokio::spawn(async move { recorder.stop(&stop_state).await });

        // Let the stop reach the preview teardown, then prove the active slot
        // is already free: a status query must answer immediately instead of
        // queueing behind the teardown.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = tokio::time::timeout(Duration::from_millis(500), state.recorder.current())
            .await
            .expect("status query must not block behind the preview teardown");
        assert!(status.is_none(), "the active slot must already be cleared");

        let stopped = stop_task.await.expect("join stop task");
        assert!(stopped.is_ok(), "stop must still succeed: {stopped:?}");
    }

    /// The fast-lane routing rule: a non-empty recipe on an in-place binding
    /// forces the full pipeline (so the recipe runs), while a plain in-place
    /// dictation (no recipe, default `full_pipeline = false`) keeps the fast
    /// lane. Normal recordings are never fast-laned, and the explicit
    /// `full_pipeline = true` opt-in already leaves the fast lane.
    #[test]
    fn wants_fast_lane_routes_recipe_in_place_to_the_full_pipeline() {
        // Plain in-place dictation, no recipe → fast lane.
        assert!(wants_fast_lane(true, false, false));
        // In-place dictation carrying a recipe → full pipeline: the fast lane
        // never runs `pipeline::run`, so a recipe must not be fast-laned. This is
        // the exact case per-app tone produces — a recipe seeded into
        // `pending_recipe` at record start (by app, not by binding) sets
        // `has_recipe`, so a matched-app dictation auto-routes off the fast lane
        // with no extra wiring.
        assert!(!wants_fast_lane(true, false, true));
        // Explicit `full_pipeline` opt-in always leaves the fast lane.
        assert!(!wants_fast_lane(true, true, false));
        assert!(!wants_fast_lane(true, true, true));
        // A normal (non-in-place) recording is never fast-laned.
        assert!(!wants_fast_lane(false, false, false));
        assert!(!wants_fast_lane(false, false, true));
    }

    /// The type-first gate: a "type the quick text now" pass fires only for an
    /// in-place dictation with `type_first` set and no recipe. A recipe-bearing
    /// in-place recording is excluded — it gets its single insertion (the
    /// recipe's result) at the end of the pipeline instead, so a type-first pass
    /// here would land the text twice (or land the raw text instead of the recipe
    /// output). This is the exact inverse of the condition
    /// `pipeline::pipeline_should_type` suppresses on, so the text lands once on
    /// every in-place path.
    #[test]
    fn wants_type_first_excludes_recipe_bearing_in_place() {
        // Plain in-place dictation with type_first, no recipe → type-first fires.
        assert!(wants_type_first(true, true, false));
        // Recipe-bearing in-place: no type-first regardless of the flag — the
        // pipeline owns the sole insertion of the recipe's result. Without this
        // guard these two states double-type, or type the raw text instead of
        // the recipe output.
        assert!(!wants_type_first(true, true, true));
        // type_first off → never a type-first pass.
        assert!(!wants_type_first(true, false, false));
        assert!(!wants_type_first(true, false, true));
        // A normal (non-in-place) recording never type-firsts.
        assert!(!wants_type_first(false, true, false));
    }

    /// Ledger-leak guard: a genuine fast-lane in-place recording never enters
    /// `pipeline::run` — the sole place the per-recording ledgers are claimed —
    /// so `stop()` itself must claim and remove them. A binding that carried only
    /// a Whisper-model override (no recipe → still the fast lane) must leave no
    /// `pending_overrides` / `pending_recipe` entry keyed by its (now-dead) id
    /// once `stop()` returns. The detached fast-lane transcription that `stop()`
    /// spawns is irrelevant here: the claim is synchronous, before the spawn.
    #[tokio::test]
    async fn fast_lane_in_place_leaves_no_pending_ledger_entry() {
        let _backend = EnvVarGuard::set("PHONEME_AUDIO_BACKEND", "synthetic");
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // Start an in-place synthetic recording (default `[in_place].full_pipeline
        // = false`, no recipe → the fast lane).
        let id = state
            .recorder
            .start(&state, RecordMode::Hold, true, None)
            .await
            .expect("start synthetic in-place recording");

        // The binding carried a per-recording Whisper-model override only — the
        // exact case that must not leak (a recipe would force the full pipeline).
        state
            .pending_overrides
            .lock()
            .unwrap()
            .insert(id.clone(), "hotkey-stt".into());

        let stopped = state.recorder.stop(&state).await;
        assert!(stopped.is_ok(), "in-place stop must succeed: {stopped:?}");

        // The fast lane claimed-and-removed both ledgers in `stop()` (before the
        // detached transcription was even spawned) — nothing leaks for the id.
        assert!(
            state.pending_overrides.lock().unwrap().get(&id).is_none(),
            "a fast-lane in-place recording must not leave a pending model override"
        );
        assert!(
            state.pending_recipe.lock().unwrap().get(&id).is_none(),
            "a fast-lane in-place recording must not leave a pending recipe entry"
        );
    }

    /// Ledger-leak guard, cancel path: a custom-hotkey recording canceled
    /// mid-capture never reaches `pipeline::run` — the sole place the
    /// per-recording ledgers are otherwise claimed — so `cancel()` itself must
    /// drop them. After a cancel, no `pending_overrides` / `pending_recipe` entry
    /// keyed by the (now-deleted) id may survive. Mirrors
    /// `fast_lane_in_place_leaves_no_pending_ledger_entry` for the cancel arm.
    #[tokio::test]
    async fn cancel_leaves_no_pending_ledger_entry() {
        let _backend = EnvVarGuard::set("PHONEME_AUDIO_BACKEND", "synthetic");
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // Start a normal synthetic recording, then stash the per-recording
        // ledgers a custom-hotkey start would have set (recipe + Whisper model).
        let id = state
            .recorder
            .start(&state, RecordMode::Hold, false, None)
            .await
            .expect("start synthetic recording");
        state
            .pending_overrides
            .lock()
            .unwrap()
            .insert(id.clone(), "hotkey-stt".into());
        state
            .pending_recipe
            .lock()
            .unwrap()
            .insert(id.clone(), "hotkey-recipe".into());

        // Cancel mid-capture — this path never enters pipeline::run.
        let cancelled = state.recorder.cancel(&state).await;
        assert!(cancelled.is_ok(), "cancel must succeed: {cancelled:?}");

        // cancel() claimed-and-removed both ledgers for the dead id.
        assert!(
            state.pending_overrides.lock().unwrap().get(&id).is_none(),
            "a canceled recording must not leave a pending model override"
        );
        assert!(
            state.pending_recipe.lock().unwrap().get(&id).is_none(),
            "a canceled recording must not leave a pending recipe entry"
        );
    }

    /// Per-app tone, default path: with the default empty `[in_place].app_recipes`,
    /// starting an in-place dictation must seed NO recipe — `resolve_app_recipe`
    /// returns `None` for any foreground app, so `pending_recipe` stays empty and
    /// the dictation keeps the fast lane exactly as before. Deterministic
    /// regardless of which window is focused on the test box.
    #[tokio::test]
    async fn start_with_no_app_recipes_seeds_no_recipe() {
        let _backend = EnvVarGuard::set("PHONEME_AUDIO_BACKEND", "synthetic");
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        assert!(
            state.config.load().in_place.app_recipes.is_empty(),
            "test precondition: default config has no per-app recipes"
        );

        let id = state
            .recorder
            .start(&state, RecordMode::Hold, true, None)
            .await
            .expect("start synthetic in-place recording");

        assert!(
            state.pending_recipe.lock().unwrap().get(&id).is_none(),
            "an empty app_recipes map must seed no recipe (today's behavior)"
        );

        // Don't leave a recording in flight for the temp-dir teardown.
        let _ = state.recorder.cancel(&state).await;
    }

    /// Per-app tone, match path: when `[in_place].app_recipes` names a recipe for
    /// the FOCUSED app, starting an in-place dictation seeds that recipe into the
    /// `pending_recipe` ledger at record start — the same ledger a custom-hotkey
    /// recipe uses, so `has_recipe` then routes the dictation off the fast lane and
    /// `pipeline::run` resolves the tone. The map is keyed to whatever window is
    /// actually focused (the daemon resolves against the live `foreground_app()`),
    /// so on a headless CI box where no app is detectable this asserts the
    /// no-detectable-app fallback (seed nothing) instead — both are correct.
    #[tokio::test]
    async fn start_seeds_per_app_recipe_for_the_focused_app() {
        let _backend = EnvVarGuard::set("PHONEME_AUDIO_BACKEND", "synthetic");
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // Key the per-app map to the live foreground stem so the daemon's
        // resolution actually matches. `None` (headless CI / non-Windows) means no
        // app to key on — fall through to the no-detectable-app assertion below.
        let focused = phoneme_core::foreground::foreground_app().map(|a| a.exe_name);
        let mut cfg = (*state.config.load_full()).clone();
        if let Some(ref app) = focused {
            cfg.in_place
                .app_recipes
                .insert(app.clone(), "formal_email".into());
        }
        state.config.store(std::sync::Arc::new(cfg));

        let id = state
            .recorder
            .start(&state, RecordMode::Hold, true, None)
            .await
            .expect("start synthetic in-place recording");

        let seeded = state.pending_recipe.lock().unwrap().get(&id).cloned();
        match focused {
            Some(_) => assert_eq!(
                seeded.as_deref(),
                Some("formal_email"),
                "a per-app recipe for the focused app must be seeded at record start"
            ),
            None => assert!(
                seeded.is_none(),
                "no detectable foreground app must seed no recipe"
            ),
        }

        let _ = state.recorder.cancel(&state).await;
    }
}
