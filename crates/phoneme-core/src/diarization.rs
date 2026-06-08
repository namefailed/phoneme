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

/// The label of the speaker active at instant `t`: the span covering `t`, or —
/// when `t` falls in a gap between turns — the *nearest* span. Returns `None`
/// only when there are no speaker spans at all. Picking the nearest span (rather
/// than defaulting to "speaker 0") keeps a line that lands just outside a turn
/// attributed to the most plausible speaker instead of a phantom one.
fn speaker_at(speakers: &[SpeakerSpan], t: f64) -> Option<&str> {
    if speakers.is_empty() {
        return None;
    }
    if let Some(covering) = speakers.iter().find(|s| t >= s.start && t <= s.end) {
        return Some(&covering.label);
    }
    speakers
        .iter()
        .min_by(|a, b| {
            interval_distance(a, t)
                .partial_cmp(&interval_distance(b, t))
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
/// - A segment whose midpoint falls in a gap between turns is attributed to the
///   nearest turn (see [`speaker_at`]), never a default speaker 0.
/// - Empty/whitespace segments are skipped.
///
/// When `speakers` is empty (diarization produced nothing) the segments are
/// joined into plain text with no speaker prefixes and a speaker count of 0.
pub fn assign_speakers(segments: &[TextSegment], speakers: &[SpeakerSpan]) -> (String, usize) {
    use std::collections::HashMap;

    let mut label_to_idx: HashMap<&str, usize> = HashMap::new();
    let mut next_idx = 1usize;
    let mut out = String::new();
    let mut current: Option<usize> = None;

    for seg in segments {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        let midpoint = seg.start + (seg.end - seg.start) / 2.0;
        let idx = match speaker_at(speakers, midpoint) {
            Some(label) => *label_to_idx.entry(label).or_insert_with(|| {
                let i = next_idx;
                next_idx += 1;
                i
            }),
            None => 0, // no diarization info at all → unlabeled, plain text
        };

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
        out.push_str(text);
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

/// Run local diarization on a 16 kHz mono WAV, returning speaker turns. The
/// model is loaded on each call (CPU execution for portability); callers should
/// run this off the async runtime (e.g. `spawn_blocking`).
pub fn run_local_diarization(audio_path: &Path) -> Result<Vec<SpeakerSpan>> {
    use speakrs::{ExecutionMode, OwnedDiarizationPipeline};

    let mut pipeline = OwnedDiarizationPipeline::from_pretrained(ExecutionMode::Cpu)?;
    let audio = load_audio_mono_16khz(audio_path)?;
    let result = pipeline.run(&audio)?;
    let segments = result.discrete_diarization.to_segments(1.0, 1.0);

    Ok(segments
        .into_iter()
        .map(|s| SpeakerSpan {
            start: s.start,
            end: s.end,
            label: s.speaker,
        })
        .collect())
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
}
