//! Meeting Mode (v1.6) machinery for the daemon recorder.
//!
//! A meeting records the microphone and the system audio (WASAPI loopback)
//! concurrently as two separate, linked recordings that share a `meeting_id`.
//! Both tracks are wall-clock aligned on stop; a partial start failure aborts
//! cleanly, and a partial stop failure still finalizes the healthy track.
//!
//! This module owns the in-flight meeting state (`ActiveMeeting`,
//! `MeetingTrackHandle`), the timeline bookkeeping (`total_paused_ms`,
//! `paused_before_ms`, `FinalizedTrack`), and the lifecycle methods
//! (`toggle_meeting`, `start_meeting`, `start_meeting_with_sources`,
//! `stop_meeting`, `finalize_meeting_track`). The single-recording path lives in
//! the parent module and is deliberately kept separate so it never has to reason
//! about a meeting.

use super::{DaemonRecorder, MeetingTrackHandle, STOP_TAIL_GRACE};
use crate::app_state::AppState;
use chrono::Local;
use phoneme_audio::device::resolve_input_device;
use phoneme_audio::meeting_align::{align_meeting_tracks, TrackAlignInput};
use phoneme_audio::recorder::{Recorder, RecorderConfig};
use phoneme_audio::source::{CpalSource, Source};
use phoneme_core::config::CaptureSource;
use phoneme_core::error::{Error, Result};
use phoneme_core::{
    HookMetadata, HookPayload, MeetingTrack, RecordMode, Recording, RecordingId, RecordingStatus,
};
use phoneme_ipc::DaemonEvent;
use std::time::Instant;

/// An in-flight meeting: the two concurrently-recording tracks (mic + system).
/// Both share `meeting_id`; stopping the meeting finalizes both together.
pub(crate) struct ActiveMeeting {
    pub(super) meeting_id: String,
    pub(super) tracks: Vec<MeetingTrackHandle>,
    pub(super) paused: bool,
    /// Wall-clock instant when the meeting session began (before per-track setup).
    pub(super) wall_started: Instant,
    /// When currently paused, the wall-offset (ms since `wall_started`) the pause
    /// began; `None` while running. Set on pause, cleared (into `pause_spans_ms`)
    /// on resume.
    pub(super) paused_at_ms: Option<i64>,
    /// Completed pause spans as `[start_ms, end_ms]` wall-offsets. Every track
    /// discards audio while paused, so its captured timeline is compressed by
    /// these spans; `stop_meeting` folds them out of the wall clock (duration and
    /// each track's first-content offset) so both tracks align on the same
    /// active-only clock instead of desyncing by the pause length.
    pub(super) pause_spans_ms: Vec<(i64, i64)>,
}

/// Total paused time (ms) across a meeting's completed pause spans. Pure helper
/// for the active-clock conversion in `stop_meeting`.
fn total_paused_ms(spans: &[(i64, i64)]) -> i64 {
    spans.iter().map(|(s, e)| (e - s).max(0)).sum()
}

/// Paused time (ms) that elapsed strictly before wall-offset `t_ms` — the spans
/// fully before it. First content is only ever stamped while running, so a span
/// never straddles `t_ms`; this is what shifts a track's first-content anchor
/// onto the active clock. Pure helper.
fn paused_before_ms(spans: &[(i64, i64)], t_ms: i64) -> i64 {
    spans
        .iter()
        .filter(|(_, e)| *e <= t_ms)
        .map(|(s, e)| (e - s).max(0))
        .sum()
}

/// One meeting track that stopped cleanly and has been aligned to the shared
/// wall-clock timeline — everything [`DaemonRecorder::finalize_meeting_track`]
/// needs to write its WAV and hand it to the pipeline.
struct FinalizedTrack {
    id: RecordingId,
    audio_path: std::path::PathBuf,
    started_at: chrono::DateTime<Local>,
    track: MeetingTrack,
    samples: Vec<i16>,
    duration_ms: i64,
}

impl DaemonRecorder {
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

    /// Start Meeting Mode (v1.6): record the microphone and the system audio
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
                tags: vec![],
                entities: vec![],
                tasks: vec![],
                speaker_names: vec![],
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
                track: Some(track.as_str().to_string()),
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

