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
use ndarray::{Array2, Array3};
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

/// One transcript word with its audio-relative timing, the unit of per-word
/// speaker attribution.
///
/// Times are **seconds** from the start of the audio — the same clock the
/// diarizer's frame matrix uses, so a word's span maps straight onto frame rows
/// with no offset. This is the diarization-layer mirror of
/// [`crate::types::TranscriptWord`] (which carries milliseconds): the provider
/// path converts ms → seconds when handing words to [`assign_words`], keeping
/// this module free of the persistence type and unit-testable with bare floats.
#[derive(Debug, Clone, PartialEq)]
pub struct WordSpan {
    /// Word start, in seconds from the start of the audio.
    pub start: f64,
    /// Word end, in seconds from the start of the audio.
    pub end: f64,
    /// The single word/token text.
    pub text: String,
    /// Whether this token starts a new written word (whisper's leading-space
    /// marker; see [`crate::types::TranscriptWord::leading_space`]). A token that
    /// does NOT — punctuation, a clitic (`'s`/`'t`), or a subword continuation —
    /// must share its host word's speaker, so [`assign_words`] never strands a
    /// `.` on the next turn or splits `That's` across speakers. Defaults to `true`
    /// (a normal space-separated word) for callers/tests that don't set it.
    pub leading_space: bool,
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

/// Label every transcript segment as one fixed speaker, producing the same
/// `[Speaker N]: …` text and the matching persisted timeline that
/// [`assign_speakers`] / [`label_segments`] produce — but without running the
/// diarizer at all.
///
/// This is the track-aware Meeting-Mode short-circuit: a meeting's mic track is
/// a single voice (the user's), so there is nothing to diarize. Reusing the
/// existing `[Speaker N]` machinery (rather than inventing a `[You]` marker)
/// keeps the pipeline's `diarized` detection (`"[Speaker "`) and the
/// merged-meeting view (`[Speaker N]:`) working unchanged; when this labelling
/// actually runs (the result's `fixed_speaker_applied`), the daemon SEEDS label
/// `speaker_label` as "You" via an if-absent `speaker_names` row — a friendly
/// default on the first transcribe that never overwrites a row, so a later user
/// rename survives a retranscribe.
///
/// `speaker_label` is the 1-based index the segments are stamped with (1 for the
/// mic track). Empty/whitespace segments are skipped, exactly as
/// [`label_segments`] skips them. Returns the formatted text plus the timeline,
/// every segment carrying `speaker = Some(speaker_label)` so the stored labels
/// agree with the `[Speaker N]` markers in the text.
pub fn label_all_as(
    segments: &[TextSegment],
    speaker_label: usize,
) -> (String, Vec<crate::types::TranscriptSegment>) {
    let mut text = String::new();
    let mut out_segments = Vec::with_capacity(segments.len());
    for seg in segments {
        let trimmed = seg.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        // One turn for the whole track: the marker is emitted once, then each
        // subsequent segment's text is space-joined onto it — mirroring the
        // same-speaker join in `assign_speakers`.
        if out_segments.is_empty() {
            text.push_str(&format!("[Speaker {speaker_label}]: "));
        } else {
            text.push(' ');
        }
        text.push_str(trimmed);
        out_segments.push(crate::types::TranscriptSegment {
            start_ms: (seg.start * 1000.0).round() as i64,
            end_ms: (seg.end * 1000.0).round() as i64,
            text: trimmed.to_string(),
            speaker: Some(speaker_label.to_string()),
        });
    }
    (text, out_segments)
}

// ── Per-word speaker attribution (from the frame-activation matrix) ──────────

/// The frame row whose window *covers* instant `t` seconds, using speakrs's own
/// frame geometry: `round((t - 0.5 * frame_duration) / frame_step)`.
///
/// speakrs does NOT place frame `f` at `f * STEP`; it centers frame `f` at
/// `frame_middle(f) = f * FRAME_STEP_SECONDS + 0.5 * FRAME_DURATION_SECONDS`
/// (speakrs `segment.rs`), which is the geometry behind the very
/// `result.segments` / [`SpeakerSpan`]s this module also returns. Its canonical
/// inverse is therefore `closest_frame(t) = round((t - 0.5*FRAME_DURATION)/STEP)`
/// (speakrs `layout.rs`). Using that exact mapping keeps the per-word frame
/// window in the SAME time domain as the segment-level spans. Omitting the
/// half-duration offset (and using `floor` instead of `round`) would bias every
/// word ~1.8 frames (~30 ms) late and make the word and segment timelines
/// disagree at speaker hand-offs — exactly the boundary case word-level
/// attribution exists to get right.
///
/// `t` at/below the first frame's center clamps to row 0. The index is *not*
/// bounds-checked against the matrix height; callers clamp it to the last row,
/// since the final frame can end slightly before the last word's timestamp.
fn frame_for_time(t: f64, frame_step: f64, frame_duration: f64) -> usize {
    let f = ((t - 0.5 * frame_duration) / frame_step).round();
    if f <= 0.0 {
        0
    } else {
        f as usize
    }
}

/// speakrs labels the `k`-th column of its activation matrix `SPEAKER_{k:02}` —
/// the exact label its `to_segments` (and therefore `DiarizationResult.segments`,
/// our [`SpeakerSpan`] source) emits. Producing the identical string here lets a
/// per-word column index flow through the SAME first-appearance map
/// [`label_segments`] uses, so word-level and segment-level labels share one
/// stable `[Speaker N]` numbering.
fn column_label(speaker_idx: usize) -> String {
    format!("SPEAKER_{speaker_idx:02}")
}

/// The speaker column with the most total activation over the frame range
/// `[start_frame, end_frame]` (inclusive of both, so a sub-frame word still
/// scores its one overlapping frame). Returns `None` when the matrix has no
/// speaker columns, or when no frame in range carries any activation (a word
/// landing in pure silence — the caller treats it as unattributed).
fn dominant_column(
    activations: &Array2<f32>,
    start_frame: usize,
    end_frame: usize,
) -> Option<usize> {
    let (num_frames, num_speakers) = activations.dim();
    if num_speakers == 0 || num_frames == 0 {
        return None;
    }
    // Clamp to the matrix: a word can end a hair past the final frame.
    let last = num_frames - 1;
    let lo = start_frame.min(last);
    let hi = end_frame.min(last);

    let mut best: Option<(usize, f64)> = None;
    for spk in 0..num_speakers {
        let mut sum = 0.0f64;
        for frame in lo..=hi {
            sum += activations[[frame, spk]] as f64;
        }
        // Strict `>` keeps the lowest column index on ties, so a tie is
        // resolved deterministically toward the first-appearing speaker.
        match best {
            Some((_, bsum)) if sum <= bsum => {}
            _ => best = Some((spk, sum)),
        }
    }
    best.and_then(|(spk, sum)| (sum > 0.0).then_some(spk))
}

/// Per-word speaker attribution from the diarizer's per-frame activation matrix:
/// each word paired with its stable 1-based speaker index (0 = unattributed),
/// plus the number of distinct speakers used. The word-level counterpart of
/// [`label_segments`], and it shares that function's labelling contract:
///
/// - A word's `[start, end]` span maps to the frame range covering it via
///   speakrs's `closest_frame` geometry (see `frame_for_time`); the speaker
///   column with the most summed activation over that range wins (so a word
///   straddling a hand-off goes to whoever speaks for most of it — the case
///   whole-segment attribution gets wrong).
/// - The winning column `k` becomes label `SPEAKER_{k:02}` and is mapped to a
///   stable 1-based index **in first-appearance order**, the identical scheme
///   [`label_segments`] applies to `DiarizationResult.segments`. So the
///   `[Speaker N]` numbers a word-level transcript shows match what the
///   segment-level path would have produced for the same speakers.
/// - A word landing in silence (no activation in its frames) gets index 0 and
///   is excluded from the speaker count, mirroring the segment-level `None`.
/// - Empty/whitespace words are skipped (as empty segments are).
///
/// - Sub-`min_turn` speaker islands are smoothed away before numbering: a single
///   short word the diarizer momentarily scored to another speaker (the classic
///   "[Speaker 2]: it" flicker) is absorbed into its dominant neighbour, so a
///   one-voice recording collapses back to a single speaker (and the caller's
///   ≤1-speaker gate renders it as plain prose) instead of fragmenting. See
///   `smooth_word_speaker_runs`. Pass `min_turn = 0.0` to disable it.
///
/// `frame_step` / `frame_duration` are `speakrs::pipeline::FRAME_STEP_SECONDS` /
/// `FRAME_DURATION_SECONDS` in production; `min_turn` is `WORD_MIN_TURN_SECS`.
/// All three are parameters so the mapping + smoothing are unit-testable with a
/// synthetic matrix (the geometry tests pass `min_turn = 0.0`).
pub fn assign_words<'a>(
    words: &'a [WordSpan],
    activations: &Array2<f32>,
    frame_step: f64,
    frame_duration: f64,
    min_turn: f64,
) -> (Vec<(&'a WordSpan, usize)>, usize) {
    use std::collections::HashMap;

    // Non-empty words only, mirroring `label_segments` skipping empty segments.
    let kept: Vec<&WordSpan> = words.iter().filter(|w| !w.text.trim().is_empty()).collect();

    // Raw per-word dominant speaker column (None = silence / no activation).
    let mut cols: Vec<Option<usize>> = kept
        .iter()
        .map(|w| {
            let start_frame = frame_for_time(w.start, frame_step, frame_duration);
            let end_frame = frame_for_time(w.end, frame_step, frame_duration);
            dominant_column(activations, start_frame, end_frame)
        })
        .collect();

    // Absorb sub-`min_turn` speaker flips so a monologue doesn't fragment, then
    // back-fill any word the geometry left unattributed into a neighbouring
    // speaker so it doesn't orphan and split its turn. Both are production
    // cleanup gated off when `min_turn == 0.0` (the geometry-test "raw" knob).
    if min_turn > 0.0 {
        smooth_word_speaker_runs(&kept, &mut cols, min_turn);
        backfill_unattributed_words(&kept, &mut cols);
        coalesce_subword_tokens(&kept, &mut cols);
    }

    // Map columns → stable 1-based indices in first-appearance order, via the
    // same `SPEAKER_{k:02}` label `label_segments` keys on, so word- and
    // segment-level transcripts number identical speakers identically.
    let mut label_to_idx: HashMap<String, usize> = HashMap::new();
    let mut next_idx = 1usize;
    let mut out = Vec::with_capacity(kept.len());
    for (word, col) in kept.iter().zip(cols.iter()) {
        let idx = match col {
            Some(c) => {
                let label = column_label(*c);
                *label_to_idx.entry(label).or_insert_with(|| {
                    let i = next_idx;
                    next_idx += 1;
                    i
                })
            }
            None => 0, // word in silence / empty matrix → unattributed
        };
        out.push((*word, idx));
    }

    (out, next_idx - 1)
}

/// A per-word speaker turn shorter than this (seconds) is treated as a diarizer
/// flicker — a single short word ("it", "if") momentarily scored to a second
/// speaker — and absorbed into the dominant neighbouring speaker. So a one-voice
/// recording collapses back to a single speaker (and renders as plain prose)
/// instead of fragmenting into phantom `[Speaker 2]` islands, while genuine
/// turns (comfortably longer than this) are untouched. The segment path's coarse
/// granularity rarely flips a whole sentence, so this guards the finer word
/// granularity that word-level attribution introduced; its segment-level analogue
/// is [`SPEAKER_MERGE_GAP_SECS`].
pub(crate) const WORD_MIN_TURN_SECS: f64 = 0.6;

/// A speaker run no longer than this many words, when it sits as an "island"
/// bracketed by the SAME speaker on both sides, is treated as per-frame flicker
/// and absorbed into that surrounding speaker. This is the primary guard against
/// the mid-sentence choppy splits the wall-clock-only `WORD_MIN_TURN_SECS`
/// missed: a 2–5 word island inside one continuous speaker's territory (e.g.
/// "...the fact that women / [Speaker 2] going to do what they / [Speaker 1]
/// want...") is almost always noise from per-word argmax over short, noisy frame
/// windows, not a real turn. Genuine turns survive because they are either longer
/// than this OR sit at a real transition (a DIFFERENT speaker on each side, not
/// the same one) — only same-speaker-bracketed islands are absorbed. A lone
/// single word is absorbed regardless of position (one word is never a real
/// turn). Per-word attribution is kept, so a genuine hand-off INSIDE a whisper
/// segment is still split — only the noise islands are smoothed.
///
/// NOTE: this counts the diarization layer's word units, which for local
/// whisper.cpp are SUBWORD tokens ("over ste pped", "don 't" each split into
/// several), so the bound is roughly twice the spoken-word count it implies
/// (~10 tokens ≈ ~5 spoken words). It only ever applies to runs bracketed by the
/// SAME speaker (one voice either side), where even a longish island is almost
/// always that voice continuing, not a real interjection.
const MAX_ISLAND_WORDS: usize = 10;

/// The larger ceiling for a same-speaker-bracketed island that is also strictly
/// shorter than BOTH of its (same-speaker) neighbours. Between [`MAX_ISLAND_WORDS`]
/// and this, a run is absorbed only when one voice clearly dominates on both
/// sides — a brief blip mid-monologue the diarizer mis-scored to the other
/// speaker (the real case: a ~16-token "cyber weapon? I mean, I mean, because you
/// don't" stranded inside a 31-token question and a 144-token monologue, both the
/// same speaker). Above this ceiling a run is treated as a genuine turn and never
/// silently merged, even if it happens to be shorter than two very long
/// monologues. ~24 tokens ≈ ~12 spoken words (whisper emits subword tokens).
const MAX_BRACKETED_ISLAND_WORDS: usize = 24;

/// A contiguous run of same-speaker words inside the per-word column sequence.
struct SpeakerRun {
    /// First word index in the run (into the `cols` / `words` slices).
    start: usize,
    /// Last word index in the run (inclusive).
    end: usize,
    /// The speaker column the run is assigned to.
    col: usize,
    /// Wall-clock span of the run in seconds (`last.end - first.start`).
    span: f64,
}

/// The speaker runs in `cols`, in order. `None` (silence) words belong to no run
/// and split runs, but two runs separated only by silence are still adjacent in
/// the returned list — so a flip bracketed by silence still smooths against its
/// real neighbours.
fn speaker_runs(words: &[&WordSpan], cols: &[Option<usize>]) -> Vec<SpeakerRun> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < cols.len() {
        if let Some(c) = cols[i] {
            let start = i;
            while i + 1 < cols.len() && cols[i + 1] == Some(c) {
                i += 1;
            }
            let span = (words[i].end - words[start].start).max(0.0);
            runs.push(SpeakerRun {
                start,
                end: i,
                col: c,
                span,
            });
        }
        i += 1;
    }
    runs
}

