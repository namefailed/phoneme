//! WAV encoding and decoding via `hound`.
//!
//! All Phoneme recordings are 16-bit PCM. This module deals only with that
//! format — `hound::WavWriter`/`WavReader` handle the on-disk byte layout.

use crate::format::{AudioConfig, Channels, SampleRate};
use phoneme_core::error::{Error, Result};
use std::path::Path;

/// Write a buffer of `i16` samples to a WAV file atomically.
///
/// The data is written to a `.tmp` sibling, finalized, and then moved into
/// the destination in a single rename — so a crash or power loss mid-write
/// never leaves a truncated WAV at `path`. On Windows, [`std::fs::rename`]
/// fails when the destination already exists, so the destination is removed
/// first if present.
///
/// Parent directories are created if missing.
pub fn write_wav(path: &Path, samples: &[i16], cfg: AudioConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // Derive the temp path in the same directory so the rename is always on the
    // same filesystem (cross-device renames are not atomic and are not needed
    // here — a WAV is never written across mount points).
    let tmp_path = path.with_extension("wav.tmp");

    let spec = hound::WavSpec {
        channels: cfg.channels.as_u8() as u16,
        sample_rate: cfg.sample_rate.as_u32(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&tmp_path, spec)
        .map_err(|e| Error::Internal(format!("hound create tmp: {e}")))?;
    for &s in samples {
        writer
            .write_sample(s)
            .map_err(|e| Error::Internal(format!("hound write_sample: {e}")))?;
    }
    writer
        .finalize()
        .map_err(|e| Error::Internal(format!("hound finalize: {e}")))?;

    // Windows rename fails when the destination exists; remove it first.
    // The window between remove and rename is vanishingly small and both
    // files are on the same volume, so this is the accepted idiom here.
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| Error::Internal(format!("remove existing wav before replace: {e}")))?;
    }
    std::fs::rename(&tmp_path, path)
        .map_err(|e| Error::Internal(format!("rename tmp to final wav: {e}")))?;
    Ok(())
}

/// Read a WAV file as `(samples, config)`. The returned config reflects the
/// file's actual format, not Phoneme's default.
pub fn read_wav(path: &Path) -> Result<(Vec<i16>, AudioConfig)> {
    let reader = hound::WavReader::open(path).map_err(|e| match e {
        hound::Error::IoError(io) => Error::Io(io),
        other => Error::Internal(format!("hound open: {other}")),
    })?;
    let spec = reader.spec();
    let cfg = AudioConfig {
        sample_rate: SampleRate(spec.sample_rate),
        channels: Channels(spec.channels as u8),
    };
    let samples: Vec<i16> = reader
        .into_samples::<i16>()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::Internal(format!("hound read: {e}")))?;
    Ok((samples, cfg))
}

/// Read just the duration of a WAV file in milliseconds (cheap — no sample data).
pub fn duration_ms(path: &Path) -> Result<i64> {
    let reader =
        hound::WavReader::open(path).map_err(|e| Error::Internal(format!("hound open: {e}")))?;
    let spec = reader.spec();
    let frames = reader.len() as u64 / spec.channels.max(1) as u64;
    let ms = (frames * 1000) / spec.sample_rate.max(1) as u64;
    Ok(ms as i64)
}

