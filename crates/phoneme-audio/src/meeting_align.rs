//! Wall-clock timeline alignment for meeting-mode dual-track recordings.

/// Samples at or below this level are treated as silence.
pub const QUIET_THRESHOLD: i16 = 100;

/// Loopback capture is "sparse" when it is shorter than the capture window by
/// at least this much — a few ms of leading noise must not disable sparse mode.
const SPARSE_DEFICIT_MS: i64 = 500;

/// One track's raw capture, passed into [`align_meeting_tracks`].
#[derive(Debug, Clone)]
pub struct TrackAlignInput {
    /// This track's raw captured samples (canonical 16 kHz mono i16), exactly as
    /// the recorder buffered them — not yet placed on the shared timeline.
    pub samples: Vec<i16>,
    /// How long after the meeting's `wall_started` this track's recorder began
    /// capturing, in milliseconds. The track's audio is placed this far into the
    /// output so the two tracks line up at their true wall-clock offsets.
    pub track_late_by_ms: i64,
    /// Milliseconds from meeting `wall_started` when the first non-silent block
    /// arrived. `None` if the track stayed silent throughout.
    pub first_content_from_wall_ms: Option<i64>,
    /// Dense capture (the microphone) delivers a continuous buffer spanning the
    /// whole capture window, so it is always placed at the recorder start and is
    /// never relocated. Only non-dense capture (WASAPI loopback, which drops
    /// leading silence and hands back only the audible segment) is eligible for
    /// sparse first-content placement.
    pub dense: bool,
}

/// One aligned track: the output samples plus the placement decision, surfaced
/// so callers can log *why* a track landed where it did.
#[derive(Debug, Clone)]
pub struct AlignedTrack {
    /// Output samples, exactly `target_duration_ms` long, with this track's
    /// content placed at its wall-clock offset and the rest zero-filled silence.
    pub samples: Vec<i16>,
    /// `true` when the track was detected as sparse loopback capture and
    /// relocated to its wall-clock first-content instant.
    pub sparse: bool,
    /// Wall-clock offset (ms) the content was placed at in the output buffer.
    pub placement_ms: i64,
}

/// Align every meeting track to the same wall-clock timeline.
pub fn align_meeting_tracks(
    tracks: &[TrackAlignInput],
    target_duration_ms: i64,
    sample_rate: u32,
) -> Vec<AlignedTrack> {
    tracks
        .iter()
        .map(|t| {
            align_one_track(
                &t.samples,
                t.track_late_by_ms,
                target_duration_ms,
                t.first_content_from_wall_ms,
                t.dense,
                sample_rate,
            )
        })
        .collect()
}

/// Align a single track (tests and callers without peer context). Treated as
/// non-dense, but with `first_content_from_wall_ms = None` it can never be
/// classified as sparse, so it is always placed at `track_late_by_ms`.
pub fn align_meeting_track_samples(
    samples: Vec<i16>,
    track_late_by_ms: i64,
    target_duration_ms: i64,
    sample_rate: u32,
) -> Vec<i16> {
    align_one_track(
        &samples,
        track_late_by_ms,
        target_duration_ms,
        None,
        false,
        sample_rate,
    )
    .samples
}