/// In-place smoothing of the per-word speaker columns: repeatedly absorb a
/// "flicker island" speaker run into a neighbour until none remain or only one
/// speaker is left. A run is an island to absorb when it is:
///
/// - a LONE single word (one word is never a real turn);
/// - a short run bracketed by the SAME speaker on both sides and no longer than
///   [`MAX_ISLAND_WORDS`] (a noise island inside one continuous speaker's
///   territory — the mid-sentence-flip case); or
/// - shorter than `min_turn` wall-clock seconds (a brief blip).
///
/// It is absorbed into the surrounding speaker when bracketed, otherwise into the
/// longer neighbour (tie → the previous run). Genuine turns survive: they are
/// either longer than the island bounds OR sit at a real transition (a different
/// speaker on each side, so not "bracketed by the same speaker"). Smallest
/// islands smooth first; only absorptions that actually flip a column count as
/// progress (a run already matching its chosen neighbour is skipped), so it
/// always terminates. Silence words stay `None`.
///
/// This restores coherent turns — like the older whole-segment attribution —
/// while KEEPING per-word attribution, so a genuine speaker hand-off inside one
/// whisper segment is still split (only noise islands are removed). It fixes the
/// regression where a recording was chopped into per-word `[Speaker N]` flips
/// mid-sentence.
fn smooth_word_speaker_runs(words: &[&WordSpan], cols: &mut [Option<usize>], min_turn: f64) {
    let words_in = |r: &SpeakerRun| r.end - r.start + 1;
    loop {
        let runs = speaker_runs(words, cols);
        if runs.len() < 2 {
            break; // 0 or 1 speaker run — nothing to absorb into.
        }
        // Same-speaker bracket = a genuine island (one voice either side), as
        // opposed to a real transition (different voices each side).
        let bracketed_same = |ri: usize| -> bool {
            match (ri.checked_sub(1), runs.get(ri + 1)) {
                (Some(p), Some(n)) => runs[p].col == n.col,
                _ => false,
            }
        };
        let absorbable = |ri: usize| -> bool {
            let r = &runs[ri];
            if words_in(r) == 1 || r.span < min_turn {
                return true;
            }
            if bracketed_same(ri) {
                // ri-1 and ri+1 both exist (that's what bracketed_same checks).
                let prev = words_in(&runs[ri - 1]);
                let next = words_in(&runs[ri + 1]);
                // A small island is always flicker; a MEDIUM one is absorbed only
                // when the SAME speaker dwarfs it on both sides — a brief blip
                // inside one continuous monologue, not a real interjection. Large
                // islands (a genuine turn) are never silently merged.
                return words_in(r) <= MAX_ISLAND_WORDS
                    || (words_in(r) <= MAX_BRACKETED_ISLAND_WORDS
                        && words_in(r) < prev
                        && words_in(r) < next);
            }
            false
        };
        // Smallest islands first (by word count) — deterministic.
        let mut order: Vec<usize> = (0..runs.len()).filter(|&i| absorbable(i)).collect();
        order.sort_by_key(|&i| words_in(&runs[i]));

        let mut changed = false;
        for ri in order {
            let run = &runs[ri];
            let prev = ri.checked_sub(1).map(|i| &runs[i]);
            let next = runs.get(ri + 1);
            // Bracketed → absorb into the surrounding speaker; otherwise into the
            // longer neighbour (tie → previous).
            let target = match (prev, next) {
                (Some(p), Some(n)) if p.col == n.col => p.col,
                (Some(p), Some(n)) => {
                    if n.span > p.span {
                        n.col
                    } else {
                        p.col
                    }
                }
                (Some(p), None) => p.col,
                (None, Some(n)) => n.col,
                (None, None) => continue,
            };
            if target == run.col {
                continue; // already this speaker (same voice across a silence) → no-op.
            }
            for c in cols[run.start..=run.end].iter_mut() {
                if c.is_some() {
                    *c = Some(target);
                }
            }
            changed = true;
            break; // runs are stale after a change — recompute from scratch.
        }
        if !changed {
            break;
        }
    }
}

