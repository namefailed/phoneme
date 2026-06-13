//! Caption export: turn a recording's transcript segments into SRT / WebVTT.
//!
//! Both formats share the same timing model — millisecond-precision cues over
//! a `&[TranscriptSegment]` slice — but differ in header and separator syntax.
//! Speaker labels, when present and non-empty, are prepended as `Name: ` so
//! screen-reader and subtitle tooling can attribute lines correctly.

use crate::types::TranscriptSegment;

// ── timestamp helpers ─────────────────────────────────────────────────────────

/// Format milliseconds as `HH:MM:SS,mmm` (SRT separator).
fn fmt_srt(ms: i64) -> String {
    let total_s = ms / 1000;
    let millis = ms % 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    format!("{h:02}:{m:02}:{s:02},{millis:03}")
}

/// Format milliseconds as `HH:MM:SS.mmm` (WebVTT separator).
fn fmt_vtt(ms: i64) -> String {
    let total_s = ms / 1000;
    let millis = ms % 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    format!("{h:02}:{m:02}:{s:02}.{millis:03}")
}

/// Prepend the speaker label when the segment carries a non-empty one.
fn cue_text(seg: &TranscriptSegment) -> String {
    match seg.speaker.as_deref() {
        Some(sp) if !sp.trim().is_empty() => format!("{}: {}", sp.trim(), seg.text),
        _ => seg.text.clone(),
    }
}

// ── public API ────────────────────────────────────────────────────────────────

/// Render segments as an SRT subtitle file (UTF-8, LF line endings).
///
/// Empty input produces an empty string (valid, zero-cue SRT). Cue indices are
/// 1-based. Hours pad to at least two digits and keep counting past 99.
///
/// ```
/// use phoneme_core::export::segments_to_srt;
/// use phoneme_core::TranscriptSegment;
/// let segs = [TranscriptSegment {
///     start_ms: 1000,
///     end_ms: 4500,
///     text: "Hello world.".into(),
///     speaker: None,
/// }];
/// assert_eq!(segments_to_srt(&segs), "1\n00:00:01,000 --> 00:00:04,500\nHello world.\n");
/// ```
pub fn segments_to_srt(segments: &[TranscriptSegment]) -> String {
    if segments.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        let idx = i + 1;
        let start = fmt_srt(seg.start_ms);
        let end = fmt_srt(seg.end_ms);
        out.push_str(&idx.to_string());
        out.push('\n');
        out.push_str(&start);
        out.push_str(" --> ");
        out.push_str(&end);
        out.push('\n');
        out.push_str(&cue_text(seg));
        out.push('\n');
        // Blank line separates cues; omit trailing blank after the last one.
        if i + 1 < segments.len() {
            out.push('\n');
        }
    }
    out
}

