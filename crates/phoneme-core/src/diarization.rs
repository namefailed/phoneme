//! Local speaker diarization (pyannote-style segmentation via `speakrs`) and the
//! provider-agnostic logic that attaches speaker labels to a transcript.
//!
//! The actual model inference lives in [`run_local_diarization`], which feeds
//! jobs through the process-wide [`DiarizerCache`] so the ~500 MB pipeline is
//! loaded once per daemon lifetime instead of once per transcription. The pure
//! [`assign_speakers`] function (which maps speaker turns onto ASR segments)
//! and the cache's lazy-init/invalidation logic are deliberately model-free so
//! they can be unit-tested without the ONNX models present.

use crate::config::DiarizationConfig;
use anyhow::Result;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// A speaker turn produced by the diarizer: `[start, end)` in **seconds** and an
/// opaque speaker label. We do NOT assume the label is a bare integer — pyannote
/// emits `"SPEAKER_00"`-style strings, so we treat it as an arbitrary key and
/// map it to a stable 1-based index ourselves.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerSpan {
    /// Turn start, in seconds from the start of the audio.
    pub start: f64,
    /// Turn end, in seconds from the start of the audio.
    pub end: f64,
    /// Opaque speaker label as the diarizer emits it (e.g. `"SPEAKER_00"`);
    /// mapped to a stable 1-based index by [`label_segments`].
    pub label: String,
}

/// One ASR transcript segment: `[start, end)` in **seconds** plus its text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSegment {
    /// Segment start, in seconds from the start of the audio.
    pub start: f64,
    /// Segment end, in seconds from the start of the audio.
    pub end: f64,
    /// The transcript text for this segment.
    pub text: String,
}

/// Distance from instant `t` to the `[start, end]` interval (0 when inside).
fn interval_distance(span: &SpeakerSpan, t: f64) -> f64 {
    if t < span.start {
        span.start - t
    } else if t > span.end {
        t - span.end
    } else {
        0.0
    }
}

/// Length of the overlap between a speaker span and the interval `[start, end]`
/// (0 when they don't overlap).
fn overlap(span: &SpeakerSpan, start: f64, end: f64) -> f64 {
    (span.end.min(end) - span.start.max(start)).max(0.0)
}

