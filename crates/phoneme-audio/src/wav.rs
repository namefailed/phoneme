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
}
