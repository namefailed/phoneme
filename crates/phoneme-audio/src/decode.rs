//! Decode arbitrary audio files (WAV / MP3 / M4A / FLAC) to Phoneme's canonical WAV.
//!
//! Recorded microphone audio already arrives as canonical 16 kHz mono i16 WAV.
//! Imported files can be anything — stereo MP3 at 44.1 kHz, AAC/ALAC in an
//! `.m4a` container, 24-bit FLAC, etc. This module uses the pure-Rust
//! [`symphonia`] demuxer/decoder to turn any supported file into interleaved
//! f32 samples, then funnels them through the SAME downmix → resample → i16
//! conversion the recorder uses, so an imported recording is byte-for-byte the
//! kind of WAV the transcription pipeline already knows how to handle.

use crate::convert::{downmix_to_mono_f32, f32_to_i16, resample_mono};
use crate::format::AudioConfig;
use crate::wav;
use phoneme_core::error::{Error, Result};
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::conv::IntoSample;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Extensions accepted for import. Lowercase, no leading dot.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "flac"];

/// Maximum decoded audio duration accepted for import, in seconds (6 hours).
///
/// `decode_to_f32` buffers the entire decoded stream into one `Vec<f32>` and
/// `convert::resample_mono` builds a `SincFixedIn` sized to the *whole* input,
/// so a long or maliciously-crafted file could exhaust memory. We enforce this
/// cap with a running per-channel sample counter as packets are decoded and
/// bail out BEFORE allocating the full-size resampler. 6 hours is far beyond
/// any realistic voice note (~6.6 GiB of f32 at 48 kHz stereo) while still
/// being a hard upper bound.
pub const MAX_IMPORT_DURATION_SECS: u64 = 6 * 60 * 60;

/// Returns `true` if `path`'s extension is one Phoneme can import.
pub fn is_supported_extension(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => SUPPORTED_EXTENSIONS
            .iter()
            .any(|s| ext.eq_ignore_ascii_case(s)),
        None => false,
    }
}

/// Decode any supported audio file into a canonical 16 kHz mono 16-bit WAV
/// written to `out_wav`. Returns the output duration in milliseconds.
///
/// Returns a descriptive [`Error::Internal`] for unsupported or corrupt input
/// (so the daemon can surface a clean message rather than panicking).
pub fn decode_to_canonical_wav(input: &Path, out_wav: &Path) -> Result<i64> {
    if !is_supported_extension(input) {
        return Err(Error::Internal(format!(
            "unsupported audio format: {} (supported: {})",
            input.display(),
            SUPPORTED_EXTENSIONS.join(", ")
        )));
    }

    let (samples, source_rate, channels) = decode_to_f32(input)?;
    if samples.is_empty() {
        return Err(Error::Internal(format!(
            "decoded no audio samples from {}",
            input.display()
        )));
    }

    // Mirror the recorder's finishing path: downmix → resample → i16.
    let mono = downmix_to_mono_f32(&samples, channels.max(1));
    let resampled = resample_mono(
        &mono,
        source_rate,
        AudioConfig::phoneme_default().sample_rate.as_u32(),
    )?;
    let pcm = f32_to_i16(&resampled);

    wav::write_wav(out_wav, &pcm, AudioConfig::phoneme_default())?;

    let frames = pcm.len() as i64; // mono, so frames == samples
    let duration_ms = frames
        .checked_mul(1000)
        .and_then(|v| v.checked_div(AudioConfig::phoneme_default().sample_rate.as_u32() as i64))
        .ok_or_else(|| Error::Internal("duration calculation overflowed".into()))?;
    Ok(duration_ms)
}

/// Returns `true` once `frames` (per-channel samples) at `sample_rate` exceeds
/// [`MAX_IMPORT_DURATION_SECS`]. Factored out so the cap is unit-testable
/// without synthesizing a multi-hour file.
fn exceeds_duration_cap(frames: u64, sample_rate: u32) -> bool {
    let max_frames = MAX_IMPORT_DURATION_SECS.saturating_mul(sample_rate as u64);
    frames > max_frames
}

/// Demux + decode `input` into interleaved f32 samples, returning
/// `(samples, sample_rate, channel_count)`.
fn decode_to_f32(input: &Path) -> Result<(Vec<f32>, u32, usize)> {
    let file = std::fs::File::open(input)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| Error::Internal(format!("could not parse {}: {e}", input.display())))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| {
            Error::Internal(format!("no decodable audio track in {}", input.display()))
        })?;
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| Error::Internal(format!("no decoder for {}: {e}", input.display())))?;

    let mut out: Vec<f32> = Vec::new();
    let mut sample_rate: u32 = 0;
    let mut channels: usize = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // Clean end-of-stream surfaces as an UnexpectedEof io error.
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => {
                return Err(Error::Internal(format!(
                    "demux error in {}: {e}",
                    input.display()
                )))
            }
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_rate == 0 {
                    sample_rate = decoded.spec().rate;
                    channels = decoded.spec().channels.count();
                }
                append_samples(&decoded, &mut out);
                // Enforce the duration cap with a running counter so we bail out
                // mid-decode (before the full-size resampler is ever built) on a
                // long or crafted file, rather than after buffering everything.
                if sample_rate > 0 && channels > 0 {
                    let frames = (out.len() / channels) as u64;
                    if exceeds_duration_cap(frames, sample_rate) {
                        return Err(Error::Internal(format!(
                            "audio too long to import (max {} hours)",
                            MAX_IMPORT_DURATION_SECS / 3600
                        )));
                    }
                }
            }
            // Recoverable decode hiccups: skip the packet, keep going.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => {
                return Err(Error::Internal(format!(
                    "decode error in {}: {e}",
                    input.display()
                )))
            }
        }
    }

    if sample_rate == 0 || channels == 0 {
        return Err(Error::Internal(format!(
            "could not determine audio format of {}",
            input.display()
        )));
    }

    Ok((out, sample_rate, channels))
}

