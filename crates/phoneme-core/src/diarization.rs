//! Local speaker diarization (pyannote-style segmentation via `speakrs`) and the
//! provider-agnostic logic that attaches speaker labels to a transcript.
//!
//! The actual model inference lives in [`run_local_diarization`]; the pure
//! [`assign_speakers`] function (which maps speaker turns onto ASR segments) is
//! deliberately model-free so it can be unit-tested without the ~500 MB ONNX
//! model present.

use anyhow::Result;
use std::path::Path;

/// A speaker turn produced by the diarizer: `[start, end)` in **seconds** and an
/// opaque speaker label. We do NOT assume the label is a bare integer — pyannote
/// emits `"SPEAKER_00"`-style strings, so we treat it as an arbitrary key and
/// map it to a stable 1-based index ourselves.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerSpan {
    pub start: f64,
    pub end: f64,
    pub label: String,
}

/// One ASR transcript segment: `[start, end)` in **seconds** plus its text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSegment {
    pub start: f64,
    pub end: f64,
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
/// - Each segment is attributed to the speaker turn it overlaps most (see
///   [`speaker_for_segment`]); one falling in a gap between turns goes to the
///   nearest turn, never a default speaker 0.
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

/// Run local diarization on a 16 kHz mono WAV, returning speaker turns. The
/// model is loaded on each call (CPU execution for portability); callers should
/// run this off the async runtime (e.g. `spawn_blocking`).
pub fn run_local_diarization(audio_path: &Path) -> Result<Vec<SpeakerSpan>> {
    use speakrs::{ExecutionMode, OwnedDiarizationPipeline};

    let mut pipeline = OwnedDiarizationPipeline::from_pretrained(ExecutionMode::Cpu)?;
    let audio = load_audio_mono_16khz(audio_path)?;
    let result = pipeline.run(&audio)?;
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
}
