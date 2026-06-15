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
//!   transcription. The stitcher below keeps the displayed caption
//!   forward-growing as the window slides.
//! - **Meetings** — both tracks record concurrently, share a `meeting_id`,
//!   and are wall-clock aligned on stop; a partial start failure aborts
//!   cleanly, and a partial stop failure still finalizes the healthy track.

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

/// Adaptive-cadence ceiling: even when ticks keep overrunning (a heavy model on
/// a weak box), never wait longer than this between preview transcriptions so the
/// caption still advances. The floor is the per-tick base interval.
const PREVIEW_INTERVAL_CEIL: Duration = Duration::from_millis(8000);

/// Pick the wait before the next preview tick. With `adaptive`, never schedule
/// faster than the last tick actually took — so a slow box/model self-throttles
/// instead of piling onto the single serial whisper-server and thrashing the
/// machine (the live-preview record-time crash) — clamped to `[base, ceil]`.
/// Without it, always `base` (the historical fixed cadence).
fn next_preview_interval(
    base: Duration,
    last_cost: Duration,
    ceil: Duration,
    adaptive: bool,
) -> Duration {
    if !adaptive {
        return base;
    }
    last_cost.clamp(base, ceil)
}

/// Normalized 0.0..=1.0 loudness of a sample block for the overlay's waveform
/// pill: RMS over the `i16` samples scaled by full scale, then a `sqrt` curve so
/// ordinary speech still visibly moves the bars (linear RMS sits very low for
/// voice). Empty input is silence.
fn rms_level_01(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let rms = (sum_sq / samples.len() as f64).sqrt() / 32768.0;
    (rms.sqrt() as f32).clamp(0.0, 1.0)
}