/// Slice a `[start_ms, end_ms)` time range out of a decoded WAV and write it to
/// `out_path` with the source's [`AudioConfig`] — the pure core of audio clip
/// export (S7). Reads `src`, converts the millisecond bounds to sample frames by
/// the source's sample rate (channel-aware: a frame is one sample per channel,
/// so the slice never lands mid-frame on stereo), and writes the cut.
///
/// Validation: `start_ms < end_ms`, both non-negative, and `start_ms` strictly
/// inside the recording. `end_ms` is **clamped** to the recording's duration
/// (asking for more than exists yields the tail, not an error). An empty
/// resulting range — `start_ms` at or past the end — is an error rather than a
/// zero-sample WAV. Returns the number of sample frames written.
///
/// Kept pure and filesystem-only so it unit-tests without a daemon: the daemon
/// handler resolves the recording's path and the default output path, then calls
/// this.
pub fn clip_wav(src: &Path, out_path: &Path, start_ms: i64, end_ms: i64) -> Result<usize> {
    if start_ms < 0 || end_ms < 0 {
        return Err(Error::InvalidConfig(format!(
            "clip range must be non-negative (got start={start_ms}ms, end={end_ms}ms)"
        )));
    }
    if start_ms >= end_ms {
        return Err(Error::InvalidConfig(format!(
            "clip start must be before end (got start={start_ms}ms, end={end_ms}ms)"
        )));
    }

    let (samples, cfg) = read_wav(src)?;
    let channels = cfg.channels.as_u8().max(1) as usize;
    let sample_rate = cfg.sample_rate.as_u32().max(1) as i64;
    let total_frames = samples.len() / channels;

    // Frame index = ms * rate / 1000, computed in i64 to avoid overflow on long
    // recordings before clamping back into the buffer.
    let start_frame = ((start_ms * sample_rate) / 1000).max(0) as usize;
    let end_frame_unclamped = ((end_ms * sample_rate) / 1000).max(0) as usize;
    // Clamp end to the recording; an over-long request yields the tail.
    let end_frame = end_frame_unclamped.min(total_frames);

    if start_frame >= end_frame {
        return Err(Error::InvalidConfig(format!(
            "clip range is empty: start={start_ms}ms is at or past the recording end ({}ms)",
            (total_frames as i64 * 1000) / sample_rate
        )));
    }

    let slice = &samples[start_frame * channels..end_frame * channels];
    write_wav(out_path, slice, cfg)?;
    Ok(end_frame - start_frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Channels, SampleRate};

    fn test_cfg() -> AudioConfig {
        AudioConfig {
            sample_rate: SampleRate(16_000),
            channels: Channels(1),
        }
    }

    #[test]
    fn write_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let samples: Vec<i16> = (0i16..64).collect();
        write_wav(&path, &samples, test_cfg()).unwrap();
        let (got, cfg) = read_wav(&path).unwrap();
        assert_eq!(got, samples);
        assert_eq!(cfg.sample_rate.as_u32(), 16_000);
        assert_eq!(cfg.channels.as_u8(), 1);
    }

    /// Atomic-write guarantee: the tmp file must not survive a successful
    /// write, and the final path must contain valid audio — even when the
    /// destination already exists (the overwrite path on Windows).
    #[test]
    fn atomic_write_no_tmp_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.wav");
        let cfg = test_cfg();
        let samples: Vec<i16> = vec![1, 2, 3, 4, 5];

        // First write — no prior destination.
        write_wav(&path, &samples, cfg).unwrap();
        assert!(path.exists(), "final file must exist after first write");
        let tmp = path.with_extension("wav.tmp");
        assert!(!tmp.exists(), "tmp file must be removed after rename");

        // Second write — destination already exists (the overwrite branch).
        let samples2: Vec<i16> = vec![10, 20, 30];
        write_wav(&path, &samples2, cfg).unwrap();
        assert!(path.exists(), "final file must still exist after overwrite");
        assert!(!tmp.exists(), "tmp must be removed after second rename");

        let (got, _) = read_wav(&path).unwrap();
        assert_eq!(got, samples2, "final file must contain the second batch");
    }

    /// Simulate an interrupted write: if the `.tmp` file already exists when
    /// `write_wav` is called (a crash left it behind), the function overwrites
    /// it and completes normally — no stale data leaks to the final path.
    #[test]
    fn stale_tmp_is_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.wav");
        let tmp = path.with_extension("wav.tmp");

        // Plant a stale tmp with junk content.
        std::fs::write(&tmp, b"stale junk").unwrap();

        let samples: Vec<i16> = vec![7, 8, 9];
        write_wav(&path, &samples, test_cfg()).unwrap();

        assert!(path.exists());
        assert!(!tmp.exists());
        let (got, _) = read_wav(&path).unwrap();
        assert_eq!(got, samples);
    }

    /// Slice a sub-range out of a known buffer and read it back: at 16 kHz mono,
    /// 1000 samples = 62.5 ms, so [250ms, 500ms) maps to frames 4000..8000 — the
    /// sample values there are exactly `4000..8000`, which is what we wrote.
    #[test]
    fn clip_extracts_the_expected_range() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.wav");
        let out = dir.path().join("clip.wav");
        // 16_000 samples = 1 s; sample i carries value (i % i16::MAX-ish) so we
        // can assert the exact slice. Use the index directly (wraps past 32767
        // but the values still match between write and read).
        let samples: Vec<i16> = (0..16_000).map(|i| i as i16).collect();
        write_wav(&src, &samples, test_cfg()).unwrap();

        let n = clip_wav(&src, &out, 250, 500).unwrap();
        // 250ms..500ms at 16 kHz = frames 4000..8000 = 4000 frames.
        assert_eq!(n, 4000);
        let (got, cfg) = read_wav(&out).unwrap();
        assert_eq!(got.len(), 4000);
        assert_eq!(got, samples[4000..8000]);
        // Same format as the source.
        assert_eq!(cfg.sample_rate.as_u32(), 16_000);
        assert_eq!(cfg.channels.as_u8(), 1);
    }

    /// An `end_ms` past the recording's duration clamps to the tail rather than
    /// erroring — the user asked for "from here to the end (or beyond)".
    #[test]
    fn clip_end_clamps_to_duration() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.wav");
        let out = dir.path().join("clip.wav");
        // 8000 samples = 500 ms at 16 kHz.
        let samples: Vec<i16> = (0..8_000).map(|i| i as i16).collect();
        write_wav(&src, &samples, test_cfg()).unwrap();

        // Ask for 250ms..10s — only ~250ms exists past the start.
        let n = clip_wav(&src, &out, 250, 10_000).unwrap();
        // frames 4000..8000 = 4000 frames (clamped to the 8000-sample end).
        assert_eq!(n, 4000);
        let (got, _) = read_wav(&out).unwrap();
        assert_eq!(got, samples[4000..8000]);
    }

    /// Channel-aware slicing: a stereo buffer is cut on frame boundaries so the
    /// left/right interleaving is never split. 1000 frames = 62.5ms at 16 kHz,
    /// so [0ms, 62ms) is frames 0..992 = 1984 interleaved samples.
    #[test]
    fn clip_is_frame_aligned_for_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("stereo.wav");
        let out = dir.path().join("clip.wav");
        let cfg = AudioConfig {
            sample_rate: SampleRate(16_000),
            channels: Channels(2),
        };
        // 2000 interleaved samples = 1000 frames.
        let samples: Vec<i16> = (0..2_000).map(|i| i as i16).collect();
        write_wav(&src, &samples, cfg).unwrap();

        let frames = clip_wav(&src, &out, 0, 62).unwrap();
        // floor(62 * 16000 / 1000) = 992 frames.
        assert_eq!(frames, 992);
        let (got, got_cfg) = read_wav(&out).unwrap();
        // The slice is an even number of samples (frame-aligned) and the
        // interleaving lines up with the source: sample 2k/2k+1 are frame k.
        assert_eq!(got.len(), 992 * 2);
        assert_eq!(got, samples[0..992 * 2]);
        assert_eq!(got_cfg.channels.as_u8(), 2);
    }

    /// Invalid ranges error rather than writing a bogus WAV: start >= end,
    /// negatives, and a start at/past the recording's end (empty slice).
    #[test]
    fn clip_rejects_invalid_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.wav");
        let out = dir.path().join("clip.wav");
        // 16_000 samples = 1 s at 16 kHz.
        let samples: Vec<i16> = (0..16_000).map(|i| i as i16).collect();
        write_wav(&src, &samples, test_cfg()).unwrap();

        // start == end and start > end are both empty ranges.
        assert!(clip_wav(&src, &out, 500, 500).is_err());
        assert!(clip_wav(&src, &out, 600, 500).is_err());
        // Negative bounds are rejected.
        assert!(clip_wav(&src, &out, -10, 500).is_err());
        // start at/past the recording end → empty slice after clamping.
        assert!(clip_wav(&src, &out, 1_000, 2_000).is_err());
        assert!(clip_wav(&src, &out, 5_000, 6_000).is_err());
        // No output file was written by any of the rejected calls.
        assert!(!out.exists());
    }
}