/// Convert one decoded audio buffer (of whatever native sample type) into
/// interleaved f32 and append it to `out`.
fn append_samples(decoded: &AudioBufferRef<'_>, out: &mut Vec<f32>) {
    macro_rules! interleave {
        ($buf:expr) => {{
            let buf = $buf;
            let frames = buf.frames();
            let channels = buf.spec().channels.count();
            out.reserve(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    out.push(buf.chan(ch)[frame].into_sample());
                }
            }
        }};
    }

    match decoded {
        AudioBufferRef::U8(b) => interleave!(b.as_ref()),
        AudioBufferRef::U16(b) => interleave!(b.as_ref()),
        AudioBufferRef::U24(b) => interleave!(b.as_ref()),
        AudioBufferRef::U32(b) => interleave!(b.as_ref()),
        AudioBufferRef::S8(b) => interleave!(b.as_ref()),
        AudioBufferRef::S16(b) => interleave!(b.as_ref()),
        AudioBufferRef::S24(b) => interleave!(b.as_ref()),
        AudioBufferRef::S32(b) => interleave!(b.as_ref()),
        AudioBufferRef::F32(b) => interleave!(b.as_ref()),
        AudioBufferRef::F64(b) => interleave!(b.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Channels, SampleRate};

    /// Synthesize a stereo 44.1 kHz sine WAV, decode it, and assert the output
    /// is canonical 16 kHz mono of roughly the same duration.
    #[test]
    fn decode_wav_to_canonical_16k_mono() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.wav");
        let dst = dir.path().join("out.wav");

        // 0.5s of 440 Hz stereo @ 44.1 kHz.
        let rate = 44_100u32;
        let dur_s = 0.5f32;
        let frames = (rate as f32 * dur_s) as usize;
        let mut interleaved: Vec<i16> = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let t = n as f32 / rate as f32;
            let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
            let v = (s * 32767.0) as i16;
            interleaved.push(v); // L
            interleaved.push(v); // R
        }
        wav::write_wav(
            &src,
            &interleaved,
            AudioConfig {
                sample_rate: SampleRate(rate),
                channels: Channels(2),
            },
        )
        .unwrap();

        let duration_ms = decode_to_canonical_wav(&src, &dst).unwrap();

        // Output WAV must be 16 kHz mono.
        let (samples, cfg) = wav::read_wav(&dst).unwrap();
        assert_eq!(
            cfg.sample_rate,
            SampleRate::HZ_16K,
            "expected 16 kHz output"
        );
        assert_eq!(cfg.channels, Channels::MONO, "expected mono output");
        assert!(!samples.is_empty(), "decoded output should have samples");

        // Duration should be ~500 ms (allow generous slack for resampler edges).
        assert!(
            (duration_ms - 500).abs() <= 50,
            "expected ~500ms, got {duration_ms}ms"
        );
    }

    #[test]
    fn rejects_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("note.txt");
        std::fs::write(&src, b"not audio").unwrap();
        let dst = dir.path().join("out.wav");
        let err = decode_to_canonical_wav(&src, &dst).unwrap_err();
        assert!(
            err.to_string().contains("unsupported audio format"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("broken.wav");
        std::fs::write(&src, b"RIFFnonsense-not-a-real-wav").unwrap();
        let dst = dir.path().join("out.wav");
        assert!(decode_to_canonical_wav(&src, &dst).is_err());
    }

    #[test]
    fn duration_cap_rejects_over_limit() {
        let rate = 16_000u32;
        let max_frames = MAX_IMPORT_DURATION_SECS * rate as u64;
        // Exactly at the cap is allowed; one frame over is rejected.
        assert!(!exceeds_duration_cap(max_frames, rate));
        assert!(exceeds_duration_cap(max_frames + 1, rate));
        // A clearly-oversized count (7 hours) is rejected.
        assert!(exceeds_duration_cap(7 * 60 * 60 * rate as u64, rate));
    }

    #[test]
    fn duration_cap_allows_normal_clip() {
        // A 10-minute clip at 48 kHz is well under the cap.
        let rate = 48_000u32;
        let frames = 10 * 60 * rate as u64;
        assert!(!exceeds_duration_cap(frames, rate));
    }

    #[test]
    fn is_supported_extension_checks() {
        assert!(is_supported_extension(Path::new("a.wav")));
        assert!(is_supported_extension(Path::new("a.MP3")));
        assert!(is_supported_extension(Path::new("a.m4a")));
        assert!(is_supported_extension(Path::new("a.flac")));
        assert!(is_supported_extension(Path::new("a.FLAC")));
        assert!(!is_supported_extension(Path::new("a.txt")));
        assert!(!is_supported_extension(Path::new("noext")));
    }

    /// Verify that `.flac` passes the extension gate and reaches symphonia's
    /// FLAC decoder. A fully-decoded round-trip test would require a FLAC
    /// encoder in the test suite (symphonia is decoder-only and we don't want
    /// to pull in an extra encoder dep). Instead we confirm the critical
    /// invariant: a `.flac` path is NOT rejected with "unsupported audio
    /// format" — any subsequent parse error is expected for a zero-byte
    /// fixture and is fine.
    #[test]
    fn flac_extension_reaches_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.flac");
        let dst = dir.path().join("out.wav");

        std::fs::write(&src, b"").unwrap();
        let err = decode_to_canonical_wav(&src, &dst).unwrap_err();
        assert!(
            !err.to_string().contains("unsupported audio format"),
            "FLAC extension was rejected before reaching the decoder: {err}"
        );
    }
}