fn align_one_track(
    raw: &[i16],
    track_late_by_ms: i64,
    target_duration_ms: i64,
    first_content_from_wall_ms: Option<i64>,
    dense: bool,
    sample_rate: u32,
) -> AlignedTrack {
    let target = ms_to_samples(target_duration_ms, sample_rate);
    if target == 0 {
        return AlignedTrack {
            samples: Vec::new(),
            sparse: false,
            placement_ms: 0,
        };
    }

    let capture_window_ms = (target_duration_ms - track_late_by_ms).max(0);
    let expected_raw = ms_to_samples(capture_window_ms, sample_rate);

    let mut samples = raw.to_vec();
    if samples.len() > expected_raw {
        samples.truncate(expected_raw);
    }

    let skip = leading_quiet_len(&samples);
    let deficit = expected_raw.saturating_sub(samples.len());
    let missing_capture = deficit > ms_to_samples(SPARSE_DEFICIT_MS, sample_rate);
    // Sub-threshold noise at the buffer head must not disqualify sparse loopback.
    let content_at_buffer_start = skip < ms_to_samples(200, sample_rate);
    let content_late_on_timeline =
        first_content_from_wall_ms.is_some_and(|t| t > track_late_by_ms + SPARSE_DEFICIT_MS);
    // Only loopback (non-dense) capture is eligible for sparse relocation; the
    // microphone is continuous and always stays at its recorder start, so it can
    // never be mis-detected as sparse no matter how it dropped blocks.
    let sparse = !dense && missing_capture && content_at_buffer_start && content_late_on_timeline;

    // Dense capture (mic): buffer spans the capture window, place at recorder start.
    // Sparse capture (loopback): only the audible segment arrived — place at the
    // wall-clock instant when non-silent audio first appeared.
    let (placement_ms, content) = if sparse {
        let placement_ms = first_content_from_wall_ms
            .unwrap_or(track_late_by_ms)
            .max(track_late_by_ms);
        (placement_ms, &samples[skip..])
    } else {
        (track_late_by_ms, samples.as_slice())
    };

    let mut out = vec![0i16; target];
    let start = ms_to_samples(placement_ms, sample_rate).min(target);
    let copy_len = content.len().min(target.saturating_sub(start));
    if copy_len > 0 {
        out[start..start + copy_len].copy_from_slice(&content[..copy_len]);
    }
    AlignedTrack {
        samples: out,
        sparse,
        placement_ms,
    }
}

fn leading_quiet_len(samples: &[i16]) -> usize {
    samples
        .iter()
        .take_while(|s| s.abs() <= QUIET_THRESHOLD)
        .count()
}