        // Per-track preview sources, captured before `tracks` is moved into
        // `ActiveMeeting`. These power both meeting-preview modes
        // (`recording.meeting_preview`):
        //  * "toggle" (default) — one loop follows a single track; the overlay's
        //    🎤/🔊 button switches it via SetPreviewSource. That's why we keep
        //    every track's snapshot handle, not just the one we start on.
        //  * "both" — one loop per track, captions shown stacked.
        let sources: Vec<(RecordingId, String, phoneme_audio::recorder::SnapshotHandle)> = tracks
            .iter()
            .map(|t| {
                (
                    t.id.clone(),
                    t.track.as_str().to_string(),
                    t.recorder.snapshot_handle(),
                )
            })
            .collect();

        *meeting_lock = Some(ActiveMeeting {
            meeting_id: meeting_id.clone(),
            tracks,
            paused: false,
            wall_started,
            paused_at_ms: None,
            pause_spans_ms: Vec::new(),
        });
        // Release the meeting lock before spawning the preview loops (they don't
        // touch `meeting`, but keep lock scopes tight).
        drop(meeting_lock);

        // Spawn the live streaming-preview loop(s) for the meeting. No-op unless
        // `recording.streaming_preview` is enabled (default: off), so meetings
        // get the same opt-in live caption single recordings do.
        let mode = state.config.load().recording.meeting_preview.clone();
        *self.meeting_preview_sources.lock().await = sources.clone();
        // The cheap audio-level waveform ("it hears me") follows one track for the
        // whole meeting: the mic (the voice the user watches), else the first
        // track. It's independent of which caption track is shown and never
        // touches whisper, so a single loop is enough. Gated on `preview_waveform`
        // inside start_level_loop; pushed into `self.preview` so stop_meeting's
        // stop_preview() tears it down with the caption loops.
        if let Some((id, _, snapshot)) = sources
            .iter()
            .find(|(_, t, _)| t == "mic")
            .or_else(|| sources.first())
            .cloned()
        {
            self.start_level_loop(state, id, snapshot).await;
        }
        if mode == "both" {
            // One caption loop per track. When the user opted into the second
            // preview server (`second_preview_needs_own_server`), the first track
            // runs on the primary preview server (yielding to final on
            // `whisper_sem`) and the second on the dedicated 2nd server (its own
            // `preview2_sem`), so the two stream concurrently. Without the opt-in
            // (or its preconditions), every loop stays on the primary server and
            // they alternate on the shared permit.
            let dual = state.config.load().second_preview_needs_own_server();
            // The dedicated 2nd preview server only parallelizes two tracks: track 0
            // (primary, on `whisper_sem`) vs the rest (the single 2nd server + its
            // one `preview2_sem`). With 3+ tracks every track past 0 shares that one
            // permit/server and alternates among themselves — fine for the only
            // production layout (2-track mic + system), but an N-track meeting would
            // need a pool of preview-N servers keyed by index. Assert the invariant
            // so a future N-track caller trips here instead of silently serializing.
            debug_assert!(
                sources.len() <= 2 || !dual,
                "2nd preview server parallelizes only 2 tracks; 3+ secondary loops would share one preview2_sem"
            );
            for (idx, (id, _, snapshot)) in sources.into_iter().enumerate() {
                let secondary = dual && idx > 0;
                self.start_preview(state, id, snapshot, secondary, false)
                    .await;
            }
        } else {
            // "toggle": start on the mic (the dense local voice the user is
            // watching the caption for); the system track is reachable via the
            // overlay's source toggle. Falls back to the first track.
            let start = sources
                .iter()
                .find(|(_, t, _)| t == "mic")
                .or_else(|| sources.first())
                .cloned();
            if let Some((id, track, snapshot)) = start {
                self.start_preview(state, id, snapshot, false, false).await;
                state
                    .events
                    .emit(DaemonEvent::PreviewSourceChanged { track });
            }
        }

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
        // Stop the live-preview loop (if any) before finalizing the tracks, so it
        // isn't mid-snapshot when the mic recorder is consumed. No-op when no
        // preview is running (preview disabled, or this build started before the
        // meeting-preview wiring). Mirrors the single-recording `stop`.
        self.stop_preview(false).await;
        self.meeting_preview_sources.lock().await.clear();
        let meeting_id = meeting.meeting_id.clone();
        let wall_started = meeting.wall_started;
        // Snapshot meeting wall-clock length before stopping recorders (stop/drain can take time).
        let stop_at = Instant::now();
        let target_duration_ms = stop_at.duration_since(wall_started).as_millis() as i64;
        let sample_rate = phoneme_audio::format::SampleRate::HZ_16K.as_u32();

