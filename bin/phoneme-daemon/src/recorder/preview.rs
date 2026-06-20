//! Live-preview machinery for the daemon recorder.
//!
//! While a recording (or meeting) is active and the relevant feature is on, two
//! kinds of background loop run off the recorder's audio:
//! - a `TranscriptionPartial` **caption** loop (`start_preview`) that transcribes
//!   a rolling tail window and stitches a stable, forward-growing caption, and
//! - a cheap `AudioLevelSample` **waveform** loop (`start_level_loop`) for the
//!   overlay's "it hears me" pill.
//!
//! The stitcher (`stitch_preview`/`merge_preview_tick`) keeps the displayed
//! caption from reshuffling words already shown as the audio window slides; the
//! adaptive cadence (`next_preview_interval`) self-throttles a heavy tick so the
//! preview never starves the final transcription. All of this lives behind the
//! same opt-in flags as before — the default path runs no loop at all.

use super::{DaemonRecorder, PreviewKind, PreviewTask};
use crate::app_state::AppState;
use phoneme_core::error::{Error, Result};
use phoneme_core::RecordingId;
use phoneme_ipc::DaemonEvent;
use std::time::Duration;

/// Base cadence for the streaming-preview loop on a CLOUD transcription provider,
/// where each tick pays HTTP + file-write overhead and we don't want to hammer the
/// API. This is NOT the universal cadence: a native (in-process) provider drops the
/// base to 1000 ms inside `start_preview` for smoother real-time captions, since it
/// has no network round-trip. The adaptive throttle (`preview_adaptive`) can stretch
/// either base up to `PREVIEW_INTERVAL_CEIL` when a tick overruns.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(2000);