/// Convert a duration in milliseconds to a mono sample count at `sample_rate`
/// (Hz), rounding down. Negative input clamps to zero, so an offset that lands
/// before the timeline start yields no samples rather than wrapping.
pub fn ms_to_samples(ms: i64, sample_rate: u32) -> usize {
    ((ms.max(0) as u64) * sample_rate as u64 / 1000) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_track_placed_at_recorder_start() {
        let sample_rate = 16_000;
        let target_ms = 40_000;
        let content_ms = 20_000;
        let content = vec![500i16; ms_to_samples(content_ms, sample_rate)];

        let aligned = align_meeting_track_samples(content, 10_000, target_ms, sample_rate);

        assert_eq!(aligned.len(), ms_to_samples(target_ms, sample_rate));
        assert!(aligned[..ms_to_samples(10_000, sample_rate)]
            .iter()
            .all(|&s| s == 0));
        assert!(
            aligned[ms_to_samples(10_000, sample_rate)..ms_to_samples(30_000, sample_rate)]
                .iter()
                .all(|&s| s == 500)
        );
        assert!(aligned[ms_to_samples(30_000, sample_rate)..]
            .iter()
            .all(|&s| s == 0));
    }

    #[test]
    fn does_not_double_pad_continuous_silence() {
        let sample_rate = 16_000;
        let target_ms = 40_000;
        let content = vec![0i16; ms_to_samples(target_ms, sample_rate)];
        let len = content.len();

        let aligned = align_meeting_track_samples(content, 0, target_ms, sample_rate);

        assert_eq!(aligned.len(), len);
        assert!(aligned.iter().all(|&s| s == 0));
    }

    /// Video starts 30s into the meeting; loopback only captured the audible segment.
    #[test]
    fn sparse_system_track_placed_at_first_content_time() {
        let sample_rate = 16_000;
        let target_ms = 90_000;
        let intro_ms = 30_000;
        let video_ms = 60_000;
        let mic = vec![300i16; ms_to_samples(target_ms, sample_rate)];
        let video_audio = vec![700i16; ms_to_samples(video_ms, sample_rate)];

        let aligned = align_meeting_tracks(
            &[
                TrackAlignInput {
                    samples: mic,
                    track_late_by_ms: 0,
                    first_content_from_wall_ms: Some(0),
                    dense: true,
                },
                TrackAlignInput {
                    samples: video_audio,
                    track_late_by_ms: 0,
                    first_content_from_wall_ms: Some(intro_ms),
                    dense: false,
                },
            ],
            target_ms,
            sample_rate,
        );

        assert!(aligned[1].sparse, "loopback with 30s intro must be sparse");
        let sys = &aligned[1].samples;
        assert_eq!(sys.len(), ms_to_samples(target_ms, sample_rate));
        assert!(sys[..ms_to_samples(intro_ms, sample_rate)]
            .iter()
            .all(|&s| s == 0));
        assert!(sys[ms_to_samples(intro_ms, sample_rate)
            ..ms_to_samples(intro_ms + video_ms, sample_rate)]
            .iter()
            .all(|&s| s == 700));
    }

    /// Real-world case: 17.3s meeting, video at 4s, ~8.2s of system audio captured.
    #[test]
    fn user_scenario_video_at_four_seconds() {
        let sample_rate = 16_000;
        let target_ms = 17_300;
        let video_start_ms = 4_000;
        let mic = vec![300i16; ms_to_samples(target_ms, sample_rate)];
        let system_audio = vec![700i16; 131_050];

        let aligned = align_meeting_tracks(
            &[
                TrackAlignInput {
                    samples: mic,
                    track_late_by_ms: 3,
                    first_content_from_wall_ms: Some(0),
                    dense: true,
                },
                TrackAlignInput {
                    samples: system_audio,
                    track_late_by_ms: 4,
                    first_content_from_wall_ms: Some(video_start_ms),
                    dense: false,
                },
            ],
            target_ms,
            sample_rate,
        );

        let sys = &aligned[1].samples;
        let video_start = ms_to_samples(video_start_ms, sample_rate);
        assert!(
            sys[..video_start].iter().all(|&s| s == 0),
            "leading silence before video"
        );
        assert!(
            sys[video_start..video_start + 131_050]
                .iter()
                .all(|&s| s == 700),
            "video audio at 4s"
        );
        assert!(
            sys[video_start + 131_050..].iter().all(|&s| s == 0),
            "trailing silence after video"
        );
    }

    #[test]
    fn dense_track_with_leading_quiet_in_buffer() {
        let sample_rate = 16_000;
        let target_ms = 90_000;
        let mut samples = vec![0i16; ms_to_samples(30_000, sample_rate)];
        samples.extend(vec![700i16; ms_to_samples(30_000, sample_rate)]);

        let aligned = align_meeting_track_samples(samples, 0, target_ms, sample_rate);

        assert_eq!(aligned.len(), ms_to_samples(target_ms, sample_rate));
        assert!(aligned[..ms_to_samples(30_000, sample_rate)]
            .iter()
            .all(|&s| s == 0));
        assert!(
            aligned[ms_to_samples(30_000, sample_rate)..ms_to_samples(60_000, sample_rate)]
                .iter()
                .all(|&s| s == 700)
        );
        assert!(aligned[ms_to_samples(60_000, sample_rate)..]
            .iter()
            .all(|&s| s == 0));
    }

    /// Loopback with a few ms of sub-threshold noise at the buffer start must still
    /// be placed at `first_content_from_wall_ms`, not at t=0.
    #[test]
    fn sparse_with_leading_subthreshold_noise_placed_at_first_content() {
        let sample_rate = 16_000;
        let target_ms = 18_983;
        let video_start_ms = 6_826;
        let mut system = vec![0i16; 1_100];
        system.extend(vec![700i16; 101_012 - 1_100]);

        let aligned = align_meeting_tracks(
            &[TrackAlignInput {
                samples: system,
                track_late_by_ms: 1,
                first_content_from_wall_ms: Some(video_start_ms),
                dense: false,
            }],
            target_ms,
            sample_rate,
        );

        assert!(aligned[0].sparse, "064846 loopback must be detected sparse");
        assert_eq!(aligned[0].placement_ms, video_start_ms);
        let sys = &aligned[0].samples;
        let video_start = ms_to_samples(video_start_ms, sample_rate);
        assert!(
            sys[..video_start].iter().all(|&s| s == 0),
            "leading silence before video"
        );
        assert!(
            sys[video_start..video_start + 800].contains(&700),
            "video audio at wall-clock start"
        );
        let loud_before = sys[..video_start]
            .iter()
            .filter(|&&s| s.abs() > 100)
            .count();
        assert_eq!(loud_before, 0, "no audio before video start");
    }

    /// A loopback track that *did* deliver continuous frames (only a small
    /// deficit) with internal leading silence must NOT be treated as sparse —
    /// the in-buffer silence already aligns it, so it stays at `track_late`.
    #[test]
    fn dense_loopback_with_internal_leading_silence_not_sparse() {
        let sample_rate = 16_000;
        let target_ms = 60_000;
        // 30s of silence then 30s of audio, captured nearly continuously.
        let mut system = vec![0i16; ms_to_samples(30_000, sample_rate)];
        system.extend(vec![700i16; ms_to_samples(30_000, sample_rate)]);

        let aligned = align_meeting_tracks(
            &[TrackAlignInput {
                samples: system,
                track_late_by_ms: 1,
                // First content was detected late, but the buffer is dense.
                first_content_from_wall_ms: Some(30_000),
                dense: false,
            }],
            target_ms,
            sample_rate,
        );

        assert!(
            !aligned[0].sparse,
            "continuous loopback (no big deficit / long in-buffer silence) is not sparse"
        );
        let sys = &aligned[0].samples;
        // Content stays at track_late (~1 ms), so the 30s of in-buffer silence
        // still lands at ~30s — internal silence is preserved, not stripped.
        assert!(sys[..ms_to_samples(29_000, sample_rate)]
            .iter()
            .all(|&s| s == 0));
        assert!(sys[ms_to_samples(31_000, sample_rate)..]
            .iter()
            .all(|&s| s == 700));
    }

    /// The microphone is dense by definition: even if it dropped enough blocks
    /// to look short AND the user stayed quiet at the start, it must never be
    /// relocated to its first-content instant (that would shift the whole track).
    #[test]
    fn mic_never_sparse_even_with_deficit_and_late_content() {
        let sample_rate = 16_000;
        let target_ms = 30_000;
        // Only 5s of audio survived (huge deficit vs a 30s window), and it's
        // loud right from the buffer head — the shape that would trip the
        // sparse heuristic for a loopback track.
        let mic = vec![600i16; ms_to_samples(5_000, sample_rate)];

        let aligned = align_meeting_tracks(
            &[TrackAlignInput {
                samples: mic,
                track_late_by_ms: 2,
                first_content_from_wall_ms: Some(10_000),
                dense: true,
            }],
            target_ms,
            sample_rate,
        );

        assert!(!aligned[0].sparse, "mic must never be classified sparse");
        assert_eq!(aligned[0].placement_ms, 2);
        let mic_out = &aligned[0].samples;
        // Content sits at the recorder start (~t=0, offset by track_late=2ms),
        // not relocated to the 10s first-content instant.
        assert!(
            mic_out[ms_to_samples(1_000, sample_rate)..ms_to_samples(5_000, sample_rate)]
                .iter()
                .all(|&s| s == 600)
        );
        assert!(mic_out[ms_to_samples(6_000, sample_rate)..]
            .iter()
            .all(|&s| s == 0));
    }

    #[test]
    fn truncates_excess_from_end_of_capture_window() {
        let sample_rate = 16_000;
        let target_ms = 10_000;
        let expected_raw = ms_to_samples(target_ms, sample_rate);
        let long = vec![400i16; expected_raw + 800];

        let aligned = align_meeting_track_samples(long, 0, target_ms, sample_rate);

        assert_eq!(aligned.len(), expected_raw);
        assert!(aligned.iter().all(|&s| s == 400));
    }
}
