//! Wall-clock timeline alignment for meeting-mode dual-track recordings.

/// Pad or trim captured samples so a meeting track spans exactly
/// `target_duration_ms` on the shared meeting timeline.
///
/// `track_late_by_ms` inserts leading silence when this track's recorder started
/// after the meeting wall-clock start (e.g. the system loopback device opens
/// slightly after the mic). Trailing silence fills the remainder so both tracks
/// share the same total length — system audio stays time-aligned with speech.
pub fn align_meeting_track_samples(
    samples: Vec<i16>,
    track_late_by_ms: i64,
    target_duration_ms: i64,
    sample_rate: u32,
) -> Vec<i16> {
    let target_samples = ms_to_samples(target_duration_ms, sample_rate);
    let leading_pad = ms_to_samples(track_late_by_ms, sample_rate);

    let mut out = Vec::with_capacity(target_samples);
    if leading_pad > 0 {
        out.extend(std::iter::repeat_n(0i16, leading_pad.min(target_samples)));
    }
    out.extend_from_slice(&samples);

    if out.len() < target_samples {
        out.resize(target_samples, 0);
    } else if out.len() > target_samples {
        out.truncate(target_samples);
    }
    out
}

pub fn ms_to_samples(ms: i64, sample_rate: u32) -> usize {
    ((ms.max(0) as u64) * sample_rate as u64 / 1000) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_late_start_and_trailing_silence() {
        let sample_rate = 16_000;
        let target_ms = 40_000;
        let content_ms = 20_000;
        let content = vec![500i16; ms_to_samples(content_ms, sample_rate)];

        let aligned = align_meeting_track_samples(content, 10_000, target_ms, sample_rate);

        assert_eq!(aligned.len(), ms_to_samples(target_ms, sample_rate));
        assert!(aligned[..ms_to_samples(10_000, sample_rate)]
            .iter()
            .all(|&s| s == 0));
        assert!(aligned[ms_to_samples(10_000, sample_rate)..ms_to_samples(30_000, sample_rate)]
            .iter()
            .all(|&s| s == 500));
        assert!(aligned[ms_to_samples(30_000, sample_rate)..]
            .iter()
            .all(|&s| s == 0));
    }

    #[test]
    fn does_not_double_pad_continuous_silence() {
        let sample_rate = 16_000;
        let target_ms = 40_000;
        let content = vec![0i16; ms_to_samples(target_ms, sample_rate)];

        let aligned = align_meeting_track_samples(content, 0, target_ms, sample_rate);

        assert_eq!(aligned.len(), content.len());
        assert_eq!(aligned, content);
    }
}