/// The label of the speaker who owns the transcript interval `[start, end]`: the
/// span with the **largest temporal overlap** with that interval, or — when no
/// span overlaps it at all (the line sits in a gap between turns) — the span
/// *nearest* to the interval's midpoint. Returns `None` only when there are no
/// speaker spans at all.
///
/// Using max-overlap rather than the old "first span covering the midpoint"
/// fixes mislabeling when turns overlap (the powerset model emits simultaneous
/// speakers): a line straddling a hand-off, or sitting inside two overlapping
/// turns, is attributed to whoever actually speaks for most of it instead of to
/// whichever turn merely started earliest. Picking the nearest span for a true
/// gap (rather than defaulting to "speaker 0") keeps a line just outside a turn
/// attributed to the most plausible speaker instead of a phantom one.
fn speaker_for_segment(speakers: &[SpeakerSpan], start: f64, end: f64) -> Option<&str> {
    if speakers.is_empty() {
        return None;
    }
    let best = speakers
        .iter()
        .max_by(|a, b| {
            overlap(a, start, end)
                .partial_cmp(&overlap(b, start, end))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("non-empty checked above");
    if overlap(best, start, end) > 0.0 {
        return Some(&best.label);
    }
    // No overlap: the line falls in a gap — attribute to the nearest turn by
    // distance from the segment midpoint.
    let mid = start + (end - start) / 2.0;
    speakers
        .iter()
        .min_by(|a, b| {
            interval_distance(a, mid)
                .partial_cmp(&interval_distance(b, mid))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|s| s.label.as_str())
}

/// Attach speaker labels to transcript segments, producing a `"[Speaker N]: …"`
/// formatted transcript and the number of distinct speakers actually used.
///
/// - Speaker labels are mapped to **stable 1-based indices in first-appearance
///   order**, so any label format works (`"SPEAKER_00"`, `"0"`, `"alice"`, …).
///   This fixes the previous `parse::<u8>()` mapping, which silently collapsed
///   every non-numeric label to speaker 0 (i.e. one speaker for everyone).
/// - Each segment is attributed to the speaker turn it overlaps most (by the
///   internal `speaker_for_segment` helper); one falling in a gap between turns
///   goes to the nearest turn, never a default speaker 0.
/// - Empty/whitespace segments are skipped.
///
/// When `speakers` is empty (diarization produced nothing) the segments are
/// joined into plain text with no speaker prefixes and a speaker count of 0.
pub fn assign_speakers(segments: &[TextSegment], speakers: &[SpeakerSpan]) -> (String, usize) {
    let (labeled, num_speakers) = label_segments(segments, speakers);
    let mut out = String::new();
    let mut current: Option<usize> = None;

    for (seg, idx) in labeled {
        if current != Some(idx) {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            if idx > 0 {
                out.push_str(&format!("[Speaker {idx}]: "));
            }
            current = Some(idx);
        } else {
            out.push(' ');
        }
        out.push_str(seg.text.trim());
    }

    (out, num_speakers)
}

/// Per-segment speaker attribution: each non-empty transcript segment paired
/// with its stable 1-based speaker index (0 = no diarization info), plus the
/// number of distinct speakers used. This is the structural primitive behind
/// [`assign_speakers`] — callers that persist segment timing (the timeline
/// views) take the per-segment indices from here, so the stored `speaker`
/// labels always agree with the `[Speaker N]` markers in the formatted text.
pub fn label_segments<'a>(
    segments: &'a [TextSegment],
    speakers: &[SpeakerSpan],
) -> (Vec<(&'a TextSegment, usize)>, usize) {
    use std::collections::HashMap;

    let mut label_to_idx: HashMap<&str, usize> = HashMap::new();
    let mut next_idx = 1usize;
    let mut out = Vec::new();

    for seg in segments {
        if seg.text.trim().is_empty() {
            continue;
        }
        let idx = match speaker_for_segment(speakers, seg.start, seg.end) {
            Some(label) => *label_to_idx.entry(label).or_insert_with(|| {
                let i = next_idx;
                next_idx += 1;
                i
            }),
            None => 0, // no diarization info at all → unlabeled, plain text
        };
        out.push((seg, idx));
    }

    (out, next_idx - 1)
}

/// Load a WAV as mono f32, asserting it is already in the canonical 16 kHz mono
/// format the diarizer expects. The recorder always writes 16 kHz mono and the
/// import path decodes to the same canonical format, so a mismatch here is a
/// real bug — we error loudly rather than feed interleaved / wrong-rate samples
/// to the model and silently produce garbage speaker segments.
pub fn load_audio_mono_16khz(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.channels != 1 {
        anyhow::bail!(
            "diarization expects mono audio, got {} channels in {}",
            spec.channels,
            path.display()
        );
    }
    if spec.sample_rate != 16_000 {
        anyhow::bail!(
            "diarization expects 16 kHz audio, got {} Hz in {}",
            spec.sample_rate,
            path.display()
        );
    }

    let samples: std::result::Result<Vec<f32>, _> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().collect(),
    };

    Ok(samples?)
}

/// The maximum gap (in seconds) across which two same-speaker turns are treated
/// as one continuous turn. speakrs frames are ~16.9 ms apart, so a turn that the
/// model splits across a brief breath/pause shows up as several spans separated
/// by tens of ms; coalescing anything under a quarter-second stitches those back
/// together without merging a genuine back-and-forth exchange (turn-taking gaps
/// are typically a half-second or more).
const SPEAKER_MERGE_GAP_SECS: f64 = 0.25;

/// Post-process raw speakrs turns into clean, assignment-ready speaker spans:
/// sort by start, merge adjacent same-speaker turns separated by a gap smaller
/// than `SPEAKER_MERGE_GAP_SECS`, and drop any zero/negative-length span.
///
/// This is the fix for the `to_segments` bug. `speakrs::DiarizationResult.segments`
/// is built internally as `to_segments(..)` (which emits **per-speaker** spans,
/// merely sorted by start) followed by `merge_segments(.., merge_gap)` with the
/// pipeline default `merge_gap == 0.0` — i.e. an effective no-op. So the turns we
/// got back were never actually merged: a single speaker's continuous speech
/// arrives as many tiny fragments split on every micro-pause, and consecutive
/// fragments of *different* speakers interleave. Feeding those raw fragments to
/// [`assign_speakers`] produced unstable, flickering speaker labels. Coalescing
/// same-speaker runs here (with a real gap) restores stable turns. Kept as a free
/// function so it can be unit-tested without the ONNX model.
fn clean_speaker_spans(mut spans: Vec<SpeakerSpan>, merge_gap: f64) -> Vec<SpeakerSpan> {
    spans.retain(|s| s.end > s.start);
    spans.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Stable tie-break on end so equal-start spans order deterministically.
            .then(
                a.end
                    .partial_cmp(&b.end)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let mut merged: Vec<SpeakerSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            // Same speaker, and the gap since their last turn is small enough to
            // treat as one continuous turn — extend rather than start a new span.
            // `max` guards against a fully-contained later fragment shrinking the
            // span (spans are start-sorted but ends can still nest).
            Some(last) if last.label == span.label && span.start - last.end < merge_gap => {
                last.end = last.end.max(span.end);
            }
            _ => merged.push(span),
        }
    }
    merged
}

// ── Pipeline cache ───────────────────────────────────────────────────────────

/// Process-wide lazy cache for the local diarization pipeline.
///
/// Loading the speakrs pipeline pulls the ~500 MB segmentation + embedding
/// ONNX models off disk and takes seconds; doing that per transcription (the
/// old behavior) dominated diarization cost. The cache loads the pipeline
/// once, on the first recording that actually needs it — never at daemon
/// startup, since most users keep diarization off and shouldn't pay the RAM.
///
/// Lifecycle policy:
/// - **Lazy:** nothing is loaded until [`get_or_load`](Self::get_or_load).
/// - **Config-keyed:** the cache remembers the `[diarization]` config it was
///   built under; `get_or_load` under a different config drops and reloads,
///   so a stale pipeline can never serve a run even if every external
///   invalidation hook were missed.
/// - **Load errors are never cached:** a failed load leaves the slot empty
///   and the next run retries. Worst case (models missing, diarization left
///   on) equals the pre-cache behavior — one load attempt per transcription —
///   and it self-heals the moment the cause clears (e.g. the setup wizard
///   downloads the models mid-session) without requiring a config touch.
/// - **Invalidation points:** the daemon drops the cache wherever it applies
///   config — the `ReloadConfig` IPC handler and the queue worker's post-run
///   reload — via [`invalidate_if_stale`](Self::invalidate_if_stale), and
///   [`run_local_diarization`] calls [`invalidate`](Self::invalidate) when
///   the queue worker dies so the next run reloads fresh.
///
/// Generic over the handle type `H` purely so the lazy-init / invalidation /
/// no-double-load logic is unit-testable with a fake loader (the real loader
/// needs the models, which aren't available in CI); production code uses
/// [`LocalDiarizerCache`].
pub struct DiarizerCache<H> {
    slot: Mutex<CacheSlot<H>>,
}

struct CacheSlot<H> {
    handle: Option<Arc<H>>,
    /// The `[diarization]` config snapshot the cached handle was built under.
    /// Meaningless while `handle` is `None`.
    cfg: DiarizationConfig,
}

/// The concrete cache the daemon holds (via `Transcriber`): speakrs pipelines
/// behind their background-queue handle.
pub type LocalDiarizerCache = DiarizerCache<QueuedDiarizer>;

impl<H> DiarizerCache<H> {
    /// An empty cache. Costs nothing until the first `get_or_load`.
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(CacheSlot {
                handle: None,
                cfg: DiarizationConfig::default(),
            }),
        }
    }

    /// Lock the slot, recovering from poison. Recovery is sound because the
    /// slot invariant — `handle` is `None` or a fully-built `Some` — holds at
    /// every panic point (a loader panic happens before the slot is written),
    /// so one crashed job can't disable diarization for the rest of the
    /// daemon's life.
    fn lock(&self) -> std::sync::MutexGuard<'_, CacheSlot<H>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The cached handle, or build one with `load` and cache it.
    ///
    /// The load runs while the slot lock is held — that is the entire
    /// double-load guard: a second caller racing the first blocks on the lock
    /// and then takes the cache-hit branch instead of loading again. A cached
    /// handle built under a *different* `[diarization]` config is dropped and
    /// rebuilt here, so config staleness is impossible at the point of use.
    pub fn get_or_load<F>(&self, cfg: &DiarizationConfig, load: F) -> Result<Arc<H>>
    where
        F: FnOnce() -> Result<H>,
    {
        let mut slot = self.lock();
        if slot.handle.is_some() && slot.cfg != *cfg {
            tracing::info!(
                reason = "[diarization] config changed",
                "dropping cached local diarization pipeline"
            );
            slot.handle = None;
        }
        if let Some(handle) = &slot.handle {
            tracing::debug!("local diarization pipeline cache hit");
            return Ok(handle.clone());
        }
        // Errors are deliberately not cached (the slot stays empty on `?`):
        // see the type-level policy note.
        let handle = Arc::new(load()?);
        slot.handle = Some(handle.clone());
        slot.cfg = cfg.clone();
        Ok(handle)
    }

    /// Drop the cached handle unconditionally; returns whether one was
    /// dropped. An in-flight run keeps its own `Arc` clone and finishes on
    /// the old pipeline — only after that clone drops does the queue close
    /// and the worker release the model memory.
    pub fn invalidate(&self, reason: &str) -> bool {
        let dropped = self.lock().handle.take().is_some();
        if dropped {
            tracing::info!(reason, "dropping cached local diarization pipeline");
        }
        dropped
    }

    /// Drop the cached handle only if it was built under a different
    /// `[diarization]` config than `cfg`; returns whether it was dropped.
    /// Called from the daemon's config-apply points so a backend switch or
    /// model-path change takes effect — and switching away from `local`
    /// releases the model RAM — without waiting for the next run.
    pub fn invalidate_if_stale(&self, cfg: &DiarizationConfig) -> bool {
        let mut slot = self.lock();
        if slot.handle.is_some() && slot.cfg != *cfg {
            slot.handle = None;
            drop(slot);
            tracing::info!(
                reason = "[diarization] config changed",
                "dropping cached local diarization pipeline"
            );
            true
        } else {
            false
        }
    }

    /// Whether a pipeline is currently cached (observability/tests only —
    /// the answer can be stale by the time the caller acts on it).
    pub fn is_loaded(&self) -> bool {
        self.lock().handle.is_some()
    }
}