/// Minimum number of *new* samples (beyond the previous preview) before we spend
/// a transcription on a fresh tick. At 16 kHz this is ~0.5 s, so the caption
/// advances about twice as smoothly as the old ~1.0 s gate (the "chunky/laggy"
/// complaint). A tick that can't get the whisper permit still skips and a heavy
/// tick still backs off via the adaptive cadence, so weak boxes never thrash —
/// this only lets capable machines update more often.
const PREVIEW_MIN_NEW_SAMPLES: usize = 8_000;

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
/// Uppercase the first alphabetic character of `s`, leaving the rest untouched.
/// Streaming-type only: the first word is stable once committed, so this keeps the
/// live-typed text aligned with the polished final's capitalized start — making the
/// stop reconcile a minimal tail patch instead of a full rewrite. No-op when the
/// first letter is already uppercase or there is no letter.
fn capitalize_first(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut done = false;
    for c in s.chars() {
        if !done && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            done = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Returns the merged caption (committed's words kept verbatim — never rewritten
/// — plus only the genuinely-new window tail), or `None` when no overlap and no
/// containment was found. `None` is the caller's cue to pick a phase-aware
/// fallback: while the take still fits the audio window the fresh transcription
/// is authoritative (replace), once it has slid the window is a post-silence tail
/// (append). Either way committed is never rewritten mid-caption, which is what
/// keeps the live preview from visibly reshuffling words already shown.
fn stitch_preview(committed: &str, window: &str) -> Option<String> {
    let window = window.trim();
    if window.is_empty() {
        return Some(committed.to_string());
    }
    if committed.is_empty() {
        return Some(window.to_string());
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
        return Some(committed.to_string());
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
                if best.is_none_or(|(bo, _)| overlap > bo) {
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
        return Some(out.join(" "));
    }

    // No overlap and no containment (a long silence split the speech, or whisper
    // re-transcribed the window wholly differently). Ambiguous — the caller picks
    // the phase-aware fallback rather than blindly appending (which would
    // duplicate a re-transcribed tail).
    None
}

/// Fold a fresh window transcription into the committed caption for one preview
/// tick: stitch when an anchor is found, otherwise pick the phase-aware fallback
/// `stitch_preview` defers to the caller. `window_slid` is whether the rolling
/// audio window has started sliding (the take no longer fits a single window).
///
/// On a no-overlap, no-containment `None`:
/// - **slid** → the window is a post-silence tail, so append it.
/// - **not slid** → the take still fits the window, so a `None` means whisper
///   re-transcribed the whole (still-short) take differently. KEEP `committed`
///   untouched and let the next tick re-anchor, rather than replacing it wholesale
///   and reshuffling every already-shown word. This upholds the subsystem's
///   "committed never rewrites" invariant. Pulled out as a pure function so the
///   fallback branches are unit-testable without an audio/whisper round trip.
fn merge_preview_tick(committed: &str, window: &str, window_slid: bool) -> String {
    stitch_preview(committed, window).unwrap_or_else(|| {
        if window_slid {
            format!("{committed} {window}")
        } else {
            committed.to_string()
        }
    })
}

/// The committed (stable) prefix length to attach to a `TranscriptionPartial`:
/// the char length of the caption shown BEFORE this tick's append, clamped to the
/// merged caption's length. Everything in `merged` past this offset is this tick's
/// freshly-appended, least-settled tail — the part the overlay dims as tentative.
///
/// `prev_committed` is the caption from the previous tick (empty on the first
/// emit); `merged` is the result of [`merge_preview_tick`] for this tick. Because
/// `merge_preview_tick` only ever keeps `prev_committed` verbatim and appends, the
/// prefix length is normally `prev_committed.len()` and `≤ merged.len()`; the
/// clamp is a guard so a future merge that ever shortened the caption could never
/// produce an out-of-range boundary. On the first emit `prev_committed` is empty
/// so the boundary is `0` (all fresh). When nothing new was appended the merged
/// caption equals `prev_committed`, so the boundary equals the full length and the
/// overlay dims nothing. Pulled out (not inlined) so the boundary rule is unit-
/// testable without driving an audio/whisper round trip.
fn preview_committed_len(prev_committed: &str, merged: &str) -> usize {
    prev_committed.len().min(merged.len())
}

impl DaemonRecorder {
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
    pub(super) async fn start_preview(
        &self,
        state: &AppState,
        id: RecordingId,
        snapshot: phoneme_audio::recorder::SnapshotHandle,
        secondary: bool,
        stream_type: bool,
    ) {
        // Streaming-type forces the loop on even when the visible preview caption
        // is off — it needs the loop's committed words as its live-typing source.
        if !state.config.load().recording.streaming_preview && !stream_type {
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
            //
            // `secondary` (meeting "both" mode, 2nd track) points this loop at the
            // SECOND preview server (its derived port) and gates it on the
            // independent `preview2_sem` — so the two meeting tracks transcribe
            // CONCURRENTLY instead of alternating on the shared `whisper_sem`.
            let preview_sem = if secondary {
                state.preview2_sem.clone()
            } else {
                state.whisper_sem.clone()
            };
            let mut provider_cfg = cfg.preview_provider_config().clone();
            if secondary {
                // Same preview model, the 2nd server's port — `apply` then
                // resolves it to the live port the 2nd server is listening on.
                provider_cfg.bundled_server_port = cfg.preview2_port();
            }
            let preview_cfg = state.whisper_ports.apply(&cfg, &provider_cfg);
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
            // Streaming-type only: what we've typed at the cursor so far (clean
            // extensions of the committed caption, first letter capitalized).
            let mut typed = String::new();
            let audio_cfg = phoneme_audio::format::AudioConfig::phoneme_default();

            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = tokio::time::sleep(current_wait) => {}
                }

                // Start the cadence clock here so the adaptive throttle measures the
                // TRUE per-tick wall cost it is meant to self-throttle on — snapshot +
                // WAV encode + write + transcribe — not the transcribe call alone. A
                // tick that skips (not enough new audio, or no free whisper permit)
                // `continue`s before reaching `next_preview_interval`, so it never
                // feeds this clock; only a tick that actually does the work does.
                let tick_start = std::time::Instant::now();

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
                // the permit is free *right now*. The primary loop holds
                // `whisper_sem`, so it yields to the final transcription (which
                // previously caused "Whisper timed out after 60s"); the secondary
                // "both"-mode loop holds the independent `preview2_sem`, so it runs
                // concurrently on its own server. The permit is held for the
                // duration of this tick's transcription.
                let _preview_permit = match preview_sem.try_acquire() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Write a temp WAV and transcribe via the configured provider. The
                // hound encode + std::fs syscalls are blocking, so run them on the
                // blocking pool rather than stalling an async worker every tick
                // (the preview runs ~1×/s for the whole recording). `samples` is
                // moved in; the fixed `tmp_wav` path + `audio_cfg` (Copy) are cloned.
                let wav_path = tmp_wav.clone();
                let wav_res = tokio::task::spawn_blocking(move || {
                    phoneme_audio::wav::write_wav(&wav_path, &samples, audio_cfg)
                })
                .await;
                match wav_res {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "streaming preview: failed to write temp WAV; skipping tick");
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "streaming preview: WAV encode task failed; skipping tick");
                        continue;
                    }
                }
                let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());

                // Use the cached provider to avoid re-resolving on every tick.
                // Config changes during recording will take effect on the next recording.
                match provider.transcribe(&tmp_wav, language.as_deref()).await {
                    Ok(text) => {
                        let text = text.trim();
                        if !text.is_empty() {
                            // Always stitch, so words already shown are never
                            // rewritten mid-caption — the old early-phase wholesale
                            // replace was exactly what made the preview visibly
                            // reshuffle words as whisper revised the growing take.
                            // On a no-overlap stitch, fall back by phase: once the
                            // window has slid it is a post-silence tail, so append;
                            // while the take still fits the window a no-overlap result
                            // means whisper re-transcribed the whole (still-short) take
                            // differently — KEEP committed untouched this tick and let
                            // the next tick re-anchor, rather than replacing it
                            // wholesale and reshuffling every shown word (the
                            // "committed never rewrites" invariant the subsystem is
                            // built on).
                            let window_slid = total_len > PREVIEW_WINDOW_SAMPLES;
                            // The committed (stable) prefix before this tick's append.
                            // Everything in the merged caption past this char boundary
                            // is freshly appended this tick — the least-settled tail the
                            // overlay dims. `merge_preview_tick` only ever keeps
                            // `committed` verbatim and appends, so this prefix length
                            // stays valid against the merged text; clamp anyway in case
                            // a future merge ever shortens it. When nothing new was
                            // appended (caption unchanged) this equals the full length,
                            // so the overlay dims nothing.
                            let prev_committed = std::mem::take(&mut committed);
                            committed = merge_preview_tick(&prev_committed, text, window_slid);
                            let committed_len = preview_committed_len(&prev_committed, &committed);
                            state.events.emit(DaemonEvent::TranscriptionPartial {
                                id: id.clone(),
                                text: committed.clone(),
                                committed_len: Some(committed_len),
                            });
                            // Streaming-type (`[in_place].stream_type`): type the
                            // newly-finalized words live. Only CLEAN forward
                            // extensions (`backspaces == 0`) are typed mid-stream
                            // — never a backspace — so the cursor doesn't churn as
                            // the caption revises; the stop reconcile fixes the
                            // rest against the accurate final transcript. The first
                            // letter is capitalized so that stop patch is a small
                            // tail edit, not a full rewrite vs the polished final.
                            if stream_type {
                                let target = capitalize_first(&committed);
                                let (backspaces, insert) =
                                    phoneme_core::dictation::reconcile_edit(&typed, &target);
                                if backspaces == 0
                                    && !insert.is_empty()
                                    && crate::in_place::type_at_cursor(&insert, "type")
                                        .await
                                        .is_ok()
                                {
                                    typed = target;
                                    *state.stream_typed.lock().await = typed.clone();
                                }
                            }
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

        self.preview.lock().await.push(PreviewTask {
            kind: PreviewKind::Caption,
            stop_tx,
            task,
        });
        tracing::info!(id = %log_id, secondary, "streaming transcription preview started");
    }

    /// Spawn the cheap live audio-level loop that feeds the overlay's waveform
    /// "it hears me" pill: snapshot a tiny trailing tail at ~15 Hz, compute a
    /// normalized RMS level, emit `AudioLevelSample`. It never acquires the
    /// whisper permit or transcribes, so it adds negligible load and runs for any
    /// capture (including in-place dictation). No-op unless `preview_waveform` is
    /// on. Stored in `self.preview` so `stop_preview` tears it down too.
    pub(super) async fn start_level_loop(
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
        self.preview.lock().await.push(PreviewTask {
            kind: PreviewKind::Level,
            stop_tx,
            task,
        });
    }

    /// Stop only the CAPTION preview loop(s), leaving the cheap waveform (level)
    /// loop running. Used by a meeting source-swap so toggling which track feeds
    /// the caption never kills the "it hears me" waveform (which follows the mic
    /// for the whole meeting). `abort` tears the loop down without awaiting an
    /// in-flight tick (see [`Self::stop_preview`]); the caller then cleans up the
    /// caption loops' temp WAVs.
    async fn stop_caption_loops(&self, abort: bool) {
        let mut guard = self.preview.lock().await;
        let (captions, keep): (Vec<PreviewTask>, Vec<PreviewTask>) = std::mem::take(&mut *guard)
            .into_iter()
            .partition(|t| t.kind == PreviewKind::Caption);
        *guard = keep;
        drop(guard);
        for PreviewTask { stop_tx, task, .. } in captions {
            let _ = stop_tx.send(());
            if abort {
                task.abort();
            } else {
                let _ = task.await;
            }
        }
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
    pub(super) async fn stop_preview(&self, abort: bool) {
        let tasks: Vec<PreviewTask> = std::mem::take(&mut *self.preview.lock().await);
        for PreviewTask { stop_tx, task, .. } in tasks {
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
        let cfg = state.config.load();
        // No caption to follow when preview is off, and the source toggle is a
        // "toggle"-mode affordance only — "both" mode shows every track at once.
        // The overlay hides the button in those cases, but guard the daemon too
        // so a stray call is a harmless no-op rather than a confusing error.
        if !cfg.recording.streaming_preview || cfg.recording.meeting_preview == "both" {
            return Ok(());
        }
        let sources = self.meeting_preview_sources.lock().await.clone();
        let entry = sources
            .iter()
            .find(|(_, t, _)| t == track)
            .map(|(id, t, h)| (id.clone(), t.clone(), h.clone()));
        let Some((id, track, snapshot)) = entry else {
            return Err(Error::Internal(format!(
                "no active meeting track {track:?} to preview"
            )));
        };
        // Stop ONLY the caption loop (the waveform loop keeps running so the
        // "it hears me" pill survives the swap), and ABORT it rather than await
        // its in-flight tick — so the toggle is snappy even when a heavy preview
        // model is mid-transcription. Aborting skips the loop's own temp-WAV
        // cleanup, so remove every meeting track's preview WAV here best-effort.
        self.stop_caption_loops(true).await;
        for (sid, _, _) in &sources {
            let tmp = std::env::temp_dir().join(format!("phoneme-preview-{}.wav", sid.file_stem()));
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        // The source toggle always feeds the primary preview server (secondary =
        // false): the 2nd server only exists to run BOTH tracks at once, never
        // for a one-track toggle.
        self.start_preview(state, id, snapshot, false, false).await;
        state
            .events
            .emit(DaemonEvent::PreviewSourceChanged { track });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── stitch_preview (pure caption stitching) ───────────────────────────

    #[test]
    fn stitch_preview_appends_only_new_tail_on_overlap() {
        // The new window re-states the tail of what's shown, then adds new words.
        let committed = "the quick brown fox";
        let window = "brown fox jumps over";
        assert_eq!(
            stitch_preview(committed, window),
            Some("the quick brown fox jumps over".to_string())
        );
    }

    #[test]
    fn stitch_preview_no_change_when_window_fully_contained() {
        // The window is entirely a suffix of the committed text — nothing new.
        let committed = "hello world how are you";
        let window = "how are you";
        assert_eq!(
            stitch_preview(committed, window),
            Some(committed.to_string())
        );
    }

    #[test]
    fn stitch_preview_handles_empty_inputs() {
        assert_eq!(
            stitch_preview("", "hello world"),
            Some("hello world".to_string())
        );
        assert_eq!(
            stitch_preview("already here", ""),
            Some("already here".to_string())
        );
        assert_eq!(stitch_preview("", ""), Some(String::new()));
        // Whitespace-only window is treated as empty.
        assert_eq!(
            stitch_preview("keep me", "   "),
            Some("keep me".to_string())
        );
    }

    #[test]
    fn stitch_preview_freezes_committed_when_whisper_revises_an_early_word() {
        // The #21 fix: the loop now ALWAYS stitches (no early-phase wholesale
        // replace), so a word already shown is never rewritten even when the next
        // full re-transcription revises it — here whisper changed "meeting" ->
        // "meaning". Committed's "meeting" is kept (the preview doesn't reshuffle),
        // and only the genuinely-new tail word is appended. The accurate final
        // transcript corrects the word later.
        let committed = "the meeting went";
        let revised_full = "the meaning went well";
        assert_eq!(
            stitch_preview(committed, revised_full),
            Some("the meeting went well".to_string())
        );
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
    fn stitch_preview_returns_none_on_disjoint_window() {
        // No overlap and no containment (e.g. a pause split the speech): stitch
        // can't safely merge, so it returns None and the loop picks the phase-aware
        // fallback (append when the window has slid, replace while the take still
        // fits it) — never a blind append that could duplicate a re-transcription.
        let committed = "first sentence done";
        let window = "completely different words";
        assert_eq!(stitch_preview(committed, window), None);
    }

    #[test]
    fn stitch_preview_overlap_is_case_insensitive() {
        // Whisper may change sentence-start casing as more context arrives; the
        // overlap must still match so we don't double-print the boundary word.
        let committed = "we met at the";
        let window = "The cafe yesterday";
        assert_eq!(
            stitch_preview(committed, window),
            Some("we met at the cafe yesterday".to_string())
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
            Some("to be or not to be that is the question".to_string())
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
        let out = stitch_preview(committed, window).expect("overlap found");
        assert_eq!(out, "hello there my friend how are you doing today");

        // Hard guarantee against the actual symptom ("text comes up multiple
        // times in a row"): no word from the overlapping run appears twice.
        let words: Vec<&str> = out.split_whitespace().collect();
        for run in ["how are you", "are you doing"] {
            let occurrences = out.matches(run).count();
            assert_eq!(occurrences, 1, "run {run:?} duplicated in {out:?}");
        }
        // "you" (the boundary word) must appear exactly once, not re-stated.
        assert_eq!(
            words.iter().filter(|w| **w == "you").count(),
            1,
            "in {out:?}"
        );
    }

    #[test]
    fn stitch_preview_feeds_two_overlapping_windows_without_duplication() {
        // Simulate the loop: a non-sliding tick seeds `committed` with the whole
        // take, then a sliding tick revises the leading words and adds a tail.
        // The accumulated caption must read each word once, in order.
        let first_window = "the meeting will start at noon today";
        // First tick (take fits the window) replaces wholesale — modeled by
        // stitching onto an empty caption.
        let committed = stitch_preview("", first_window).expect("seeds from empty");
        assert_eq!(committed, first_window);

        // Second (sliding) tick: whisper re-cased "The" and dropped "the meeting",
        // re-stating from "will start" with two new trailing words.
        let second_window = "Will start at noon today in room five";
        let out = stitch_preview(&committed, second_window).expect("overlap found");
        assert_eq!(out, "the meeting will start at noon today in room five");
        assert_eq!(
            out.matches("at noon today").count(),
            1,
            "duplicated in {out:?}"
        );
    }

    #[test]
    fn merge_preview_tick_non_slid_none_keeps_committed() {
        // The bug this guards: on a no-overlap `None` while the take still FITS the
        // window (window_slid == false), the old fallback replaced the caption with
        // the bare `window`, throwing away everything shown and reshuffling every
        // word. With the fix, committed is kept verbatim this tick (the next tick
        // re-anchors) — committed is never wholesale-replaced.
        let committed = "the quick brown fox jumps";
        // A wholly different transcription with no overlapping run → `stitch_preview`
        // returns None.
        let window = "completely unrelated words here";
        assert!(
            stitch_preview(committed, window).is_none(),
            "test needs a genuinely-disjoint window to drive the None branch"
        );
        let out = merge_preview_tick(committed, window, /* window_slid */ false);
        assert_eq!(
            out, committed,
            "a non-slid no-overlap tick must keep committed, not replace it with the window"
        );
    }

    #[test]
    fn merge_preview_tick_slid_none_appends() {
        // The slid fallback must NOT regress: once the window has slid a no-overlap
        // result is a post-silence tail and is appended after committed.
        let committed = "the quick brown fox jumps";
        let window = "over the lazy dog";
        assert!(
            stitch_preview(committed, window).is_none(),
            "test needs a disjoint window to reach the fallback"
        );
        let out = merge_preview_tick(committed, window, /* window_slid */ true);
        assert_eq!(out, "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn merge_preview_tick_overlap_stitches_regardless_of_phase() {
        // When an anchor IS found, the stitch result wins in either phase — the
        // window_slid flag only governs the no-overlap fallback.
        let committed = "the quick brown fox";
        let window = "brown fox jumps over";
        for slid in [false, true] {
            assert_eq!(
                merge_preview_tick(committed, window, slid),
                "the quick brown fox jumps over",
                "slid={slid}"
            );
        }
    }

    // ── preview_committed_len (tentative-tail boundary) ───────────────────────

    #[test]
    fn committed_len_is_zero_on_first_emit() {
        // First tick: nothing committed yet, the whole window seeds the caption.
        // Everything is fresh, so the boundary is 0 (overlay dims the whole line).
        let prev = "";
        let merged = merge_preview_tick(prev, "the quick brown fox", false);
        let len = preview_committed_len(prev, &merged);
        assert_eq!(len, 0);
        assert!(len <= merged.len());
    }

    #[test]
    fn committed_len_equals_prior_committed_len_on_append() {
        // A normal append tick: the prior caption is kept verbatim and a new tail
        // is appended. The boundary is exactly the prior committed char length, so
        // only the appended tail dims.
        let prev = "the quick brown fox";
        let merged = merge_preview_tick(prev, "brown fox jumps over", /* slid */ true);
        assert_eq!(merged, "the quick brown fox jumps over");
        let len = preview_committed_len(prev, &merged);
        assert_eq!(len, prev.len());
        assert!(len <= merged.len());
        // The dimmed tail is exactly the freshly-appended words.
        assert_eq!(&merged[len..], " jumps over");
    }

    #[test]
    fn committed_len_is_full_length_when_no_new_words() {
        // The window is fully contained in the committed caption — nothing new.
        // `merge_preview_tick` returns the caption unchanged, so the boundary is
        // the full length and the overlay dims nothing.
        let prev = "hello world how are you";
        let merged = merge_preview_tick(prev, "are you", /* slid */ false);
        assert_eq!(merged, prev);
        let len = preview_committed_len(prev, &merged);
        assert_eq!(len, merged.len());
        assert!(merged[len..].is_empty());
    }

    #[test]
    fn committed_len_clamps_when_caption_would_shrink() {
        // Defensive: the boundary can never exceed the merged caption length even
        // if a (hypothetical) merge shortened the caption.
        let len = preview_committed_len("a long prior caption", "short");
        assert_eq!(len, "short".len());
    }
}
