//! Sample format conversion helpers.
//!
//! CPAL commonly hands us f32 samples at 44.1 or 48 kHz stereo from the OS;
//! Phoneme writes i16 at 16 kHz mono. These functions bridge the gap.

use phoneme_core::error::{Error, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// Convert f32 samples in `[-1.0, 1.0]` to i16. Values outside the range are
/// clamped (not wrapped).
pub fn f32_to_i16(input: &[f32]) -> Vec<i16> {
    input
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            // Scale by 32768 so -1.0 uses the full negative range (i16::MIN);
            // clamp keeps +1.0 at i16::MAX (32768 isn't representable as i16).
            (clamped * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect()
}

/// Convert i16 samples to f32 in `[-1.0, 1.0]`.
pub fn i16_to_f32(input: &[i16]) -> Vec<f32> {
    input.iter().map(|&s| s as f32 / i16::MAX as f32).collect()
}

/// Downmix multi-channel interleaved samples to mono by averaging.
pub fn downmix_to_mono_f32(input: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return input.to_vec();
    }
    let frames = input.len() / channels;
    (0..frames)
        .map(|f| {
            let start = f * channels;
            input[start..start + channels].iter().sum::<f32>() / channels as f32
        })
        .collect()
}

/// Resample mono f32 samples from `from_rate` to `to_rate`. If the rates match,
/// returns a copy. Uses rubato's sinc resampler with default parameters tuned
/// for speech.
pub fn resample_mono(input: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(input.to_vec());
    }
    if input.is_empty() {
        return Ok(vec![]);
    }
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = to_rate as f64 / from_rate as f64;
    let chunk_size = input.len();
    let mut resampler = SincFixedIn::<f32>::new(ratio, 1.0, params, chunk_size, 1)
        .map_err(|e| Error::Internal(format!("rubato init: {e}")))?;
    let chunks = vec![input.to_vec()];
    let out = resampler
        .process(&chunks, None)
        .map_err(|e| Error::Internal(format!("rubato process: {e}")))?;
    Ok(out.into_iter().next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_round_trips_within_tolerance() {
        let orig: Vec<f32> = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let int16 = f32_to_i16(&orig);
        let back = i16_to_f32(&int16);
        for (o, b) in orig.iter().zip(back.iter()) {
            assert!((o - b).abs() < 0.001, "drift too high: orig={o} back={b}");
        }
    }

    #[test]
    fn f32_to_i16_clamps_out_of_range() {
        let v = f32_to_i16(&[1.5, -2.0, 0.0]);
        assert_eq!(v[0], i16::MAX);
        assert_eq!(v[1], i16::MIN); // (-1.0)*32768 = -32768 = full-scale negative
        assert_eq!(v[2], 0);
    }

    #[test]
    fn downmix_stereo_averages_channels() {
        // Interleaved L,R,L,R,L,R
        let stereo = vec![1.0, -1.0, 0.5, -0.5, 0.0, 0.0];
        let mono = downmix_to_mono_f32(&stereo, 2);
        assert_eq!(mono, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let mono = vec![0.1, 0.2, 0.3];
        let out = downmix_to_mono_f32(&mono, 1);
        assert_eq!(out, mono);
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let input = vec![0.0_f32, 0.5, -0.5];
        let out = resample_mono(&input, 16_000, 16_000).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn resample_downsamples_length_roughly_proportional() {
        // 1 second of 48kHz mono = 48000 samples → ~16000 after resample to 16kHz
        let input = vec![0.0_f32; 48_000];
        let out = resample_mono(&input, 48_000, 16_000).unwrap();
        // Rubato's chunked resampler has some delay/padding; allow ±5%.
        let expected = 16_000;
        let tolerance = (expected as f32 * 0.05) as usize;
        assert!(
            (out.len() as i64 - expected as i64).unsigned_abs() as usize <= tolerance,
            "got {} samples, expected ~{}",
            out.len(),
            expected
        );
    }
}