impl<H> Default for DiarizerCache<H> {
    fn default() -> Self {
        Self::new()
    }
}

// Manual impl so `H` needn't be `Debug` (the speakrs queue handles aren't).
impl<H> std::fmt::Debug for DiarizerCache<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiarizerCache")
            .field("loaded", &self.is_loaded())
            .finish()
    }
}

/// A loaded local diarization pipeline running on speakrs's background queue
/// worker thread. The worker owns the models; this handle just feeds it jobs.
pub struct QueuedDiarizer {
    /// Sender and receiver under ONE lock: a job is pushed and its result
    /// received under the same guard, so exactly one job is ever in flight
    /// and the next result always belongs to the lock holder. This is the
    /// serialization point for overlapping transcriptions (the queue worker
    /// is serial, but retranscribe/in-place runs can race the queue) — they
    /// line up here instead of each loading a private pipeline.
    queue: Mutex<(speakrs::QueueSender, speakrs::QueueReceiver)>,
}

/// Why a queued diarization job failed — the split decides whether the cached
/// pipeline survives the failure.
enum QueueRunError {
    /// The job itself failed (inference error on this audio). The worker is
    /// still healthy, so the cache stays warm.
    Job(speakrs::PipelineError),
    /// The queue/worker is gone (panicked or shut down). The handle is
    /// permanently useless — the caller must invalidate the cache so the next
    /// run loads a fresh pipeline.
    QueueDead(anyhow::Error),
}