/// Back-fill every still-unattributed (`None`) word into a neighbouring speaker.
///
/// `dominant_column` returns `None` for a word whose frame window carries no
/// activation in the diarizer's segmentation matrix — i.e. whisper heard a word
/// where the segmentation model saw no active speaker. This happens routinely at
/// turn boundaries and during overlaps, NOT only in real silence, so a `None`
/// word is almost always a genuinely-spoken word the geometry just missed.
///
/// Left untouched, such a word renders with no `[Speaker N]:` prefix AND splits
/// the surrounding turn in two (the transcript builder starts a fresh turn on any
/// speaker change, and `0`/unattributed counts as a change) — the orphaned-word
/// chop the user sees as "all chopped up". `smooth_word_speaker_runs` can't fix
/// it: it only ever rewrites `Some` runs and treats `None` as a gap.
///
/// So after smoothing we assign each `None` word the speaker it most likely
/// belongs to, using the surrounding attributed words as anchors (computed from
/// the pre-backfill columns, so the result is order-independent):
///
/// - bracketed by the SAME speaker on both sides → that speaker (a momentary
///   non-speech frame inside one continuous turn);
/// - at a hand-off (a DIFFERENT speaker each side) → the temporally nearest
///   neighbour (smallest inter-word gap), so the boundary word lands with whoever
///   it abuts;
/// - leading words (before the first attributed word) → the first speaker;
///   trailing words (after the last) → the last speaker.
///
/// No-op when no word is attributed at all (the caller's ≤1-speaker gate then
/// renders plain prose). Never introduces a new speaker column — it only ever
/// copies an existing neighbour's — so the speaker count is unchanged.
fn backfill_unattributed_words(words: &[&WordSpan], cols: &mut [Option<usize>]) {
    let n = cols.len();
    // Nearest attributed neighbour to the left of each index (carry forward).
    let mut left: Vec<Option<(usize, usize)>> = vec![None; n];
    let mut last: Option<(usize, usize)> = None;
    for i in 0..n {
        left[i] = last;
        if let Some(c) = cols[i] {
            last = Some((i, c));
        }
    }
    // Nearest attributed neighbour to the right (carry backward).
    let mut right: Vec<Option<(usize, usize)>> = vec![None; n];
    let mut next: Option<(usize, usize)> = None;
    for i in (0..n).rev() {
        right[i] = next;
        if let Some(c) = cols[i] {
            next = Some((i, c));
        }
    }
    for i in 0..n {
        if cols[i].is_some() {
            continue;
        }
        cols[i] = match (left[i], right[i]) {
            (Some((_, lc)), Some((_, rc))) if lc == rc => Some(lc),
            (Some((lj, lc)), Some((rj, rc))) => {
                let dl = (words[i].start - words[lj].end).abs();
                let dr = (words[rj].start - words[i].end).abs();
                Some(if dr < dl { rc } else { lc })
            }
            (Some((_, lc)), None) => Some(lc),
            (None, Some((_, rc))) => Some(rc),
            (None, None) => None,
        };
    }
}