/// Render segments as a WebVTT subtitle file (UTF-8, LF line endings).
///
/// Empty input produces just the mandatory `WEBVTT` header line (plus one blank
/// line after it). Hours pad to at least two digits and keep counting past 99.
pub fn segments_to_vtt(segments: &[TranscriptSegment]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for seg in segments {
        let start = fmt_vtt(seg.start_ms);
        let end = fmt_vtt(seg.end_ms);
        out.push_str(&start);
        out.push_str(" --> ");
        out.push_str(&end);
        out.push('\n');
        out.push_str(&cue_text(seg));
        out.push('\n');
        out.push('\n');
    }
    out
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start_ms: i64, end_ms: i64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start_ms,
            end_ms,
            text: text.to_string(),
            speaker: None,
        }
    }

    fn seg_speaker(start_ms: i64, end_ms: i64, text: &str, sp: &str) -> TranscriptSegment {
        TranscriptSegment {
            start_ms,
            end_ms,
            text: text.to_string(),
            speaker: Some(sp.to_string()),
        }
    }

    // ── SRT ──

    #[test]
    fn srt_empty_input_is_empty_string() {
        assert_eq!(segments_to_srt(&[]), "");
    }

    #[test]
    fn srt_single_cue() {
        let segs = [seg(1000, 4500, "Hello world.")];
        let out = segments_to_srt(&segs);
        assert_eq!(out, "1\n00:00:01,000 --> 00:00:04,500\nHello world.\n");
    }

    #[test]
    fn srt_multiple_cues_separated_by_blank_line() {
        let segs = [seg(0, 2000, "First."), seg(2500, 5000, "Second.")];
        let out = segments_to_srt(&segs);
        assert_eq!(
            out,
            "1\n00:00:00,000 --> 00:00:02,000\nFirst.\n\n\
             2\n00:00:02,500 --> 00:00:05,000\nSecond.\n"
        );
    }

    #[test]
    fn srt_speaker_prefix() {
        let segs = [seg_speaker(0, 1000, "Hi.", "Alice")];
        let out = segments_to_srt(&segs);
        assert!(
            out.contains("Alice: Hi."),
            "expected 'Alice: Hi.' in:\n{out}"
        );
    }

    #[test]
    fn srt_empty_speaker_not_prefixed() {
        let mut s = seg(0, 1000, "Hi.");
        s.speaker = Some(String::new());
        let out = segments_to_srt(&[s]);
        // Text line must be bare — no "Name: " prefix.  The timestamp lines
        // legitimately contain colons, so we check only the final text line.
        assert!(out.ends_with("Hi.\n"));
        let text_line = out.lines().last().unwrap_or("");
        assert!(
            !text_line.contains(':'),
            "unexpected colon in text line: {text_line:?}"
        );
    }

    #[test]
    fn srt_over_one_hour() {
        let segs = [seg(3_661_042, 3_665_000, "Late cue.")];
        let out = segments_to_srt(&segs);
        assert!(
            out.contains("01:01:01,042 --> 01:01:05,000"),
            "unexpected timestamp in:\n{out}"
        );
    }

    #[test]
    fn srt_millisecond_zero_padding() {
        // 500 ms → ,500; 50 ms → ,050; 5 ms → ,005
        assert!(segments_to_srt(&[seg(500, 1050, "a")]).contains("00:00:00,500 --> 00:00:01,050"));
        assert!(segments_to_srt(&[seg(50, 105, "a")]).contains("00:00:00,050 --> 00:00:00,105"));
        assert!(segments_to_srt(&[seg(5, 1005, "a")]).contains("00:00:00,005 --> 00:00:01,005"));
    }

    // ── VTT ──

    #[test]
    fn vtt_empty_input_is_header_only() {
        let out = segments_to_vtt(&[]);
        assert_eq!(out, "WEBVTT\n\n");
    }

    #[test]
    fn vtt_single_cue() {
        let segs = [seg(1000, 4500, "Hello world.")];
        let out = segments_to_vtt(&segs);
        assert_eq!(
            out,
            "WEBVTT\n\n00:00:01.000 --> 00:00:04.500\nHello world.\n\n"
        );
    }

    #[test]
    fn vtt_multiple_cues() {
        let segs = [seg(0, 2000, "First."), seg(2500, 5000, "Second.")];
        let out = segments_to_vtt(&segs);
        assert_eq!(
            out,
            "WEBVTT\n\n\
             00:00:00.000 --> 00:00:02.000\nFirst.\n\n\
             00:00:02.500 --> 00:00:05.000\nSecond.\n\n"
        );
    }

    #[test]
    fn vtt_speaker_prefix() {
        let segs = [seg_speaker(0, 1000, "Hi.", "Bob")];
        let out = segments_to_vtt(&segs);
        assert!(out.contains("Bob: Hi."), "expected 'Bob: Hi.' in:\n{out}");
    }

    #[test]
    fn vtt_over_one_hour() {
        let segs = [seg(3_661_042, 3_665_000, "Late cue.")];
        let out = segments_to_vtt(&segs);
        assert!(
            out.contains("01:01:01.042 --> 01:01:05.000"),
            "unexpected timestamp in:\n{out}"
        );
    }

    #[test]
    fn vtt_millisecond_zero_padding() {
        assert!(segments_to_vtt(&[seg(500, 1050, "a")]).contains("00:00:00.500 --> 00:00:01.050"));
        assert!(segments_to_vtt(&[seg(50, 105, "a")]).contains("00:00:00.050 --> 00:00:00.105"));
        assert!(segments_to_vtt(&[seg(5, 1005, "a")]).contains("00:00:00.005 --> 00:00:01.005"));
    }
}