impl QueuedDiarizer {
    /// Load the speakrs pipeline (CPU execution for portability) and move it
    /// onto its background queue worker. Multi-second and allocation-heavy —
    /// only ever called through [`DiarizerCache::get_or_load`].
    fn load() -> Result<Self> {
        use speakrs::{ExecutionMode, OwnedDiarizationPipeline};

        tracing::info!("loading local diarization pipeline (segmentation + embedding models)");
        let started = std::time::Instant::now();
        let pipeline = OwnedDiarizationPipeline::from_pretrained(ExecutionMode::Cpu)?;
        let (sender, receiver) = pipeline.into_queued()?;
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "local diarization pipeline loaded"
        );
        Ok(Self {
            queue: Mutex::new((sender, receiver)),
        })
    }

    /// Run one diarization job to completion on the shared worker. Blocks for
    /// the whole inference (callers are already on blocking threads).
    fn diarize(
        &self,
        file_id: &str,
        audio: Vec<f32>,
    ) -> std::result::Result<speakrs::DiarizationResult, QueueRunError> {
        // Poison recovery is safe here too: the only state a panicked holder
        // can leave behind is an unclaimed result, which the job-id loop below
        // drains.
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let (sender, receiver) = &mut *queue;
        let job_id = sender
            .push(speakrs::QueuedDiarizationRequest::new(file_id, audio))
            .map_err(|e| QueueRunError::QueueDead(e.into()))?;
        loop {
            let next = receiver
                .recv()
                .map_err(|e| QueueRunError::QueueDead(e.into()))?;
            if next.job_id == job_id {
                return next.result.map_err(QueueRunError::Job);
            }
            // Unreachable while push+recv share one lock; drained defensively
            // rather than handing a stale result to the wrong recording.
            tracing::warn!(file_id = %next.file_id, "discarding unclaimed diarization result");
        }
    }
}

/// The pipeline cache plus the `[diarization]` config snapshot a run should be
/// keyed under — everything a transcription provider needs to diarize one
/// recording. Minted per provider by `Transcriber::provider`, so every minted
/// provider shares the one process-wide cache.
#[derive(Debug, Clone)]
pub struct LocalDiarizer {
    cache: Arc<LocalDiarizerCache>,
    config: DiarizationConfig,
}

impl LocalDiarizer {
    /// Bind the shared cache to the live `[diarization]` config.
    pub fn new(cache: Arc<LocalDiarizerCache>, config: DiarizationConfig) -> Self {
        Self { cache, config }
    }