/// Keep written words atomic across speaker attribution: a token that did NOT
/// start a new word — punctuation, a clitic (`'s`/`'t`), or a subword
/// continuation (`ste`/`pped`) — inherits the speaker of the word-start it
/// attaches to. A single written word can't have two speakers, so without this a
/// turn boundary that falls mid-word strands a `.` on the next speaker's turn or
/// splits `That's` across two (the "cut into each other" artifact the per-word
/// argmax produces at hand-offs). Applied left-to-right so a run of continuations
/// all chain back to their word-start's column. Word-start tokens
/// (`leading_space`) keep their own attribution; a leading continuation token
/// (index 0, no preceding word) is left as-is.
fn coalesce_subword_tokens(words: &[&WordSpan], cols: &mut [Option<usize>]) {
    for i in 1..words.len() {
        if !words[i].leading_space {
            cols[i] = cols[i - 1];
        }
    }
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
/// The production path now reads `DiarizationConfig::merge_gap_secs` (same 0.25
/// default); this constant remains as the fixed value the unit tests pin against.
#[cfg_attr(not(test), allow(dead_code))]
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
    fn load(cfg: &DiarizationConfig) -> Result<Self> {
        use speakrs::pipeline::{PipelineConfig, ReconstructMethod};
        use speakrs::{ExecutionMode, OwnedDiarizationPipeline};

        tracing::info!("loading local diarization pipeline (segmentation + embedding models)");
        let started = std::time::Instant::now();
        // A custom `[diarization].models_dir` loads a user-supplied bundle
        // (segmentation + embedding ONNX); empty (the default) downloads/uses the
        // pretrained models. The diarizer cache is keyed on the whole config, so
        // changing the dir reloads the pipeline.
        let dir = cfg.models_dir.trim();
        let pipeline = if dir.is_empty() {
            OwnedDiarizationPipeline::from_pretrained(ExecutionMode::Cpu)?
        } else {
            OwnedDiarizationPipeline::from_dir(dir, ExecutionMode::Cpu)?
        };

        // Map the user-facing knobs onto speakrs' PipelineConfig. The diarizer
        // cache is keyed on the whole DiarizationConfig, so changing any of these
        // in Settings drops the cached pipeline and reloads with the new values.
        let reconstruct_method = if cfg.reconstruct_method.eq_ignore_ascii_case("standard") {
            ReconstructMethod::Standard
        } else {
            ReconstructMethod::Smoothed {
                epsilon: cfg.reconstruct_method_epsilon as f32,
            }
        };
        let pc = PipelineConfig {
            merge_gap: cfg.merge_gap_secs,
            speaker_keep_threshold: cfg.speaker_keep_threshold,
            reconstruct_method,
            ..PipelineConfig::default()
        };
        let (sender, receiver) = pipeline.into_queued_with_config(pc)?;
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
    pub fn run(&self, audio_path: &Path) -> Result<LocalDiarization> {
        run_local_diarization(audio_path, &self.cache, &self.config)
    }
}

/// The full result of one local diarization run: the cleaned speaker turns the
/// transcript paths consume **plus** the raw model arrays a few of them need.
///
/// `spans` is the post-processed turn list (`clean_speaker_spans`) used by the
/// segment-level attribution path and the word-level fallback — unchanged from
/// what [`run_local_diarization`] used to return.
///
/// `discrete_diarization` is the per-frame activation matrix (frames × speakers,
/// one row per `FRAME_STEP_SECONDS`) that word-level attribution
/// ([`assign_words`]) sums over to pick each word's speaker. Column `k`
/// corresponds to label `SPEAKER_{k:02}`.
///
/// `embeddings`, `hard_clusters`, and `segmentations` are surfaced verbatim from
/// the speakrs result for a **future feature** (persistent named-speaker
/// voiceprints — "Cluster 5"): per-cluster embedding centroids are aggregated
/// from `embeddings` (chunks × speakers × dim) over the `(chunk, speaker)` cells
/// whose `hard_clusters` id matches and that are active in `segmentations`.
/// Nothing in the current word-level path reads them; they are carried here so
/// the return type isn't rewritten again when that feature lands.
#[derive(Debug, Clone)]
pub struct LocalDiarization {
    /// Cleaned, assignment-ready speaker turns (the segment-level path and the
    /// word-level fallback consume these).
    pub spans: Vec<SpeakerSpan>,
    /// Per-frame binary speaker activations, shape `(frames, speakers)`, one row
    /// per `FRAME_STEP_SECONDS`. Word-level attribution sums over this; column
    /// `k` maps to label `SPEAKER_{k:02}`.
    pub discrete_diarization: Array2<f32>,
    /// Per-chunk speaker embeddings, shape `(chunks, speakers, dim)`. Surfaced
    /// for the deferred named-speaker-voiceprint feature; unused today.
    pub embeddings: Array3<f32>,
    /// Per-chunk-speaker cluster ids, shape `(chunks, speakers)` (`-1` =
    /// unassigned). Surfaced for the deferred voiceprint feature; unused today.
    pub hard_clusters: Array2<i32>,
    /// Decoded powerset segmentations, shape `(chunks, frames, speakers)`.
    /// Tells which `(chunk, speaker)` cells are active when aggregating
    /// centroids. Surfaced for the deferred voiceprint feature; unused today.
    pub segmentations: Array3<f32>,
}

/// Two speaker clusters whose centroid voiceprints have at least this cosine
/// similarity are merged into one. speakrs' clustering (AHC seed → VBx) sometimes
/// over-splits a SINGLE voice into several clusters — a 2-person recording can
/// come back as 3 "speakers" — and the two fragments of one voice score far
/// higher against each other than two genuinely-different voices do. Calibrated
/// on real recordings: a same-voice over-split pair measured ~0.57 cosine, while
/// genuinely-different voices sat ~0.33–0.46, so 0.5 merges the former and keeps
/// the latter apart. (Distinct from `clean_speaker_spans`/smoothing, which fix
/// turn TIMING; this fixes the speaker COUNT.)
const SPEAKER_MERGE_COSINE: f32 = 0.5;

/// L2-normalized centroid embedding per speaker column (cluster id == column
/// index), aggregated from the per-`(chunk, speaker)` embeddings over the cells
/// whose `hard_clusters` id matches. `None` for a column with no finite
/// embeddings. `embeddings` is `(chunks, speakers, dim)`, `hard_clusters` is
/// `(chunks, speakers)`.
fn cluster_centroids(
    embeddings: &Array3<f32>,
    hard_clusters: &Array2<i32>,
    num_cols: usize,
) -> Vec<Option<Vec<f32>>> {
    let (chunks, speakers, dim) = embeddings.dim();
    let mut sums: Vec<(Vec<f64>, usize)> = vec![(vec![0.0; dim], 0); num_cols];
    for c in 0..chunks {
        for s in 0..speakers {
            let cid = hard_clusters[[c, s]];
            if cid < 0 || cid as usize >= num_cols {
                continue;
            }
            let e = embeddings.slice(ndarray::s![c, s, ..]);
            if !e.iter().all(|v| v.is_finite()) {
                continue;
            }
            let (sum, cnt) = &mut sums[cid as usize];
            for (i, v) in e.iter().enumerate() {
                sum[i] += *v as f64;
            }
            *cnt += 1;
        }
    }
    sums.into_iter()
        .map(|(sum, cnt)| {
            if cnt == 0 {
                return None;
            }
            let mut v: Vec<f32> = sum.iter().map(|x| (x / cnt as f64) as f32).collect();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v {
                    *x /= norm;
                }
            }
            Some(v)
        })
        .collect()
}

/// Map each speaker column to its canonical (merged) column via single-linkage
/// agglomerative merging on centroid cosine ≥ `threshold` (see
/// [`SPEAKER_MERGE_COSINE`]). The smallest column index in a merged group is the
/// canonical one (so first-appearance numbering stays sensible). Columns with no
/// centroid never merge. A no-op (identity map) when nothing is similar enough.
fn merge_similar_clusters(
    embeddings: &Array3<f32>,
    hard_clusters: &Array2<i32>,
    num_cols: usize,
    threshold: f32,
) -> Vec<usize> {
    let centroids = cluster_centroids(embeddings, hard_clusters, num_cols);
    let mut parent: Vec<usize> = (0..num_cols).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    for i in 0..num_cols {
        for j in (i + 1)..num_cols {
            if let (Some(ci), Some(cj)) = (&centroids[i], &centroids[j]) {
                let cos: f32 = ci.iter().zip(cj).map(|(a, b)| a * b).sum();
                if cos >= threshold {
                    let ri = find(&mut parent, i);
                    let rj = find(&mut parent, j);
                    if ri != rj {
                        let (keep, drop) = if ri < rj { (ri, rj) } else { (rj, ri) };
                        parent[drop] = keep;
                    }
                }
            }
        }
    }
    (0..num_cols).map(|c| find(&mut parent, c)).collect()
}

/// Parse a `SPEAKER_{k:02}` label back to its column index `k`.
fn parse_speaker_column(label: &str) -> Option<usize> {
    label.strip_prefix("SPEAKER_").and_then(|n| n.parse().ok())
}

/// Warm the local diarization pipeline into `cache` ahead of time — e.g. at
/// daemon startup when `[diarization].preload_at_startup` is on — so the first
/// diarized recording doesn't pay the multi-second, ~500 MB model load inline.
///
/// A no-op unless the backend is `Local` (only it loads models). Blocking and
/// allocation-heavy: call from `spawn_blocking`, never on the async runtime. A
/// load error is logged and swallowed (errors are never cached), so the next
/// real run simply retries the load.
pub fn preload_local_diarizer(cache: &LocalDiarizerCache, cfg: &DiarizationConfig) {
    if cfg.provider != crate::config::DiarizationBackend::Local {
        return;
    }
    match cache.get_or_load(cfg, || QueuedDiarizer::load(cfg)) {
        Ok(_) => tracing::info!("local diarization models preloaded at startup"),
        Err(e) => {
            tracing::warn!(error = %e, "diarization preload failed; will retry on first use")
        }
    }
}