/// Stitch a freshly-transcribed trailing window onto the preview text already
/// shown, producing a stable, forward-growing caption instead of a text that
/// "rewinds" every time the rolling audio window slides.
///
/// Why this exists: the preview transcribes only the last ~15 s of audio each
/// tick (`PREVIEW_WINDOW_SAMPLES`) to keep per-tick cost constant. While the
/// recording is shorter than that window, each transcription covers the whole
/// take and grows monotonically — fine to show directly. But once the window
/// starts sliding, each transcription begins partway through the speech, so
/// naively replacing the displayed text makes its *start* jump around (the most
/// visible jank). We instead treat the previously-shown text as a committed
/// prefix and append only the genuinely-new tail of the latest window, found by
/// the longest overlap between the committed text's suffix and the window's
/// prefix. Word-boundary matching (not raw chars) keeps whisper's minor
/// re-tokenization from defeating the overlap.
///
/// `committed` is the full text shown so far; `window` is the latest window
/// transcription. Returns the new full text to display. This is a pure function
/// so it can be unit-tested without any audio/whisper round trip.
///
/// The window is always the freshest source of truth for the speech it covers,
/// so we *anchor on it*: find where the window re-states the committed tail, keep
/// the committed prefix older than that anchor, and append only the window words
/// past the overlap. We never blindly re-append the whole window onto `committed`
/// — that was the duplication bug. Whisper re-tokenizes/revises the window's
/// leading words between ticks, so an overlap pinned to the very first window
/// word often failed to match and ~15 s of already-shown words got re-appended,
/// permanently (`committed` only grows). Two defenses keep that from happening:
/// we normalize words (lowercase + strip surrounding punctuation) before
/// comparing so minor revisions still match, and we let the overlap start a few
/// words *into* the window (`MAX_LEADING_SKIP`) so a revised/inserted leading
/// word doesn't defeat the match. Each tick's tail is therefore sourced once,
/// from the newest transcription, so a word run can never duplicate.
fn stitch_preview(committed: &str, window: &str) -> String {
    let window = window.trim();
    if window.is_empty() {
        return committed.to_string();
    }
    if committed.is_empty() {
        return window.to_string();
    }

    let committed_words: Vec<&str> = committed.split_whitespace().collect();
    let window_words: Vec<&str> = window.split_whitespace().collect();

    // Normalize for comparison: lowercase + strip surrounding ASCII punctuation,
    // so whisper's minor revisions across ticks (casing drift, a trailing comma
    // appearing/disappearing) don't defeat the overlap. We compare on these but
    // emit the original words so punctuation/casing still shows.
    let norm = |w: &str| {
        w.trim_matches(|c: char| c.is_ascii_punctuation())
            .to_ascii_lowercase()
    };
    let committed_norm: Vec<String> = committed_words.iter().map(|w| norm(w)).collect();
    let window_norm: Vec<String> = window_words.iter().map(|w| norm(w)).collect();

    // Total containment: the window is already a suffix of committed (nothing
    // new revised in this slide). Leave the caption untouched so we never grow
    // it with a re-statement of words already shown.
    if window_norm.len() <= committed_norm.len()
        && committed_norm[committed_norm.len() - window_norm.len()..] == window_norm[..]
    {
        return committed.to_string();
    }

    // How many leading window words we'll allow to be skipped when anchoring: a
    // revised/inserted leading word ("um", a re-cased first word) shouldn't pin
    // the overlap to window[0] and force a blind append.
    const MAX_LEADING_SKIP: usize = 8;
    // Cap the overlap search so this stays cheap even on long captions.
    const MAX_OVERLAP: usize = 64;

    // Find the best anchor: the longest run of trailing committed words that
    // equals a window run starting at some small offset `skip` from the window's
    // head. The committed prefix older than that run is kept; the window past the
    // overlap supplies the genuinely-new words. Prefer the longest overlap, and
    // among equal overlaps the smallest skip (closest to a clean head match).
    let mut best: Option<(usize /*overlap*/, usize /*skip*/)> = None;
    let max_skip = MAX_LEADING_SKIP.min(window_norm.len().saturating_sub(1));
    for skip in 0..=max_skip {
        let max_overlap = committed_norm
            .len()
            .min(window_norm.len() - skip)
            .min(MAX_OVERLAP);
        for overlap in (1..=max_overlap).rev() {
            let tail = &committed_norm[committed_norm.len() - overlap..];
            let head = &window_norm[skip..skip + overlap];
            if tail == head {
                if best.map_or(true, |(bo, _)| overlap > bo) {
                    best = Some((overlap, skip));
                }
                // Longest for this skip found — no shorter one can beat it.
                break;
            }
        }
    }

    if let Some((overlap, skip)) = best {
        // Keep ALL the committed words (its copy of the overlap stays — committed
        // casing/punctuation wins at the boundary) and append ONLY the window
        // words after its overlap region. Window words before `skip` (a revised
        // leading fragment) and the overlap itself restate committed content, so
        // they are dropped — no run is ever duplicated.
        let mut out: Vec<&str> = committed_words.clone();
        out.extend_from_slice(&window_words[skip + overlap..]);
        return out.join(" ");
    }

    // No overlap found (e.g. a long silence split the speech, or whisper produced
    // a wholly different window). Append the window as a new segment so we never
    // lose newly-spoken words; a leading separator keeps it readable.
    format!("{committed} {window}")
}

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

/// One meeting track that stopped cleanly and has been aligned to the shared
/// wall-clock timeline — everything [`DaemonRecorder::finalize_meeting_track`]
/// needs to write its WAV and hand it to the pipeline.
struct FinalizedTrack {
    id: RecordingId,
    audio_path: PathBuf,
    started_at: chrono::DateTime<Local>,
    track: MeetingTrack,
    samples: Vec<i16>,
    duration_ms: i64,
}