        // Fold paused spans out of the wall clock. While paused, every track
        // discards audio, so the captured buffers are compressed by the paused
        // total. Align and store against this active-only duration (and shift
        // each track's first-content anchor below by the pause time before it),
        // otherwise the loopback track lands a full pause-length late vs the mic.
        let mut pause_spans_ms = meeting.pause_spans_ms.clone();
        if let Some(start_ms) = meeting.paused_at_ms {
            // Stopped while still paused — close the open span at stop time.
            pause_spans_ms.push((start_ms, target_duration_ms));
        }
        let total_paused_ms = total_paused_ms(&pause_spans_ms);
        let active_duration_ms = (target_duration_ms - total_paused_ms).max(0);

        tracing::info!(
            target_duration_ms = target_duration_ms,
            active_duration_ms = active_duration_ms,
            total_paused_ms = total_paused_ms,
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

        // Every track the meeting had, including ones that fail below. Only
        // when none of them reaches the pipeline does stop_meeting error.
        let track_total = stop_results.len();
        for (id, audio_path, started_at, track, track_late_by_ms, stop_result) in stop_results {
            match stop_result {
                Ok((raw_samples, _duration_ms, first_non_silent_at)) => {
                    // Anchor first content on the active clock too: remove any
                    // paused time that elapsed before it (a track's buffer has no
                    // paused audio, so its first sample sits at the active offset).
                    let first_content_from_wall_ms = first_non_silent_at.map(|t| {
                        let wall_ms = t.duration_since(wall_started).as_millis() as i64;
                        (wall_ms - paused_before_ms(&pause_spans_ms, wall_ms)).max(0)
                    });
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
        let aligned = align_meeting_tracks(&align_inputs, active_duration_ms, sample_rate);

        let mut track_data: Vec<FinalizedTrack> = Vec::new();

        for (meta, aligned_track) in stopped.into_iter().zip(aligned) {
            let capture_window_ms = (active_duration_ms - meta.track_late_by_ms).max(0);
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
                duration_ms: active_duration_ms,
            });
        }

        // Finalize every track independently. One track's failure must not
        // abandon its siblings mid-loop — the other track is a complete,
        // healthy recording that deserves to reach the pipeline. A failed
        // track takes the normal failure path (TranscribeFailed, visible in
        // the library) and the rest proceed; only when every track of the
        // meeting failed does stop_meeting itself report an error.
        let mut finalized = 0usize;
        for track in track_data {
            let (id, track_label) = (track.id.clone(), track.track);
            match Self::finalize_meeting_track(state, &meeting_id, track).await {
                Ok(()) => finalized += 1,
                Err(e) => {
                    tracing::error!(
                        id = %id,
                        track = track_label.as_str(),
                        error = %e,
                        "meeting track finalize failed; continuing with the remaining tracks"
                    );
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

        tracing::info!(session = %meeting_id, "meeting stopped");

        // Resume idle pre-capture now the meeting released the microphone.
        // No-op when pre-roll is disabled. Runs even when every track failed,
        // so a fully-failed stop still restores the idle state.
        self.ensure_preroll(state).await;

        if track_total > 0 && finalized == 0 {
            return Err(Error::Internal(format!(
                "meeting {meeting_id}: every track failed to finalize — see the daemon log"
            )));
        }
        Ok(meeting_id)
    }

    /// Finalize one cleanly-stopped meeting track: write its aligned samples
    /// to WAV, flip the catalog row to `Transcribing` with the shared
    /// wall-clock duration, enqueue it for the normal pipeline, and emit
    /// `RecordingStopped`. Any step failing aborts this track alone — the
    /// caller (`stop_meeting`) isolates tracks from each other and routes a
    /// failure to the normal TranscribeFailed path.
    async fn finalize_meeting_track(
        state: &AppState,
        meeting_id: &str,
        track: FinalizedTrack,
    ) -> Result<()> {
        let FinalizedTrack {
            id,
            audio_path,
            started_at,
            track,
            samples,
            duration_ms: final_duration_ms,
        } = track;

        // Write the timeline-aligned samples to WAV. Peak-normalize first when
        // enabled (off by default), matching the single-recording path: each
        // meeting track is transcribed independently, so per-track normalization
        // hands every speaker's track a healthy signal without affecting the
        // others' relative levels.
        let audio_cfg = phoneme_audio::format::AudioConfig::phoneme_default();
        let mut samples = samples;
        let snap = state.config.load();
        if snap.recording.normalize {
            phoneme_audio::normalize_peak(&mut samples, snap.recording.normalize_target_dbfs);
        }
        // Write on a blocking thread — at 16 kHz mono i16 a 60-minute meeting
        // track is ~115 MB; writing synchronously on the async executor stalls
        // other tasks for hundreds of milliseconds. Consistent with the preview
        // WAV path which already uses spawn_blocking.
        let audio_path_write = audio_path.clone();
        tokio::task::spawn_blocking(move || {
            phoneme_audio::wav::write_wav(&audio_path_write, &samples, audio_cfg)
        })
        .await
        .map_err(|e| Error::Internal(format!("spawn_blocking for WAV write panicked: {e}")))??;

        // Update catalog with the (possibly padded) duration. A meeting track
        // always rides the serial queue, so it starts Queued; the pipeline
        // flips it to Transcribing when the worker claims it.
        state
            .catalog
            .update_status_and_duration(&id, RecordingStatus::Queued, final_duration_ms)
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
            meeting_id: Some(meeting_id.to_string()),
        });
        tracing::info!(id = %id, track = track.as_str(), ms = final_duration_ms, "meeting track stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::tests::test_state;
    use crate::recorder::ActiveRecording;
    use phoneme_audio::format::AudioConfig;
    use phoneme_audio::source::{GeneratorSource, SyntheticSource};
    use phoneme_core::ListFilter;
    use std::time::Duration;

    // ── meeting pause → active-clock folding ──────────────────────────────

    #[test]
    fn paused_totals_and_active_duration() {
        // No pauses: active == wall, no shift.
        assert_eq!(total_paused_ms(&[]), 0);
        assert_eq!(paused_before_ms(&[], 5_000), 0);

        // Two pause spans (1s and 2s); a 30s meeting → 27s active.
        let spans = [(2_000, 3_000), (10_000, 12_000)];
        assert_eq!(total_paused_ms(&spans), 3_000);
        let wall = 30_000;
        assert_eq!((wall - total_paused_ms(&spans)).max(0), 27_000);
    }

    #[test]
    fn paused_before_only_counts_earlier_spans() {
        let spans = [(2_000, 3_000), (10_000, 12_000)];
        // First content at 1s — before any pause: no shift.
        assert_eq!(paused_before_ms(&spans, 1_000), 0);
        // First content at 5s — after the first (1s) pause only.
        assert_eq!(paused_before_ms(&spans, 5_000), 1_000);
        // First content at 20s — after both pauses (1s + 2s).
        assert_eq!(paused_before_ms(&spans, 20_000), 3_000);
        // The loopback's wall first-content of 5s folds to 4s on the active clock,
        // matching a mic whose buffer (also pause-compressed) starts at 0.
        assert_eq!(5_000 - paused_before_ms(&spans, 5_000), 4_000);
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

        // Both were enqueued (status flipped to Queued; the pipeline worker
        // flips each to Transcribing when it claims the item).
        for r in &meeting_rows {
            assert_eq!(
                r.status,
                RecordingStatus::Queued,
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
            .start(&state, RecordMode::Hold, false, None)
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
            focused_app: None,
            focused_window_title: None,
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
            tags: vec![],
            entities: vec![],
            tasks: vec![],
            speaker_names: vec![],
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

    /// The two synthetic sinks feeding a test meeting's mic + system tracks.
    type TwoSinks = (
        phoneme_audio::source::SyntheticSink,
        phoneme_audio::source::SyntheticSink,
    );

    /// Start a two-track synthetic meeting and return `(meeting_id, sinks)`
    /// ready for the stop-path tests.
    async fn start_two_track_meeting(state: &AppState) -> (String, TwoSinks) {
        let audio_cfg = AudioConfig::phoneme_default();
        let (mic_src, mic_sink) = SyntheticSource::new(audio_cfg);
        let (sys_src, sys_sink) = SyntheticSource::new(audio_cfg);
        let meeting_id = state
            .recorder
            .start_meeting_with_sources(
                state,
                vec![
                    (MeetingTrack::Mic, Box::new(mic_src)),
                    (MeetingTrack::System, Box::new(sys_src)),
                ],
            )
            .await
            .expect("start meeting");
        (meeting_id, (mic_sink, sys_sink))
    }

    /// Block a WAV write at `path` by occupying the destination with a
    /// directory: `write_wav`'s tmp-then-replace cannot remove a directory, so
    /// finalizing that track fails while every other path stays healthy.
    fn block_wav_path(path: &str) {
        std::fs::create_dir_all(path).expect("create blocking directory");
    }

    #[tokio::test]
    async fn stop_meeting_partial_failure_keeps_healthy_track() {
        // One track failing to finalize must not abandon the other.
        // The system track's WAV write is sabotaged; the mic track must still
        // be written, flipped to Transcribing, and enqueued — and stop_meeting
        // reports success for the partial result.
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        let (meeting_id, (mic_sink, sys_sink)) = start_two_track_meeting(&state).await;

        let rows = state.catalog.list(&ListFilter::default()).await.unwrap();
        let sys_path = rows
            .iter()
            .find(|r| r.track.as_deref() == Some("system"))
            .expect("system track row")
            .audio_path
            .clone();
        block_wav_path(&sys_path);

        mic_sink.push(vec![100i16; 8_000]).await.unwrap();
        sys_sink.push(vec![200i16; 8_000]).await.unwrap();
        mic_sink.close();
        sys_sink.close();

        state
            .recorder
            .stop_meeting(&state)
            .await
            .expect("a partial failure is still a successful stop");

        let rows = state.catalog.list(&ListFilter::default()).await.unwrap();
        let by_track = |t: &str| {
            rows.iter()
                .find(|r| {
                    r.meeting_id.as_deref() == Some(meeting_id.as_str())
                        && r.track.as_deref() == Some(t)
                })
                .unwrap_or_else(|| panic!("missing {t} track row"))
        };
        let mic = by_track("mic");
        assert_eq!(
            mic.status,
            RecordingStatus::Queued,
            "the healthy track must still reach the pipeline"
        );
        assert!(
            std::path::Path::new(&mic.audio_path).is_file(),
            "the healthy track's WAV must be written"
        );
        assert_eq!(
            by_track("system").status,
            RecordingStatus::TranscribeFailed,
            "the failed track takes the normal failure path"
        );
    }

    #[tokio::test]
    async fn stop_meeting_errors_only_when_every_track_fails() {
        // The flip side: when no track reaches the pipeline the stop must
        // surface an error (the caller would otherwise report a clean stop for
        // a meeting that produced nothing), and the meeting state must still be
        // fully cleared.
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        let (meeting_id, (mic_sink, sys_sink)) = start_two_track_meeting(&state).await;

        let rows = state.catalog.list(&ListFilter::default()).await.unwrap();
        for r in rows
            .iter()
            .filter(|r| r.meeting_id.as_deref() == Some(meeting_id.as_str()))
        {
            block_wav_path(&r.audio_path);
        }

        mic_sink.push(vec![100i16; 8_000]).await.unwrap();
        sys_sink.push(vec![200i16; 8_000]).await.unwrap();
        mic_sink.close();
        sys_sink.close();

        let err = state
            .recorder
            .stop_meeting(&state)
            .await
            .expect_err("all tracks failing must surface an error");
        assert!(matches!(err, Error::Internal(_)), "got {err:?}");
        assert!(
            !state.recorder.meeting_active().await,
            "a fully-failed stop must still clear the meeting"
        );

        let rows = state.catalog.list(&ListFilter::default()).await.unwrap();
        for r in rows
            .iter()
            .filter(|r| r.meeting_id.as_deref() == Some(meeting_id.as_str()))
        {
            assert_eq!(
                r.status,
                RecordingStatus::TranscribeFailed,
                "every track must land on the failure path"
            );
        }
    }
}