/// Run local diarization on a 16 kHz mono WAV, returning the cleaned speaker
/// turns alongside the raw model arrays (see [`LocalDiarization`]). The pipeline
/// comes from `cache` — loaded on first use, then reused across recordings (the
/// per-call `from_pretrained` reload this replaced cost seconds and ~500 MB of
/// churn per transcription). Blocking for the whole inference; callers run it off
/// the async runtime (e.g. `spawn_blocking`).
pub fn run_local_diarization(
    audio_path: &Path,
    cache: &LocalDiarizerCache,
    cfg: &DiarizationConfig,
) -> Result<LocalDiarization> {
    // Decode the audio before touching the cache so a bad WAV fails fast
    // without costing (or being blamed on) a model load.
    let audio = load_audio_mono_16khz(audio_path)?;
    let pipeline = cache.get_or_load(cfg, || QueuedDiarizer::load(cfg))?;

    // The file id is only a label (speakrs uses it for RTTM/log output).
    let file_id = audio_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".to_string());

    let mut result = match pipeline.diarize(&file_id, audio) {
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

    // Collapse each frame to its single highest-scoring speaker before we read
    // the matrix for word-level attribution. The powerset model can mark a frame
    // active for several speakers at once (overlapping speech); making it
    // exclusive gives each frame one winner, so summing a word's frames over the
    // speaker columns yields a clean argmax. The `SPEAKER_{k:02}` column labels
    // are unchanged by this collapse, so word-level columns stay aligned with the
    // labels speakrs's `to_segments` emits into `result.segments`. (speakrs itself
    // runs `to_segments` on the reconstructed matrix WITHOUT `make_exclusive` — it
    // thresholds each speaker column independently at > 0.5 — so this collapse is
    // ours alone, for the per-word argmax, not a reproduction of how
    // `result.segments` was built.)
    result.discrete_diarization.make_exclusive();

    // Voiceprint merge: speakrs' clustering can over-split ONE voice into several
    // clusters (a 2-person recording returned as 3 "speakers"), which both
    // inflates the speaker count and chops a single speaker's turn as the model
    // flip-flops between that voice's fragments. Merge clusters whose centroid
    // voiceprints are similar enough to be the same voice (see
    // `SPEAKER_MERGE_COSINE`); genuinely-distinct voices stay separate. Fold each
    // merged column of the per-frame matrix into its canonical column and relabel
    // the segment spans, so BOTH word-level (argmax over the matrix) and
    // segment-level (overlap vs spans) attribution see the merged speakers.
    let num_cols = result.discrete_diarization.0.ncols();
    let canon = merge_similar_clusters(
        &result.embeddings.0,
        &result.hard_clusters.0,
        num_cols,
        SPEAKER_MERGE_COSINE,
    );
    if (0..num_cols).any(|c| canon[c] != c) {
        for (c, &p) in canon.iter().enumerate() {
            if p == c {
                continue;
            }
            let dropped = result.discrete_diarization.0.column(c).to_owned();
            {
                let mut keep = result.discrete_diarization.0.column_mut(p);
                keep += &dropped;
            }
            result.discrete_diarization.0.column_mut(c).fill(0.0);
        }
        for seg in result.segments.iter_mut() {
            if let Some(k) = parse_speaker_column(&seg.speaker) {
                if k < num_cols && canon[k] != k {
                    seg.speaker = column_label(canon[k]);
                }
            }
        }
        tracing::info!(
            from = num_cols,
            to = (0..num_cols).filter(|&c| canon[c] == c).count(),
            "voiceprint merge collapsed over-clustered speakers"
        );
    }

    // `result.segments` carries correctly-scaled (seconds) turns — that part of
    // the prior fix was right; the old `to_segments(1.0, 1.0)` had passed a frame
    // STEP/DURATION of 1.0 s against the model's real ~16.9 ms / ~61.9 ms geometry
    // and inflated every timestamp ~59×. But `result.segments` is NOT actually
    // merged (speakrs builds it with the default `merge_gap == 0.0`, a no-op), so
    // we coalesce same-speaker fragments ourselves before handing them off.
    let raw_spans: Vec<SpeakerSpan> = result
        .segments
        .iter()
        .map(|s| SpeakerSpan {
            start: s.start,
            end: s.end,
            label: s.speaker.clone(),
        })
        .collect();
    let spans = clean_speaker_spans(raw_spans, cfg.merge_gap_secs);

    Ok(LocalDiarization {
        spans,
        // Move the arrays out of their newtype wrappers (each derefs to the
        // inner ndarray); the `.0` is the owned `Array`.
        discrete_diarization: result.discrete_diarization.0,
        embeddings: result.embeddings.0,
        hard_clusters: result.hard_clusters.0,
        segmentations: result.segmentations.0,
    })
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
    fn word(start: f64, end: f64, text: &str) -> WordSpan {
        WordSpan {
            start,
            end,
            text: text.to_string(),
            leading_space: true,
        }
    }
    /// Like [`word`] but a non-word-start token (punctuation/clitic/subword).
    fn cont(start: f64, end: f64, text: &str) -> WordSpan {
        WordSpan {
            leading_space: false,
            ..word(start, end, text)
        }
    }
    /// Build (words, cols) from `(column, token_count)` runs, with realistic 0.3 s
    /// words back-to-back — for exercising the run-level smoothing thresholds.
    fn seq(spec: &[(usize, usize)]) -> (Vec<WordSpan>, Vec<Option<usize>>) {
        let mut words = Vec::new();
        let mut cols = Vec::new();
        let mut t = 0.0;
        for &(col, n) in spec {
            for _ in 0..n {
                words.push(word(t, t + 0.3, "x"));
                cols.push(Some(col));
                t += 0.3;
            }
        }
        (words, cols)
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

    // ── Track-aware Meeting Mode: fixed single-speaker labelling ──────────────

    #[test]
    fn label_all_as_wraps_every_segment_under_one_speaker() {
        // A meeting mic track is one voice: no diarizer runs, every segment is
        // stamped `[Speaker 1]` and the timeline carries speaker "1" so the
        // stored labels agree with the text markers.
        let segments = vec![
            seg(0.0, 1.5, "hello everyone"),
            seg(1.5, 3.25, "thanks for joining"),
        ];
        let (text, out) = label_all_as(&segments, 1);
        assert_eq!(text, "[Speaker 1]: hello everyone thanks for joining");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].start_ms, 0);
        assert_eq!(out[0].end_ms, 1500);
        assert_eq!(out[0].text, "hello everyone");
        assert_eq!(out[0].speaker.as_deref(), Some("1"));
        assert_eq!(out[1].start_ms, 1500);
        assert_eq!(out[1].end_ms, 3250);
        assert_eq!(out[1].text, "thanks for joining");
        assert_eq!(out[1].speaker.as_deref(), Some("1"));
    }

    #[test]
    fn label_all_as_skips_empty_segments() {
        // Mirror `empty_segments_are_skipped`: blank/whitespace segments are
        // dropped from both the text and the timeline, and the marker prefixes
        // the first REAL segment (not a leading blank one).
        let segments = vec![
            seg(0.0, 1.0, "   "),
            seg(1.0, 2.0, "real words"),
            seg(2.0, 3.0, "\t"),
        ];
        let (text, out) = label_all_as(&segments, 1);
        assert_eq!(text, "[Speaker 1]: real words");
        assert_eq!(out.len(), 1, "blank segments dropped from the timeline");
        assert_eq!(out[0].text, "real words");
        assert_eq!(out[0].speaker.as_deref(), Some("1"));
    }

    #[test]
    fn label_all_as_empty_input_yields_empty_output() {
        let (text, out) = label_all_as(&[], 1);
        assert!(text.is_empty());
        assert!(out.is_empty());
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

    // ── Per-word attribution from the frame-activation matrix ────────────────
    //
    // The real `discrete_diarization` matrix needs the speakrs models, so these
    // feed `assign_words` synthetic frames×speakers matrices. Each row is one
    // `frame_step`-long frame; column k is speaker `SPEAKER_{k:02}`. The frame
    // step is the same for the matrix rows and the word→frame mapping.

    use ndarray::array;

    /// Synthetic frame step; the real one is `speakrs::pipeline::FRAME_STEP_SECONDS`.
    const STEP: f64 = 0.05;
    /// Synthetic frame duration. speakrs centers frame `f` at `f*STEP + 0.5*DUR`;
    /// with `0.5*DUR == STEP` here, frame `f`'s center is the clean value
    /// `(f+1)*STEP`, and `frame_for_time(frame_mid(f)) == f`.
    const DUR: f64 = 0.10;
    /// Center of frame `f` (speakrs `frame_middle`): the time a word must sit at
    /// to map back to frame `f` via [`frame_for_time`].
    fn frame_mid(f: usize) -> f64 {
        f as f64 * STEP + 0.5 * DUR
    }

    #[test]
    fn frame_for_time_maps_to_the_covering_frame_center() {
        // round((t - 0.5*DUR) / STEP): a word at frame f's center maps back to f;
        // times at/below the first center clamp to row 0.
        for f in 0..6 {
            assert_eq!(frame_for_time(frame_mid(f), STEP, DUR), f);
        }
        assert_eq!(frame_for_time(0.0, STEP, DUR), 0);
        assert_eq!(frame_for_time(-1.0, STEP, DUR), 0);
    }

    #[test]
    fn frame_for_time_matches_speakrs_closest_frame_with_real_constants() {
        // Regression for the half-duration offset: against speakrs's real frame
        // geometry, t = 1.0 s is frame 57 — round((1.0 - 0.5*FRAME_DURATION)/STEP)
        // — NOT 59, the offset-free floor(1.0/STEP) the first cut produced.
        let step = speakrs::pipeline::FRAME_STEP_SECONDS;
        let dur = speakrs::pipeline::FRAME_DURATION_SECONDS;
        assert_eq!(frame_for_time(1.0, step, dur), 57);
        assert_eq!(frame_for_time(0.5, step, dur), 28);
        assert_eq!(frame_for_time(0.0, step, dur), 0);
    }

    #[test]
    fn column_label_matches_speakrs_to_segments_naming() {
        // The whole stable-index alignment hinges on this string matching the
        // label speakrs' `to_segments` emits for column k.
        assert_eq!(column_label(0), "SPEAKER_00");
        assert_eq!(column_label(1), "SPEAKER_01");
        assert_eq!(column_label(12), "SPEAKER_12");
    }

    #[test]
    fn each_word_lands_on_its_dominant_speaker_when_the_flip_is_mid_segment() {
        // Two speakers, six frames. Speaker 0 (column 0) owns frames 0–2, speaker
        // 1 (column 1) owns frames 3–5 — the flip is at frame 3 (t = 0.15 s), in
        // the MIDDLE of what a single whisper segment might span. Whole-segment
        // attribution would put the entire segment on one speaker; per-word
        // attribution splits it correctly.
        let m = array![
            [1.0, 0.0], // frame 0  [0.00,0.05)
            [1.0, 0.0], // frame 1  [0.05,0.10)
            [1.0, 0.0], // frame 2  [0.10,0.15)
            [0.0, 1.0], // frame 3  [0.15,0.20)
            [0.0, 1.0], // frame 4  [0.20,0.25)
            [0.0, 1.0], // frame 5  [0.25,0.30)
        ];
        let words = vec![
            word(frame_mid(0), frame_mid(1), "alpha"), // frames 0..=1 → speaker 0
            word(frame_mid(2), frame_mid(2), "beta"),  // frame 2      → speaker 0
            word(frame_mid(3), frame_mid(4), "gamma"), // frames 3..=4 → speaker 1
            word(frame_mid(5), frame_mid(5), "delta"), // frame 5      → speaker 1
        ];
        let (labeled, n) = assign_words(&words, &m, STEP, DUR, 0.0);
        assert_eq!(n, 2, "two distinct speakers used");
        let idxs: Vec<usize> = labeled.iter().map(|(_, i)| *i).collect();
        // First-appearance order: speaker 0 → index 1, speaker 1 → index 2.
        assert_eq!(idxs, vec![1, 1, 2, 2]);
    }

    #[test]
    fn boundary_straddling_word_goes_to_its_dominant_frames() {
        // The case whole-segment (and naive midpoint) attribution gets wrong: a
        // word straddling the hand-off. Speaker 0 owns frames 0–1, speaker 1 owns
        // frames 2–5. A word spanning [0.05, 0.29] covers frame 1 (spk 0) plus
        // frames 2,3,4,5 (spk 1): 1 frame vs 4 → speaker 1 wins on summed
        // activation, even though it starts inside speaker 0's region.
        let m = array![
            [1.0, 0.0], // 0
            [1.0, 0.0], // 1
            [0.0, 1.0], // 2
            [0.0, 1.0], // 3
            [0.0, 1.0], // 4
            [0.0, 1.0], // 5
        ];
        let words = vec![word(frame_mid(1), frame_mid(5), "straddle")]; // frames 1..=5
        let (labeled, n) = assign_words(&words, &m, STEP, DUR, 0.0);
        assert_eq!(n, 1);
        // Only one word, so it's the first-appearing speaker → index 1, but it is
        // speaker column 1 (the dominant one), not column 0 where it started.
        assert_eq!(labeled[0].1, 1);
        // Prove the dominance: the same word over a matrix where column 0 is
        // dominant instead would still map to index 1 (first appearance), so
        // assert the column directly via the helper.
        assert_eq!(dominant_column(&m, 1, 5), Some(1));
    }

    #[test]
    fn word_in_silence_is_unattributed() {
        // A word whose frames carry no activation (a gap in diarization) gets
        // index 0 and is excluded from the speaker count, mirroring the
        // segment-level `None`.
        let m = array![[0.0, 0.0], [1.0, 0.0], [0.0, 0.0]];
        let words = vec![
            word(frame_mid(1), frame_mid(1), "voiced"), // frame 1 → speaker 0
            word(frame_mid(2), frame_mid(2), "silent"), // frame 2 → all-zero → unattributed
        ];
        let (labeled, n) = assign_words(&words, &m, STEP, DUR, 0.0);
        assert_eq!(n, 1, "only the voiced word counts toward speaker count");
        assert_eq!(labeled[0].1, 1);
        assert_eq!(labeled[1].1, 0, "silent word is unattributed");
    }

    #[test]
    fn empty_words_are_skipped_like_empty_segments() {
        let m = array![[1.0, 0.0], [0.0, 1.0]];
        let words = vec![
            word(frame_mid(0), frame_mid(0), "  "), // skipped
            word(frame_mid(0), frame_mid(0), "a"),  // frame 0 → speaker 0
            word(frame_mid(1), frame_mid(1), "b"),  // frame 1 → speaker 1
        ];
        let (labeled, n) = assign_words(&words, &m, STEP, DUR, 0.0);
        assert_eq!(labeled.len(), 2, "the whitespace word is dropped");
        assert_eq!(n, 2);
        assert_eq!(labeled[0].0.text, "a");
        assert_eq!(labeled[1].0.text, "b");
    }

    #[test]
    fn argmax_ties_break_to_the_lowest_column() {
        // Equal activation across columns resolves deterministically to the
        // lowest-index column (the first-appearing speaker), never flickers.
        let m = array![[1.0, 1.0]];
        assert_eq!(dominant_column(&m, 0, 0), Some(0));
    }

    #[test]
    fn dominant_column_clamps_a_word_ending_past_the_last_frame() {
        // The final frame can end a hair before the last word's timestamp; the
        // frame index clamps to the last row rather than panicking.
        let m = array![[1.0, 0.0], [0.0, 1.0]];
        // end_frame 9 is well past row 1; clamp → consider rows [1,1] → speaker 1.
        assert_eq!(dominant_column(&m, 1, 9), Some(1));
    }

    #[test]
    fn empty_matrix_attributes_nothing() {
        let m: Array2<f32> = Array2::zeros((0, 0));
        let words = vec![word(frame_mid(0), frame_mid(1), "x")];
        let (labeled, n) = assign_words(&words, &m, STEP, DUR, 0.0);
        assert_eq!(n, 0);
        assert_eq!(labeled[0].1, 0, "no columns → unattributed");
    }

    // ── Word-turn smoothing (the "[Speaker 2]: it" regression guard) ─────────
    //
    // `smooth_word_speaker_runs` operates on a column sequence + the words' real
    // timings, so these tests hand it realistic-duration words directly (the
    // micro-second frame geometry of the tests above is orthogonal to it).

    /// The exact bug the user hit: a one-voice recording where a single short
    /// word ("it") momentarily scored to a second speaker. Smoothing absorbs the
    /// island back into the surrounding speaker, collapsing to one column — which
    /// makes the caller's ≤1-speaker gate render it as plain prose.
    #[test]
    fn lone_short_word_flip_is_absorbed_into_the_surrounding_speaker() {
        let words = [
            word(0.0, 0.4, "i"),
            word(0.4, 0.8, "really"),
            word(0.8, 1.0, "it"), // the 0.2 s flip
            word(1.0, 1.4, "think"),
            word(1.4, 1.9, "so"),
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), Some(0), Some(1), Some(0), Some(0)];
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert!(
            cols.iter().all(|c| *c == Some(0)),
            "the lone short flip is absorbed: {cols:?}"
        );
    }

    /// A genuine, sustained second-speaker turn (well over the threshold) is left
    /// alone — smoothing must not flatten real multi-speaker audio.
    #[test]
    fn sustained_second_speaker_turn_survives_smoothing() {
        let words = [
            word(0.0, 0.6, "hello"),
            word(0.6, 1.2, "there"), // spk0: 1.2 s
            word(1.2, 1.8, "hi"),
            word(1.8, 2.5, "back"),
            word(2.5, 3.2, "atcha"), // spk1: 2.0 s
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), Some(0), Some(1), Some(1), Some(1)];
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert_eq!(
            cols,
            vec![Some(0), Some(0), Some(1), Some(1), Some(1)],
            "balanced turns are untouched"
        );
    }

    /// A short flip between two different speakers goes to the LONGER neighbour.
    #[test]
    fn short_flip_is_absorbed_into_the_longer_neighbour() {
        let words = [
            word(0.0, 0.5, "a"),
            word(0.5, 1.0, "b"),  // spk0: 1.0 s
            word(1.0, 1.15, "x"), // spk1: 0.15 s flip
            word(1.15, 1.8, "c"),
            word(1.8, 2.5, "d"),
            word(2.5, 3.3, "e"), // spk2: 2.3 s (longer than spk0)
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), Some(0), Some(1), Some(2), Some(2), Some(2)];
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert_eq!(
            cols[2],
            Some(2),
            "flip absorbed into the longer (spk2) side"
        );
    }

    /// Smoothing bridges a silence: a flip surrounded by the same speaker across
    /// an unattributed (silence) word still collapses, and the silence stays.
    #[test]
    fn smoothing_bridges_silence_and_leaves_it_unattributed() {
        let words = [
            word(0.0, 0.6, "one"),
            word(0.6, 1.2, "two"),
            word(1.2, 1.35, "it"),  // short flip
            word(1.35, 1.6, "..."), // silence (None)
            word(1.6, 2.2, "three"),
            word(2.2, 2.8, "four"),
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), Some(0), Some(1), None, Some(0), Some(0)];
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert_eq!(
            cols,
            vec![Some(0), Some(0), Some(0), None, Some(0), Some(0)],
            "flip absorbed across the silence; the silence word stays None"
        );
    }

    /// A single speaker (one run, rest silence) has nothing to absorb into and is
    /// left untouched — smoothing never invents or drops the lone speaker.
    #[test]
    fn smoothing_leaves_a_single_speaker_alone() {
        let words = [
            word(0.0, 0.2, "a"),
            word(0.2, 0.45, "b"),
            word(0.45, 0.7, "c"),
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), None, Some(0)];
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert_eq!(cols, vec![Some(0), None, Some(0)], "lone speaker untouched");
    }

    // ── Unattributed-word back-fill (the orphaned-fragment chop guard) ───────
    //
    // After smoothing, a word the segmentation left `None` is assigned to a
    // neighbour so it never renders prefix-less and splits its turn in two.

    /// A `None` word inside one speaker's turn (a frame the segmentation missed)
    /// is back-filled to that speaker, so the turn stays one contiguous block
    /// instead of being broken by an orphaned, prefix-less word.
    #[test]
    fn backfill_fills_a_same_speaker_gap() {
        let words = [
            word(0.0, 0.5, "the"),
            word(0.5, 1.0, "fact"),
            word(1.0, 1.2, "that"), // segmentation saw no active speaker here
            word(1.2, 1.7, "women"),
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), Some(0), None, Some(0)];
        backfill_unattributed_words(&kept, &mut cols);
        assert_eq!(cols, vec![Some(0), Some(0), Some(0), Some(0)]);
    }

    /// A `None` word at a hand-off (a different speaker each side) goes to the
    /// temporally nearest neighbour — here the right speaker, which it abuts.
    #[test]
    fn backfill_sends_a_handoff_gap_to_the_nearest_speaker() {
        let words = [
            word(0.0, 0.6, "is"),
            word(0.6, 0.7, "a"),      // left speaker ends at 0.7
            word(2.0, 2.1, "weapon"), // None gap, abuts the right speaker
            word(2.1, 2.6, "i"),
            word(2.6, 3.1, "mean"), // right speaker
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), Some(0), None, Some(1), Some(1)];
        backfill_unattributed_words(&kept, &mut cols);
        assert_eq!(
            cols[2],
            Some(1),
            "the boundary word lands with the nearer (right) speaker"
        );
    }

    /// Leading words (before the first attributed word) attach to the first
    /// speaker; trailing words (after the last) attach to the last.
    #[test]
    fn backfill_attaches_leading_and_trailing_gaps_to_the_edges() {
        let words = [
            word(0.0, 0.3, "i"),
            word(0.3, 0.6, "don't"), // leading None
            word(0.6, 1.1, "know"),  // first speaker
            word(1.1, 1.6, "yeah"),  // last speaker
            word(1.6, 1.9, "you"),   // trailing None
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![None, None, Some(0), Some(1), None];
        backfill_unattributed_words(&kept, &mut cols);
        assert_eq!(cols, vec![Some(0), Some(0), Some(0), Some(1), Some(1)]);
    }

    /// With nothing attributed there is no anchor to copy — every word stays
    /// `None` and the caller's ≤1-speaker gate renders plain prose.
    #[test]
    fn backfill_with_no_anchor_is_a_noop() {
        let words = [word(0.0, 0.5, "a"), word(0.5, 1.0, "b")];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![None, None];
        backfill_unattributed_words(&kept, &mut cols);
        assert_eq!(cols, vec![None, None]);
    }

    /// A clitic / punctuation / subword token inherits its word-start's speaker,
    /// so a turn boundary never strands a "." on the next speaker or splits
    /// "That's" across two — the boundary "cut into each other" artifact.
    #[test]
    fn coalesce_pulls_continuations_into_their_word_start() {
        let words = [
            word(0.0, 0.5, "Yeah"), // word start → speaker 0
            cont(0.5, 0.6, "."),    // punctuation argmaxed to speaker 1
            word(0.6, 1.1, "It"),   // speaker 1's real turn starts
            cont(1.1, 1.2, "'s"),   // clitic argmaxed back to speaker 0
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![Some(0), Some(1), Some(1), Some(0)];
        coalesce_subword_tokens(&kept, &mut cols);
        assert_eq!(
            cols,
            vec![Some(0), Some(0), Some(1), Some(1)],
            "'.' joins 'Yeah' (spk0); \"'s\" joins 'It' (spk1)"
        );
    }

    /// A run of consecutive continuations all chain back to one word-start.
    #[test]
    fn coalesce_chains_a_run_of_continuations() {
        let words = [
            word(0.0, 0.4, "over"),
            cont(0.4, 0.5, "ste"),
            cont(0.5, 0.6, "pped"),
            cont(0.6, 0.7, "?"),
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        // Every continuation argmaxed to a different/noisy speaker.
        let mut cols = vec![Some(0), Some(1), Some(0), Some(1)];
        coalesce_subword_tokens(&kept, &mut cols);
        assert_eq!(
            cols,
            vec![Some(0), Some(0), Some(0), Some(0)],
            "all of 'overstepped?' is one speaker"
        );
    }

    /// The US-Govt opening: a 16-token island scored to speaker 1 sits inside a
    /// 31-token and a 144-token run of speaker 0 — over MAX_ISLAND_WORDS but
    /// dwarfed by the SAME speaker on both sides, so it's absorbed (a brief blip
    /// mid-monologue the diarizer mis-scored, not a real interjection).
    #[test]
    fn medium_island_dwarfed_by_same_speaker_is_absorbed() {
        let (words, mut cols) = seq(&[(0, 31), (1, 16), (0, 144)]);
        let kept: Vec<&WordSpan> = words.iter().collect();
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert!(
            cols.iter().all(|c| *c == Some(0)),
            "16-token island dwarfed by 31 & 144 same-speaker runs is absorbed: {cols:?}"
        );
    }

    /// A medium island that is NOT shorter than both neighbours (one side is
    /// comparable) is a real turn and survives.
    #[test]
    fn medium_island_not_dwarfed_on_both_sides_survives() {
        let (words, mut cols) = seq(&[(0, 12), (1, 16), (0, 144)]);
        let kept: Vec<&WordSpan> = words.iter().collect();
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert!(
            cols.contains(&Some(1)),
            "island not dwarfed on both sides survives"
        );
    }

    /// A large run (a genuine turn) between two longer same-speaker runs is never
    /// silently merged, even though it is shorter than both.
    #[test]
    fn large_bracketed_turn_survives_even_if_shorter_than_both() {
        let (words, mut cols) = seq(&[(0, 144), (1, 113), (0, 226)]);
        let kept: Vec<&WordSpan> = words.iter().collect();
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert_eq!(
            cols.iter().filter(|c| **c == Some(1)).count(),
            113,
            "a 113-token turn is above the bracketed ceiling and is never absorbed"
        );
    }

    /// The mid-sentence-flip regression: a MULTI-word run bracketed by the SAME
    /// speaker (a noise island inside one voice's continuous speech) is absorbed,
    /// even though every word is well over the 0.6s wall-clock threshold — so the
    /// word-count island rule, not the old span guard, does the work.
    #[test]
    fn multi_word_island_bracketed_by_same_speaker_is_absorbed() {
        let words = [
            word(0.0, 0.5, "respect"),
            word(0.5, 1.0, "the"),
            word(1.0, 1.5, "fact"),
            word(1.5, 2.0, "going"), // 4-word island start (each word 0.5 s)
            word(2.0, 2.5, "to"),
            word(2.5, 3.0, "do"),
            word(3.0, 3.5, "what"),
            word(3.5, 4.0, "they"), // island end
            word(4.0, 4.5, "want"),
            word(4.5, 5.0, "now"),
        ];
        let kept: Vec<&WordSpan> = words.iter().collect();
        // [0 0 0] [1 1 1 1] [0 0 0] — the four 1s span ~2 s, far above 0.6 s.
        let mut cols = vec![
            Some(0),
            Some(0),
            Some(0),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(0),
            Some(0),
            Some(0),
        ];
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert!(
            cols.iter().all(|c| *c == Some(0)),
            "the bracketed multi-word noise island is absorbed: {cols:?}"
        );
    }

    /// A genuinely long second-speaker run bracketed by another speaker (a real
    /// in-the-middle turn, longer than MAX_ISLAND_WORDS) is NOT absorbed — only
    /// short islands are flicker.
    #[test]
    fn long_bracketed_turn_above_island_max_survives() {
        let words: Vec<WordSpan> = (0..20)
            .map(|i| word(i as f64 * 0.5, (i as f64 + 1.0) * 0.5, "w"))
            .collect();
        let kept: Vec<&WordSpan> = words.iter().collect();
        // [0 ×3] [1 ×14] [0 ×3] — the 1-run is 14 words (> MAX_ISLAND_WORDS = 10).
        let mut cols: Vec<Option<usize>> = (0..20)
            .map(|i| {
                if (3..17).contains(&i) {
                    Some(1)
                } else {
                    Some(0)
                }
            })
            .collect();
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert!(
            cols[3..17].iter().all(|c| *c == Some(1)),
            "a long bracketed turn survives: {cols:?}"
        );
    }

    /// A real transition between two long turns (a DIFFERENT speaker each side,
    /// both long) is left intact — coherent two-speaker output, never over-merged.
    #[test]
    fn genuine_transition_between_two_long_turns_survives() {
        let words: Vec<WordSpan> = (0..12)
            .map(|i| word(i as f64 * 0.5, (i as f64 + 1.0) * 0.5, "w"))
            .collect();
        let kept: Vec<&WordSpan> = words.iter().collect();
        let mut cols = vec![
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
        ];
        let before = cols.clone();
        smooth_word_speaker_runs(&kept, &mut cols, WORD_MIN_TURN_SECS);
        assert_eq!(cols, before, "two genuine long turns are untouched");
    }

    /// Over-clustering fix: when speakrs splits one voice into two clusters
    /// (centroids very similar), they merge into the lowest-index canonical
    /// column; a genuinely-distinct third voice stays separate. Mirrors the real
    /// "US Government" recording (3 clusters, c1≈c2) collapsing to 2.
    #[test]
    fn voiceprint_merge_collapses_an_over_split_voice() {
        // cluster 0 = [1,0]; clusters 1,2 ≈ [0,1] (same voice). At 0.5: {1,2} merge.
        let embeddings =
            ndarray::Array3::from_shape_vec((3, 1, 2), vec![1.0, 0.0, 0.0, 1.0, 0.1, 0.99])
                .unwrap();
        let hard = ndarray::Array2::from_shape_vec((3, 1), vec![0, 1, 2]).unwrap();
        let canon = merge_similar_clusters(&embeddings, &hard, 3, 0.5);
        assert_eq!(
            canon,
            vec![0, 1, 1],
            "the over-split voice (c2) folds into c1"
        );
    }

    /// Genuinely-distinct voices (the real 'Preferences' 2-speaker case, ~0.32
    /// cosine) are NOT merged at the 0.5 threshold.
    #[test]
    fn voiceprint_merge_keeps_distinct_voices_separate() {
        let embeddings =
            ndarray::Array3::from_shape_vec((2, 1, 2), vec![1.0, 0.0, 0.32, 0.947]).unwrap();
        let hard = ndarray::Array2::from_shape_vec((2, 1), vec![0, 1]).unwrap();
        let canon = merge_similar_clusters(&embeddings, &hard, 2, 0.5);
        assert_eq!(canon, vec![0, 1], "two distinct voices stay separate");
    }

    /// Diagnostic (ignored): for each WAV in CAL_WAV1/CAL_WAV2, print speakrs'
    /// final speaker count + the pairwise cosine between per-cluster centroid
    /// voiceprints. Tells whether an over-clustered recording (N speakers for
    /// fewer real voices) has clusters similar enough to merge, and at what
    /// cosine. Run:
    ///   $env:CAL_WAV1="...us_govt.wav"; $env:CAL_WAV2="...prefs.wav";
    ///   cargo test -p phoneme-core diag_cluster_cosines -- --ignored --nocapture
    #[test]
    #[ignore = "manual diagnostic; needs the ~500MB speakrs models + CAL_WAV1/2"]
    fn diag_cluster_cosines() {
        use speakrs::{ExecutionMode, OwnedDiarizationPipeline};
        let wavs = [
            std::env::var("CAL_WAV1").unwrap_or_default(),
            std::env::var("CAL_WAV2").unwrap_or_default(),
        ];
        let mut pipeline =
            OwnedDiarizationPipeline::from_pretrained(ExecutionMode::Cpu).expect("load pipeline");
        for wav in wavs.iter().filter(|w| !w.is_empty()) {
            let audio = load_audio_mono_16khz(std::path::Path::new(wav)).expect("load wav");
            let cfg = pipeline.pipeline_config();
            let r = pipeline
                .run_with_config(&audio, "diag", &cfg)
                .expect("diarize");
            let speakers: std::collections::BTreeSet<String> =
                r.segments.iter().map(|s| s.speaker.clone()).collect();
            let emb = &r.embeddings.0;
            let hc = &r.hard_clusters.0;
            let (chunks, spk, dim) = emb.dim();
            let mut sums: std::collections::BTreeMap<i32, (Vec<f64>, usize)> =
                std::collections::BTreeMap::new();
            for c in 0..chunks {
                for s in 0..spk {
                    let cid = hc[[c, s]];
                    if cid < 0 {
                        continue;
                    }
                    let e = emb.slice(ndarray::s![c, s, ..]);
                    if !e.iter().all(|v| v.is_finite()) {
                        continue;
                    }
                    let ent = sums.entry(cid).or_insert_with(|| (vec![0.0; dim], 0));
                    for (i, v) in e.iter().enumerate() {
                        ent.0[i] += *v as f64;
                    }
                    ent.1 += 1;
                }
            }
            let cents: Vec<(i32, Vec<f64>)> = sums
                .iter()
                .map(|(cid, (s, n))| {
                    let mut v: Vec<f64> = s.iter().map(|x| x / *n as f64).collect();
                    let nrm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
                    if nrm > 0.0 {
                        for x in &mut v {
                            *x /= nrm;
                        }
                    }
                    (*cid, v)
                })
                .collect();
            eprintln!(
                "DIAG {wav}: segment_speakers={speakers:?} clusters={}",
                cents.len()
            );
            for i in 0..cents.len() {
                for j in i + 1..cents.len() {
                    let cos: f64 = cents[i].1.iter().zip(&cents[j].1).map(|(a, b)| a * b).sum();
                    eprintln!("DIAG   cos(c{}, c{}) = {cos:.3}", cents[i].0, cents[j].0);
                }
            }
        }
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
            ..DiarizationConfig::default()
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