#[derive(Clone, Default)]
pub struct DaemonRecorder {
    active: Arc<Mutex<Option<ActiveRecording>>>,
    handle: Arc<Mutex<Option<Recorder>>>,
    /// Idle pre-roll pre-capture, present only while enabled and not actively
    /// recording. `None` means no continuous capture is running (the default).
    preroll: Arc<Mutex<Option<PreRoll>>>,
    /// Streaming transcription preview loops, present only while recording with
    /// the feature enabled. Empty (the default) means no preview is running.
    /// Single recordings and meetings in "toggle" mode run one loop; meetings
    /// in "both" mode run one per track.
    preview: Arc<Mutex<Vec<PreviewTask>>>,
    /// The active meeting's preview sources — (recording id, track label,
    /// snapshot handle) per track — kept so `SetPreviewSource` can switch which
    /// track feeds the preview mid-meeting ("toggle" mode). Cleared when the
    /// meeting stops/cancels.
    meeting_preview_sources:
        Arc<Mutex<Vec<(RecordingId, String, phoneme_audio::recorder::SnapshotHandle)>>>,
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
    /// recorder (through `snapshot`) every `PREVIEW_INTERVAL`, transcribes the
    /// audio so far via the configured provider, and emits `TranscriptionPartial`.
    /// It transcribes one tick at a time (a slow transcription simply means the
    /// next tick is skipped — never two in flight), and stops when told to via
    /// `stop_tx`.
    ///
    /// `snapshot` is a `SnapshotHandle` cloned from the recorder whose audio
    /// the preview should reflect: the single recording's recorder, or a
    /// meeting's mic track. Passing the handle (rather than reaching into
    /// `self.handle`) is what lets one preview implementation serve BOTH the
    /// single-recording and the meeting path — meetings previously emitted no
    /// partials at all because their recorder lives inside `ActiveMeeting`, not
    /// `self.handle`.
    async fn start_preview(
        &self,
        state: &AppState,
        id: RecordingId,
        snapshot: phoneme_audio::recorder::SnapshotHandle,
    ) {
        if !state.config.load().recording.streaming_preview {
            return;
        }
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let state = state.clone();
        let log_id = id.clone();
        let task = tokio::spawn(async move {
            let cfg = state.config.load();
            // The live preview uses its own provider when configured
            // (`preview_whisper`) — a fast local model on a second server, or a
            // cloud API — so it never contends with the final transcription.
            // Falls back to the main provider when unset (unchanged behavior).
            // `apply` swaps in the port the bundled server actually listens on
            // (it falls back from the configured port when another app holds it).
            let preview_cfg = state
                .whisper_ports
                .apply(&cfg, cfg.preview_provider_config());
            let provider = state.transcription.provider(
                &preview_cfg,
                &phoneme_core::config::DiarizationConfig::default(),
            );
            let is_native = provider.is_native();

            // If the provider is native (running directly in our RAM), we can safely
            // drop the interval to 1000ms for real-time streaming without worrying
            // about HTTP/file-write overhead. Cloud providers get longer intervals
            // to avoid overwhelming the API.
            let base_interval = if is_native {
                std::time::Duration::from_millis(1000)
            } else {
                PREVIEW_INTERVAL
            };
            // Adaptive cadence: when a tick's transcription overruns the base
            // interval (heavy model on a weak box), wait at least that long
            // before the next one so the preview self-throttles instead of
            // piling onto the single serial whisper-server and thrashing the
            // machine. Starts at the base cadence; the first sleep also doubles
            // as the "don't transcribe near-empty audio" warm-up.
            let adaptive = cfg.recording.preview_adaptive;
            let mut current_wait = base_interval;

            let tmp_wav =
                std::env::temp_dir().join(format!("phoneme-preview-{}.wav", id.file_stem()));
            let mut last_len = 0usize;
            // The stable, forward-growing caption shown so far. While the
            // recording is shorter than the audio window each transcription is of
            // the whole take (authoritative), so we replace this wholesale; once
            // the window slides we stitch the new tail onto it (see stitch_preview).
            let mut committed = String::new();
            let audio_cfg = phoneme_audio::format::AudioConfig::phoneme_default();

            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = tokio::time::sleep(current_wait) => {}
                }

                // Snapshot only the trailing window of audio captured so far (the
                // recorder also tells us the full captured length so we can still
                // throttle on newly-accumulated audio). If the recorder is gone
                // (race with stop), `snapshot_tail` errors and we end the loop.
                let (total_len, samples) =
                    match snapshot.snapshot_tail(PREVIEW_WINDOW_SAMPLES).await {
                        Ok(s) => s,
                        Err(_) => break,
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
                let tick_start = std::time::Instant::now();
                match provider.transcribe(&tmp_wav, language.as_deref()).await {
                    Ok(text) => {
                        let text = text.trim();
                        if !text.is_empty() {
                            // While the take still fits inside the audio window the
                            // transcription covers everything from the start, so it
                            // is the authoritative full caption — show it directly.
                            // Once the window has begun sliding (longer take), the
                            // transcription only covers the tail, so stitch it onto
                            // the committed caption to keep the preview growing
                            // forward instead of rewinding its start each tick.
                            let window_slid = total_len > PREVIEW_WINDOW_SAMPLES;
                            committed = if window_slid {
                                stitch_preview(&committed, text)
                            } else {
                                text.to_string()
                            };
                            state.events.emit(DaemonEvent::TranscriptionPartial {
                                id: id.clone(),
                                text: committed.clone(),
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
                // Adapt the next wait to how long this tick actually took, so a
                // heavy model on a weak box self-throttles instead of trying to
                // run every base-interval and thrashing (the record-time crash).
                current_wait = next_preview_interval(
                    base_interval,
                    tick_start.elapsed(),
                    PREVIEW_INTERVAL_CEIL,
                    adaptive,
                );
            }

            // Clean up temp file even if loop exits early
            let _ = tokio::fs::remove_file(&tmp_wav).await;
        });

        self.preview
            .lock()
            .await
            .push(PreviewTask { stop_tx, task });
        tracing::info!(id = %log_id, "streaming transcription preview started");
    }

    /// Spawn the cheap live audio-level loop that feeds the overlay's waveform
    /// "it hears me" pill: snapshot a tiny trailing tail at ~15 Hz, compute a
    /// normalized RMS level, emit `AudioLevelSample`. It never acquires the
    /// whisper permit or transcribes, so it adds negligible load and runs for any
    /// capture (including in-place dictation). No-op unless `preview_waveform` is
    /// on. Stored in `self.preview` so `stop_preview` tears it down too.
    async fn start_level_loop(
        &self,
        state: &AppState,
        id: RecordingId,
        snapshot: phoneme_audio::recorder::SnapshotHandle,
    ) {
        if !state.config.load().recording.preview_waveform {
            return;
        }
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let state = state.clone();
        let task = tokio::spawn(async move {
            // ~100 ms tail at 16 kHz, sampled ~15×/s — lively without measurable cost.
            const LEVEL_TAIL_SAMPLES: usize = 1600;
            let mut interval = tokio::time::interval(Duration::from_millis(66));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = interval.tick() => {}
                }
                // Recorder gone (race with stop) → exit, like the preview loop.
                let (_total, samples) = match snapshot.snapshot_tail(LEVEL_TAIL_SAMPLES).await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let level = rms_level_01(&samples);
                state.events.emit(DaemonEvent::AudioLevelSample {
                    id: id.clone(),
                    level,
                });
            }
        });
        self.preview
            .lock()
            .await
            .push(PreviewTask { stop_tx, task });
    }

    /// Stop the streaming-preview loop (if running) and wait for it to exit so
    /// its temp WAV is cleaned up. No-op when no preview is running.
    ///
    /// `abort` (in-place dictation only) tears the loop down WITHOUT awaiting an
    /// in-flight tick: it signals stop and `abort()`s the join handle so the
    /// held whisper permit is released immediately and the latency-critical
    /// dictation transcribe isn't delayed. Aborting skips the loop's graceful
    /// temp-WAV cleanup, so callers that pass `abort = true` must remove the
    /// preview WAV(s) themselves (see `stop`). Normal recordings and meetings
    /// pass `abort = false` for the graceful, await-based teardown — that path
    /// lets the loop delete its own temp WAV and must stay unchanged.
    async fn stop_preview(&self, abort: bool) {
        let tasks: Vec<PreviewTask> = std::mem::take(&mut *self.preview.lock().await);
        for PreviewTask { stop_tx, task } in tasks {
            let _ = stop_tx.send(());
            if abort {
                task.abort();
            } else {
                let _ = task.await;
            }
        }
    }

    /// Switch which meeting track feeds the live preview ("toggle" mode): stop
    /// the running loop(s) and start one on the requested track's snapshot.
    /// Errors when no meeting is active or the track label doesn't exist.
    /// Emits `PreviewSourceChanged` so the overlay's toggle reflects it.
    pub async fn set_preview_source(&self, state: &AppState, track: &str) -> Result<()> {
        let entry = self
            .meeting_preview_sources
            .lock()
            .await
            .iter()
            .find(|(_, t, _)| t == track)
            .map(|(id, t, h)| (id.clone(), t.clone(), h.clone()));
        let Some((id, track, snapshot)) = entry else {
            return Err(Error::Internal(format!(
                "no active meeting track {track:?} to preview"
            )));
        };
        self.stop_preview(false).await;
        self.start_preview(state, id, snapshot).await;
        state
            .events
            .emit(DaemonEvent::PreviewSourceChanged { track });
        Ok(())
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

        // Capture the foreground app for in-place dictations, at START — this is
        // the window the user is dictating into, before the brief recording can
        // shift focus. The process stem keys the per-app type/paste/off override;
        // the window title is read ONLY when app-aware context is opted in and
        // the app isn't denylisted (privacy-first — off by default reads nothing).
        // Both Win32 calls and `config.load()` are synchronous (no await), so
        // doing this under the `active` guard is sound.
        let (focused_app, focused_window_title) = if in_place {
            let cfg = state.config.load();
            let app = phoneme_core::foreground::foreground_app();
            let exe = app.as_ref().map(|a| a.exe_name.clone());
            let title = app.filter(|a| {
                !a.window_title.is_empty()
                    && cfg.in_place.may_read_window_title(Some(a.exe_name.as_str()))
            });
            (exe, title.map(|a| a.window_title))
        } else {
            (None, None)
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
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
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
        // `recording.streaming_preview` is enabled (default: off). Runs for
        // in-place dictation too: the dictation overlay shows the same caption,
        // and the loop self-throttles around the latency-critical paste — each
        // tick only transcribes when it can `try_acquire` the single serial
        // whisper permit, so the dictation's own transcribe always wins it and
        // an in-flight tick simply skips. The dictation stop path tears this
        // preview down WITHOUT awaiting (see `stop`), so a preview tick can
        // never delay the paste ("constantly listening, never pastes").
        self.start_preview(state, id.clone(), preview_snapshot)
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
        // section, then drop both guards BEFORE any slow await. The preview
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
        // await-based teardown (which deletes its own temp WAV) — do NOT abort
        // them or their preview WAVs leak.
        self.stop_preview(active.in_place).await;
        if active.in_place {
            let preview_wav = std::env::temp_dir()
                .join(format!("phoneme-preview-{}.wav", active.id.file_stem()));
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
        let in_place_cfg = state.config.load().in_place.clone();
        let fast_lane = active.in_place && !in_place_cfg.full_pipeline;
        if fast_lane {
            crate::in_place::spawn_fast_lane(
                state.clone(),
                active.id.clone(),
                active.audio_path.clone(),
                active.focused_app.clone(),
                active.focused_window_title.clone(),
            );
        } else {
            // Full-pipeline dictation with `type_first`: the text shouldn't
            // wait for the queue, so a type-only pass types the quick
            // transcription NOW, alongside the normal enqueue below — the
            // pipeline still runs every step for the library copy, but skips
            // its own end-of-run typing so the text lands exactly once.
            if active.in_place && in_place_cfg.type_first {
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
        // What cancel found to tear down, moved OUT of the state mutexes so
        // the slow awaits below (preview teardown, recorder cancel) run with
        // no lock held — same reasoning as `stop`.
        enum Taken {
            Single(ActiveRecording, Option<Recorder>),
            Meeting(Box<ActiveMeeting>),
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
                if let Some(recorder) = recorder {
                    if let Err(e) = recorder.cancel().await {
                        tracing::warn!("failed to cancel recorder: {e}");
                    }
                }
                state.catalog.delete(&active.id).await?;
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
                user_edited: false,
                favorite: false,
                tag_suggestions: vec![],
                summary: None,
                summary_model: None,
                title: None,
                title_is_auto: true,
                title_model: None,
                tag_model: None,
                diarization_model: None,
                tags: vec![],
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
        //    🎤/🔊 button switches it via SetPreviewSource (which is why every
        //    track's snapshot handle is kept, not just the starting one).
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
        });
        // Release the meeting lock before spawning the preview loops (they don't
        // touch `meeting`, but keep lock scopes tight).
        drop(meeting_lock);

        // Spawn the live streaming-preview loop(s) for the meeting. No-op unless
        // `recording.streaming_preview` is enabled (default: off), so meetings
        // get the same opt-in live caption single recordings do.
        let mode = state.config.load().recording.meeting_preview.clone();
        *self.meeting_preview_sources.lock().await = sources.clone();
        // The cheap audio-level waveform ("it hears me") follows ONE track for the
        // whole meeting — the mic (the voice the user watches), else the first
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
            for (id, _, snapshot) in sources {
                self.start_preview(state, id, snapshot).await;
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
                self.start_preview(state, id, snapshot).await;
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

        // Every track the meeting had — including ones that fail below. Only
        // when NONE of them reaches the pipeline does stop_meeting error.
        let track_total = stop_results.len();
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

        // Finalize every track independently. One track's failure must not
        // abandon its siblings mid-loop — the other track is a complete,
        // healthy recording that deserves to reach the pipeline. A failed
        // track takes the normal failure path (TranscribeFailed, visible in
        // the library) and the rest proceed; only when EVERY track of the
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
    /// `RecordingStopped`. Any step failing aborts THIS track only — the
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
        phoneme_audio::wav::write_wav(&audio_path, &samples, audio_cfg)?;

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
    use crate::app_state::AppState;
    use phoneme_audio::format::AudioConfig;
    use phoneme_audio::source::{GeneratorSource, SyntheticSource};
    use phoneme_core::{Config, ListFilter};

    // ── stitch_preview (pure caption stitching) ───────────────────────────

    #[test]
    fn stitch_preview_appends_only_new_tail_on_overlap() {
        // The new window re-states the tail of what's shown, then adds new words.
        let committed = "the quick brown fox";
        let window = "brown fox jumps over";
        assert_eq!(
            stitch_preview(committed, window),
            "the quick brown fox jumps over"
        );
    }

    #[test]
    fn stitch_preview_no_change_when_window_fully_contained() {
        // The window is entirely a suffix of the committed text — nothing new.
        let committed = "hello world how are you";
        let window = "how are you";
        assert_eq!(stitch_preview(committed, window), committed);
    }

    #[test]
    fn stitch_preview_handles_empty_inputs() {
        assert_eq!(stitch_preview("", "hello world"), "hello world");
        assert_eq!(stitch_preview("already here", ""), "already here");
        assert_eq!(stitch_preview("", ""), "");
        // Whitespace-only window is treated as empty.
        assert_eq!(stitch_preview("keep me", "   "), "keep me");
    }

    // ── next_preview_interval (adaptive cadence — the record-time crash fix) ──

    #[test]
    fn adaptive_off_keeps_fixed_cadence() {
        let base = Duration::from_millis(1000);
        let ceil = Duration::from_millis(8000);
        // Even a long tick keeps the base cadence when adaptive is off.
        assert_eq!(
            next_preview_interval(base, Duration::from_millis(5000), ceil, false),
            base
        );
    }

    #[test]
    fn adaptive_backs_off_to_tick_cost_clamped() {
        let base = Duration::from_millis(1000);
        let ceil = Duration::from_millis(8000);
        // Fast tick → stays at the base floor.
        assert_eq!(
            next_preview_interval(base, Duration::from_millis(200), ceil, true),
            base
        );
        // Slow tick → wait at least as long as it actually took.
        assert_eq!(
            next_preview_interval(base, Duration::from_millis(3000), ceil, true),
            Duration::from_millis(3000)
        );
        // Pathologically slow tick → capped at the ceiling so the caption still advances.
        assert_eq!(
            next_preview_interval(base, Duration::from_millis(20000), ceil, true),
            ceil
        );
    }

    // ── rms_level_01 (waveform pill loudness) ─────────────────────────────

    #[test]
    fn rms_level_01_silence_and_full_scale() {
        assert_eq!(rms_level_01(&[]), 0.0);
        assert_eq!(rms_level_01(&[0, 0, 0, 0]), 0.0);
        // A full-scale square wave reads near the top of the range.
        assert!(rms_level_01(&[i16::MAX, i16::MIN, i16::MAX, i16::MIN]) > 0.99);
        // A mid-level block sits strictly between silence and full scale.
        let m = rms_level_01(&[8000, -8000, 8000, -8000]);
        assert!(m > 0.0 && m < 1.0, "mid level was {m}");
    }

    #[test]
    fn stitch_preview_appends_disjoint_window_as_new_segment() {
        // No overlap at all (e.g. a pause split the speech): never drop the new
        // words — append them so the caption still advances.
        let committed = "first sentence done";
        let window = "completely different words";
        assert_eq!(
            stitch_preview(committed, window),
            "first sentence done completely different words"
        );
    }

    #[test]
    fn stitch_preview_overlap_is_case_insensitive() {
        // Whisper may change sentence-start casing as more context arrives; the
        // overlap must still match so we don't double-print the boundary word.
        let committed = "we met at the";
        let window = "The cafe yesterday";
        assert_eq!(
            stitch_preview(committed, window),
            "we met at the cafe yesterday"
        );
    }

    #[test]
    fn stitch_preview_prefers_longest_overlap() {
        // "to be" appears twice; the longest trailing/leading overlap must win so
        // we don't re-append an already-shown run.
        let committed = "to be or not to be";
        let window = "to be that is the question";
        assert_eq!(
            stitch_preview(committed, window),
            "to be or not to be that is the question"
        );
    }

    #[test]
    fn stitch_preview_no_duplication_when_window_revises_leading_words() {
        // The regression this fix targets: between ticks whisper re-tokenizes the
        // rolling window's LEADING words (here it prepends a filler "um" and
        // re-cases "How"), so the overlap no longer pins to window[0]. The old
        // code's blind fallback re-appended the whole ~15 s window, duplicating
        // words already shown. With normalization + a small leading-skip anchor we
        // must instead append ONLY the genuinely-new tail, with no run repeated.
        let committed = "hello there my friend how are you";
        // Same speech, leading words revised, plus two new words at the end.
        let window = "um How are you doing today";
        let out = stitch_preview(committed, window);
        assert_eq!(out, "hello there my friend how are you doing today");

        // Hard guarantee against the actual symptom ("text comes up multiple
        // times in a row"): no word from the overlapping run appears twice.
        let words: Vec<&str> = out.split_whitespace().collect();
        for run in ["how are you", "are you doing"] {
            let occurrences = out.matches(run).count();
            assert_eq!(occurrences, 1, "run {run:?} duplicated in {out:?}");
        }
        // "you" (the boundary word) must appear exactly once, not re-stated.
        assert_eq!(words.iter().filter(|w| **w == "you").count(), 1, "in {out:?}");
    }

    #[test]
    fn stitch_preview_feeds_two_overlapping_windows_without_duplication() {
        // Simulate the loop: a non-sliding tick seeds `committed` with the whole
        // take, then a sliding tick revises the leading words and adds a tail.
        // The accumulated caption must read each word once, in order.
        let first_window = "the meeting will start at noon today";
        // First tick (take fits the window) replaces wholesale — modeled by
        // stitching onto an empty caption.
        let committed = stitch_preview("", first_window);
        assert_eq!(committed, first_window);

        // Second (sliding) tick: whisper re-cased "The" and dropped "the meeting",
        // re-stating from "will start" with two new trailing words.
        let second_window = "Will start at noon today in room five";
        let out = stitch_preview(&committed, second_window);
        assert_eq!(out, "the meeting will start at noon today in room five");
        assert_eq!(out.matches("at noon today").count(), 1, "duplicated in {out:?}");
    }

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
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
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
        // Audit M2: one track failing to finalize must not abandon the other.
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
        // Audit M2, the flip side: when NO track reaches the pipeline the stop
        // must surface an error (the caller would otherwise report a clean
        // stop for a meeting that produced nothing) — and the meeting state
        // must still be fully cleared.
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

    #[tokio::test]
    async fn stop_keeps_status_queries_responsive_during_preview_teardown() {
        // Audit M3: `stop` must not hold the active-recording lock across the
        // preview teardown — a slow in-flight preview tick (here a stand-in
        // task that takes 1.5 s to wind down) used to block every status and
        // control IPC for its whole duration.
        std::env::set_var("PHONEME_AUDIO_BACKEND", "synthetic");
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        state
            .recorder
            .start(&state, RecordMode::Hold, false)
            .await
            .expect("start synthetic recording");

        // Inject a preview task that ignores its stop signal for 1.5 s — the
        // shape of a preview loop stuck inside a slow transcription tick.
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = stop_rx.await;
            tokio::time::sleep(Duration::from_millis(1_500)).await;
        });
        state
            .recorder
            .preview
            .lock()
            .await
            .push(PreviewTask { stop_tx, task });

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
        std::env::remove_var("PHONEME_AUDIO_BACKEND");
    }
}
