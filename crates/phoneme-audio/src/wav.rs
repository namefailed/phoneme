//! WAV encoding and decoding via `hound`.
//!
//! All Phoneme recordings are 16-bit PCM. This module deals only with that
//! format — `hound::WavWriter`/`WavReader` handle the on-disk byte layout.

use crate::format::{AudioConfig, Channels, SampleRate};
use phoneme_core::error::{Error, Result};
use std::path::Path;

/// Write a buffer of `i16` samples to a WAV file. Creates parent directories
/// if missing.
pub fn write_wav(path: &Path, samples: &[i16], cfg: AudioConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let spec = hound::WavSpec {
        channels: cfg.channels.as_u8() as u16,
        sample_rate: cfg.sample_rate.as_u32(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| Error::Internal(format!("hound create: {e}")))?;
    for &s in samples {
        writer
            .write_sample(s)
            .map_err(|e| Error::Internal(format!("hound write_sample: {e}")))?;
    }
    writer
        .finalize()
        .map_err(|e| Error::Internal(format!("hound finalize: {e}")))?;
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
    let reader = hound::WavReader::open(path)
        .map_err(|e| Error::Internal(format!("hound open: {e}")))?;
    let spec = reader.spec();
    let frames = reader.len() as u64 / spec.channels.max(1) as u64;
    let ms = (frames * 1000) / spec.sample_rate.max(1) as u64;
    Ok(ms as i64)
}