    /// Diarize one audio file through the shared pipeline. Blocking — run via
    /// `spawn_blocking` from async contexts.
    pub fn run(&self, audio_path: &Path) -> Result<Vec<SpeakerSpan>> {
        run_local_diarization(audio_path, &self.cache, &self.config)
    }
}

/// Run local diarization on a 16 kHz mono WAV, returning speaker turns. The
/// pipeline comes from `cache` — loaded on first use, then reused across
/// recordings (the per-call `from_pretrained` reload this replaced cost
/// seconds and ~500 MB of churn per transcription). Blocking for the whole
/// inference; callers run it off the async runtime (e.g. `spawn_blocking`).
pub fn run_local_diarization(
    audio_path: &Path,
    cache: &LocalDiarizerCache,
    cfg: &DiarizationConfig,
) -> Result<Vec<SpeakerSpan>> {
    // Decode the audio before touching the cache so a bad WAV fails fast
    // without costing (or being blamed on) a model load.
    let audio = load_audio_mono_16khz(audio_path)?;
    let pipeline = cache.get_or_load(cfg, QueuedDiarizer::load)?;

    // The file id is only a label (speakrs uses it for RTTM/log output).
    let file_id = audio_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".to_string());

    let result = match pipeline.diarize(&file_id, audio) {
        Ok(result) => result,
        Err(QueueRunError::Job(e)) => return Err(e.into()),
        Err(QueueRunError::QueueDead(e)) => {
            // This run fails (the caller falls back to an unlabeled
            // transcript), but the dead handle must not stay cached — drop it
            // so the next run loads a fresh pipeline instead of failing
            // forever.
            cache.invalidate("diarization queue worker died");
            return Err(e.context("diarization queue worker died"));
        }
    };

    // `result.segments` carries correctly-scaled (seconds) turns — that part of
    // the prior fix was right; the old `to_segments(1.0, 1.0)` had passed a frame
    // STEP/DURATION of 1.0 s against the model's real ~16.9 ms / ~61.9 ms geometry
    // and inflated every timestamp ~59×. But `result.segments` is NOT actually
    // merged (speakrs builds it with the default `merge_gap == 0.0`, a no-op), so
    // we coalesce same-speaker fragments ourselves before handing them off.
    let spans = result
        .segments
        .into_iter()
        .map(|s| SpeakerSpan {
            start: s.start,
            end: s.end,
            label: s.speaker,
        })
        .collect();

    Ok(clean_speaker_spans(spans, SPEAKER_MERGE_GAP_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, text: &str) -> TextSegment {
        TextSegment {
            start,
            end,
            text: text.to_string(),
        }
    }
    fn span(start: f64, end: f64, label: &str) -> SpeakerSpan {
        SpeakerSpan {
            start,
            end,
            label: label.to_string(),
        }
    }

    #[test]
    fn non_numeric_labels_map_to_distinct_speakers() {
        // The bug this guards against: pyannote labels like "SPEAKER_00" used to
        // be parse::<u8>()'d, fail, and collapse everyone to Speaker 0.
        let segments = vec![
            seg(0.0, 2.0, "hello there"),
            seg(2.0, 4.0, "hi back"),
            seg(4.0, 6.0, "how are you"),
        ];
        let speakers = vec![
            span(0.0, 2.0, "SPEAKER_00"),
            span(2.0, 4.0, "SPEAKER_01"),
            span(4.0, 6.0, "SPEAKER_00"),
        ];
        let (text, n) = assign_speakers(&segments, &speakers);
        assert_eq!(n, 2, "two distinct speakers must be recognized");
        assert!(text.contains("[Speaker 1]: hello there"));
        assert!(text.contains("[Speaker 2]: hi back"));
        // Returning to SPEAKER_00 reuses index 1, not a new number.
        assert_eq!(text.matches("[Speaker 1]").count(), 2);
    }

    #[test]
    fn gap_attributed_to_nearest_not_speaker_zero() {
        // A transcript segment in a diarization gap should go to the nearest
        // turn, never silently to a phantom speaker 0.
        let segments = vec![seg(5.0, 6.0, "in the gap")];
        let speakers = vec![span(0.0, 4.0, "SPEAKER_00"), span(10.0, 14.0, "SPEAKER_01")];
        let (text, n) = assign_speakers(&segments, &speakers);
        assert_eq!(n, 1);
        // Midpoint 5.5 is nearest to SPEAKER_00 (ends at 4.0, dist 1.5) vs
        // SPEAKER_01 (starts at 10.0, dist 4.5).
        assert!(text.starts_with("[Speaker 1]: in the gap"), "got: {text}");
    }

    #[test]
    fn no_speakers_yields_plain_text() {
        let segments = vec![seg(0.0, 1.0, "alpha"), seg(1.0, 2.0, "beta")];
        let (text, n) = assign_speakers(&segments, &[]);
        assert_eq!(n, 0);
        assert!(!text.contains("[Speaker"));
        assert_eq!(text, "alpha beta");
    }

    #[test]
    fn consecutive_same_speaker_segments_join_without_relabel() {
        let segments = vec![seg(0.0, 1.0, "one"), seg(1.0, 2.0, "two")];
        let speakers = vec![span(0.0, 2.0, "SPEAKER_00")];
        let (text, n) = assign_speakers(&segments, &speakers);
        assert_eq!(n, 1);
        assert_eq!(text, "[Speaker 1]: one two");
    }

    #[test]
    fn empty_segments_are_skipped() {
        let segments = vec![seg(0.0, 1.0, "   "), seg(1.0, 2.0, "real")];
        let speakers = vec![span(0.0, 2.0, "alice")];
        let (text, n) = assign_speakers(&segments, &speakers);
        assert_eq!(n, 1);
        assert_eq!(text, "[Speaker 1]: real");
    }

    // ── The `to_segments` bug: fragment coalescing ───────────────────────────

    #[test]
    fn clean_spans_merges_same_speaker_fragments_across_micro_pauses() {
        // Reproduces the `to_segments` bug. speakrs returns one speaker's
        // continuous speech as several spans split on every micro-pause (here a
        // ~50 ms breath at 1.0 and ~80 ms at 2.05), because the pipeline's merge
        // step runs with `merge_gap == 0.0` and never coalesces them. Those raw
        // fragments must collapse back into one turn.
        let raw = vec![
            span(0.0, 1.0, "SPEAKER_00"),
            span(1.05, 2.05, "SPEAKER_00"),
            span(2.13, 3.0, "SPEAKER_00"),
        ];
        let cleaned = clean_speaker_spans(raw, SPEAKER_MERGE_GAP_SECS);
        assert_eq!(cleaned.len(), 1, "one continuous turn, got: {cleaned:?}");
        assert_eq!(cleaned[0], span(0.0, 3.0, "SPEAKER_00"));
    }

    #[test]
    fn clean_spans_keeps_genuine_turn_taking_separate() {
        // A real back-and-forth must NOT be merged: the half-second-plus gap
        // between turns is well above the merge threshold, and the speakers
        // differ regardless.
        let raw = vec![
            span(0.0, 2.0, "SPEAKER_00"),
            span(2.6, 4.0, "SPEAKER_01"),
            span(4.7, 6.0, "SPEAKER_00"),
        ];
        let cleaned = clean_speaker_spans(raw.clone(), SPEAKER_MERGE_GAP_SECS);
        assert_eq!(cleaned, raw, "distinct turns must be preserved");
    }

    #[test]
    fn clean_spans_sorts_and_drops_empty() {
        // speakrs emits per-speaker spans sorted only by start; a zero/negative
        // length span (a clustering artifact) carries no speech and is dropped.
        let raw = vec![
            span(4.0, 6.0, "SPEAKER_01"),
            span(3.0, 3.0, "SPEAKER_00"), // zero-length → dropped
            span(0.0, 2.0, "SPEAKER_00"),
        ];
        let cleaned = clean_speaker_spans(raw, SPEAKER_MERGE_GAP_SECS);
        assert_eq!(
            cleaned,
            vec![span(0.0, 2.0, "SPEAKER_00"), span(4.0, 6.0, "SPEAKER_01")]
        );
    }

    #[test]
    fn clean_spans_absorbs_nested_same_speaker_fragment() {
        // A later same-speaker fragment fully contained in the previous turn must
        // not shrink it (ends can nest even though spans are start-sorted).
        let raw = vec![span(0.0, 5.0, "SPEAKER_00"), span(1.0, 2.0, "SPEAKER_00")];
        let cleaned = clean_speaker_spans(raw, SPEAKER_MERGE_GAP_SECS);
        assert_eq!(cleaned, vec![span(0.0, 5.0, "SPEAKER_00")]);
    }

    // ── The overlap mis-assignment bug ───────────────────────────────────────

    #[test]
    fn overlapping_turns_attributed_by_max_overlap_not_earliest_start() {
        // The powerset model emits simultaneous speakers, so turns overlap. Two
        // overlapping turns ([0,9] and [5,12]) and two transcript lines:
        //   - "first speaker" [0,3]: only inside SPEAKER_00 → Speaker 1.
        //   - "second speaker" [6,11]: midpoint 8.5 is inside BOTH turns, but it
        //     overlaps SPEAKER_01 more (5.0 s vs 3.0 s).
        // The old midpoint-first-match logic attributed the second line to the
        // earliest-starting covering span (SPEAKER_00), collapsing the whole
        // transcript onto ONE speaker. Max-overlap recovers the second speaker.
        let speakers = vec![span(0.0, 9.0, "SPEAKER_00"), span(5.0, 12.0, "SPEAKER_01")];
        let segments = vec![
            seg(0.0, 3.0, "first speaker"),
            seg(6.0, 11.0, "second speaker"),
        ];
        let (text, n) = assign_speakers(&segments, &speakers);
        assert_eq!(n, 2, "both speakers must be recovered, got: {text}");
        assert_eq!(
            text,
            "[Speaker 1]: first speaker\n\n[Speaker 2]: second speaker"
        );
    }

    #[test]
    fn straddling_handoff_goes_to_dominant_speaker() {
        // Two non-overlapping turns with a clean hand-off at 5.0. A line that
        // straddles it [4.0, 8.0] overlaps SPEAKER_00 by 1.0s and SPEAKER_01 by
        // 3.0s → SPEAKER_01 owns it.
        let speakers = vec![span(0.0, 5.0, "SPEAKER_00"), span(5.0, 10.0, "SPEAKER_01")];
        let segments = vec![
            seg(0.5, 4.0, "first speaker talks"),
            seg(4.0, 8.0, "straddles the handoff"),
        ];
        let (text, _) = assign_speakers(&segments, &speakers);
        assert!(
            text.contains("[Speaker 1]: first speaker talks"),
            "got: {text}"
        );
        assert!(
            text.contains("[Speaker 2]: straddles the handoff"),
            "got: {text}"
        );
    }

    #[test]
    fn end_to_end_fragmented_overlapping_diarization_is_stable() {
        // Putting both fixes together on realistic raw speakrs output: SPEAKER_00
        // is fragmented across micro-pauses and its turns overlap SPEAKER_01's.
        // After cleaning, the transcript reads as two stable turns instead of a
        // flickering Speaker 1/2/1/2 mess.
        let raw = vec![
            span(0.0, 2.0, "SPEAKER_00"),
            span(2.1, 4.0, "SPEAKER_00"), // micro-pause → same turn
            span(3.8, 8.0, "SPEAKER_01"), // overlaps the tail of SPEAKER_00
            span(8.1, 9.0, "SPEAKER_01"), // micro-pause → same turn
        ];
        let speakers = clean_speaker_spans(raw, SPEAKER_MERGE_GAP_SECS);
        // Two clean turns: SPEAKER_00 [0,4], SPEAKER_01 [3.8,9].
        assert_eq!(
            speakers.len(),
            2,
            "expected 2 merged turns, got: {speakers:?}"
        );

        let segments = vec![
            seg(0.0, 2.0, "a one"),
            seg(2.0, 4.0, "a two"),
            seg(4.0, 6.0, "b one"),
            seg(6.0, 9.0, "b two"),
        ];
        let (text, n) = assign_speakers(&segments, &speakers);
        assert_eq!(n, 2, "exactly two speakers, got: {text}");
        assert_eq!(text, "[Speaker 1]: a one a two\n\n[Speaker 2]: b one b two");
    }

    // ── Pipeline cache: lazy init / invalidation / no double load ────────────
    //
    // Exercised through `DiarizerCache<&str>` with counting fake loaders. The
    // real loader (speakrs `from_pretrained` + the queue plumbing in
    // `QueuedDiarizer`) needs the ~500 MB models and stays untested here —
    // these tests pin the lifecycle logic everything else hangs off.

    use crate::config::DiarizationBackend;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn diar_cfg(provider: DiarizationBackend, model_path: &str) -> DiarizationConfig {
        DiarizationConfig {
            provider,
            local_model_path: model_path.to_string(),
        }
    }

    #[test]
    fn cache_is_lazy_until_first_use() {
        let cache: DiarizerCache<&str> = DiarizerCache::new();
        assert!(!cache.is_loaded(), "a fresh cache must hold nothing");

        let cfg = diar_cfg(DiarizationBackend::Local, "");
        let handle = cache.get_or_load(&cfg, || Ok("pipeline")).unwrap();
        assert_eq!(*handle, "pipeline");
        assert!(cache.is_loaded());
    }

    #[test]
    fn second_use_is_a_cache_hit() {
        let cache: DiarizerCache<&str> = DiarizerCache::new();
        let cfg = diar_cfg(DiarizationBackend::Local, "");
        let loads = AtomicUsize::new(0);
        let load = || {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok("pipeline")
        };

        let first = cache.get_or_load(&cfg, load).unwrap();
        let second = cache.get_or_load(&cfg, load).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1, "one load serves both runs");
        assert!(
            Arc::ptr_eq(&first, &second),
            "both runs must share the same pipeline"
        );
    }

    #[test]
    fn changed_config_reloads_at_point_of_use() {
        // The use-time config check is the correctness backbone: even if every
        // daemon invalidation hook were missed, a run under a new
        // `[diarization]` config must never reuse a pipeline built under the
        // old one.
        let cache: DiarizerCache<&str> = DiarizerCache::new();
        let old = cache
            .get_or_load(&diar_cfg(DiarizationBackend::Local, ""), || Ok("old"))
            .unwrap();
        let new = cache
            .get_or_load(
                &diar_cfg(DiarizationBackend::Local, "C:/models/x.onnx"),
                || Ok("new"),
            )
            .unwrap();
        assert!(!Arc::ptr_eq(&old, &new), "stale pipeline must be dropped");
        assert_eq!(*new, "new");
    }

    #[test]
    fn load_errors_are_not_cached() {
        // Policy: a failed load must not poison the cache. The slot stays
        // empty so the next run retries — which is what lets a mid-session
        // model download (the setup wizard) start working without a config
        // change or daemon restart.
        let cache: DiarizerCache<&str> = DiarizerCache::new();
        let cfg = diar_cfg(DiarizationBackend::Local, "");

        let err = cache.get_or_load(&cfg, || anyhow::bail!("models missing"));
        assert!(err.is_err());
        assert!(
            !cache.is_loaded(),
            "a failed load must leave the slot empty"
        );

        let ok = cache.get_or_load(&cfg, || Ok("healed")).unwrap();
        assert_eq!(*ok, "healed");
    }

    #[test]
    fn invalidate_drops_and_reports() {
        let cache: DiarizerCache<&str> = DiarizerCache::new();
        assert!(!cache.invalidate("nothing cached"), "empty cache: no-op");

        let cfg = diar_cfg(DiarizationBackend::Local, "");
        cache.get_or_load(&cfg, || Ok("pipeline")).unwrap();
        assert!(cache.invalidate("worker died"));
        assert!(!cache.is_loaded());
    }

    #[test]
    fn invalidate_if_stale_only_drops_on_config_change() {
        let cache: DiarizerCache<&str> = DiarizerCache::new();
        let cfg = diar_cfg(DiarizationBackend::Local, "");
        assert!(
            !cache.invalidate_if_stale(&cfg),
            "empty cache is never stale"
        );

        let handle = cache.get_or_load(&cfg, || Ok("pipeline")).unwrap();

        // Same config reapplied (the queue worker reloads config after every
        // run): the warm pipeline must survive.
        assert!(!cache.invalidate_if_stale(&cfg));
        let again = cache.get_or_load(&cfg, || Ok("reloaded")).unwrap();
        assert!(Arc::ptr_eq(&handle, &again));

        // Backend switched away from Local: drop (this is what releases the
        // model RAM when the user turns diarization off).
        assert!(cache.invalidate_if_stale(&diar_cfg(DiarizationBackend::None, "")));
        assert!(!cache.is_loaded());
    }

    #[test]
    fn concurrent_first_use_loads_exactly_once() {
        // Two transcriptions hitting a cold cache at the same time (queue
        // worker + a retranscribe) must not both pay the load: the loser
        // blocks on the slot lock, then takes the cache-hit branch.
        let cache = Arc::new(DiarizerCache::<usize>::new());
        let loads = Arc::new(AtomicUsize::new(0));
        let cfg = diar_cfg(DiarizationBackend::Local, "");

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let cache = cache.clone();
                let loads = loads.clone();
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    cache
                        .get_or_load(&cfg, || {
                            loads.fetch_add(1, Ordering::SeqCst);
                            // Hold the load long enough that the other threads
                            // pile up on the lock while it is in progress.
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            Ok(42usize)
                        })
                        .unwrap()
                })
            })
            .collect();

        let results: Vec<Arc<usize>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(loads.load(Ordering::SeqCst), 1, "exactly one load");
        assert!(results.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
    }

    #[test]
    fn loader_panic_does_not_wedge_the_cache() {
        // A loader panic poisons the slot mutex mid-load; the lock recovery is
        // sound because the slot is still empty at that point. The next run
        // must be able to load normally instead of hitting PoisonError panics
        // forever.
        let cache = Arc::new(DiarizerCache::<&str>::new());
        let cfg = diar_cfg(DiarizationBackend::Local, "");

        let crashing = {
            let cache = cache.clone();
            let cfg = cfg.clone();
            std::thread::spawn(move || {
                let _ = cache.get_or_load(&cfg, || panic!("loader exploded"));
            })
        };
        assert!(crashing.join().is_err(), "loader panic propagates");

        let healed = cache.get_or_load(&cfg, || Ok("healed")).unwrap();
        assert_eq!(*healed, "healed");
    }
}
