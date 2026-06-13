//! Peak normalization for a finalized recording.
//!
//! Whisper transcribes a recording more accurately when the speech sits at a
//! healthy level; a microphone left turned down captures the same words far
//! quieter than the model expects. Peak normalization fixes that after capture:
//! it finds the loudest sample, works out the single gain that lifts that peak
//! to a chosen ceiling, and scales the whole buffer by it. The shape of the
//! waveform is untouched — every sample moves by the same factor — so this is a
//! pure level change, not compression or limiting.
//!
//! This runs on the final captured buffer just before the WAV is written. It is
//! deliberately *not* applied to the live preview (the preview is best-effort
//! and re-transcribed continuously) nor to imported files (those already carry
//! whatever level their author chose).

/// Full-scale reference for the dBFS scale: the largest magnitude an `i16`
/// sample can carry. Matches the reference [`crate::silence`] uses, so a dBFS
/// figure means the same thing across the crate.
const FULL_SCALE: f32 = i16::MAX as f32;

/// Below this peak the buffer is treated as silence and left untouched. A clip
/// whose loudest sample is this quiet is either truly silent or pure noise
/// floor; amplifying it would only make hiss louder and could explode a near-
/// zero peak into full-scale noise. 32 of 32767 is about -60 dBFS.
const MIN_PEAK: i16 = 32;

/// Peak-normalize `samples` in place so the loudest sample sits at
/// `target_dbfs` full-scale decibels.
///
/// `target_dbfs` is a ceiling expressed in dBFS: `0.0` is digital full scale,
/// negative values leave headroom (the shipped default is `-1.0`, i.e. a hair
/// below clipping). A single gain is computed from the current peak and applied
/// to every sample, so relative dynamics are preserved.
///
/// Returns `true` when a gain was applied, `false` when the buffer was left
/// untouched. The buffer is left untouched — and only attenuation is ever
/// skipped, never silently amplified — in these cases:
///
/// * the buffer is empty, or its peak is below the noise-floor guard (about
///   -60 dBFS — silence or pure hiss): amplifying it would just raise hiss, so
///   it is left alone;
/// * the peak already sits at or above the target: this is a *boost-quiet-audio*
///   feature, so it never pushes a loud recording down (which would not help
///   transcription) and never amplifies past the target into clipping.
///
/// The computed gain is clamped so the loudest sample lands exactly on the
/// target rather than overshooting through floating-point error, guaranteeing
/// no new clipping is introduced.
pub fn normalize_peak(samples: &mut [i16], target_dbfs: f32) -> bool {
    // Loudest magnitude in the buffer. `i16::MIN` has no positive counterpart,
    // so saturate its magnitude to `i16::MAX` to avoid an overflow on `abs()`.
    let peak = samples
        .iter()
        .map(|&s| s.saturating_abs())
        .max()
        .unwrap_or(0);

    // Silent / noise-floor clip: never amplify, never divide by zero.
    if peak < MIN_PEAK {
        return false;
    }

    // The sample value the target ceiling corresponds to, e.g. -1 dBFS ≈ 29204.
    let target_amplitude = FULL_SCALE * 10f32.powf(target_dbfs / 20.0);

    // Only ever boost: if the peak is already at or past the target the
    // recording is loud enough, so leave it exactly as captured.
    if (peak as f32) >= target_amplitude {
        return false;
    }

    // Single gain that lifts the current peak to the target. Clamp the result
    // back to the target so floating-point rounding can never push the loudest
    // sample past the ceiling and clip.
    let gain = target_amplitude / peak as f32;
    for s in samples.iter_mut() {
        let scaled = (*s as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32);
        *s = scaled.round() as i16;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest magnitude in a buffer, as the tests reason about peaks.
    fn peak_of(samples: &[i16]) -> i16 {
        samples
            .iter()
            .map(|&s| s.saturating_abs())
            .max()
            .unwrap_or(0)
    }

    /// Target amplitude (in i16 sample units) for a dBFS ceiling, mirroring the
    /// math inside `normalize_peak` so the assertions below are exact.
    fn target_amplitude(target_dbfs: f32) -> f32 {
        FULL_SCALE * 10f32.powf(target_dbfs / 20.0)
    }

    #[test]
    fn quiet_buffer_is_amplified_to_target_peak() {
        // A quiet clip peaking at ~1000 should be lifted so its loudest sample
        // lands on the -1 dBFS ceiling (~29204), within rounding.
        let mut samples = vec![1000i16, -800, 500, -1000, 250];
        let applied = normalize_peak(&mut samples, -1.0);
        assert!(applied, "a quiet buffer should be amplified");

        let expected = target_amplitude(-1.0);
        let new_peak = peak_of(&samples) as f32;
        assert!(
            (new_peak - expected).abs() <= 1.0,
            "peak {new_peak} should sit on the target {expected}"
        );
    }

    #[test]
    fn amplification_preserves_relative_dynamics() {
        // Every sample scales by the same gain, so the ratio between two samples
        // is unchanged (here: the second sample is half the first).
        let mut samples = vec![2000i16, 1000];
        normalize_peak(&mut samples, -1.0);
        assert!(
            (samples[0] as f32 - 2.0 * samples[1] as f32).abs() <= 2.0,
            "the 2:1 ratio must survive normalization: {samples:?}"
        );
    }

    #[test]
    fn silent_buffer_is_left_untouched() {
        // All-zero buffer: no divide-by-zero, no change, no gain reported.
        let mut samples = vec![0i16; 16];
        let applied = normalize_peak(&mut samples, -1.0);
        assert!(!applied, "a silent buffer must not be amplified");
        assert_eq!(samples, vec![0i16; 16]);
    }

    #[test]
    fn noise_floor_buffer_is_left_untouched() {
        // A peak below MIN_PEAK is treated as noise floor and never boosted —
        // otherwise hiss in a "silent" clip would be amplified to full scale.
        let mut samples = vec![5i16, -7, 3, -10, 8];
        let before = samples.clone();
        let applied = normalize_peak(&mut samples, -1.0);
        assert!(!applied, "a noise-floor buffer must not be amplified");
        assert_eq!(samples, before);
    }

    #[test]
    fn empty_buffer_is_a_noop() {
        let mut samples: Vec<i16> = Vec::new();
        let applied = normalize_peak(&mut samples, -1.0);
        assert!(!applied);
        assert!(samples.is_empty());
    }

    #[test]
    fn already_loud_buffer_is_not_changed() {
        // Peak already past the -1 dBFS target → this boost-only pass leaves it
        // alone rather than attenuating it.
        let mut samples = vec![i16::MAX, -30000, 100];
        let before = samples.clone();
        let applied = normalize_peak(&mut samples, -1.0);
        assert!(!applied, "an already-loud buffer must be left as captured");
        assert_eq!(samples, before);
    }

    #[test]
    fn never_pushed_into_clipping() {
        // Even a buffer whose peak is just shy of the target must not overshoot
        // full scale after the gain + clamp.
        let mut samples = vec![29_000i16, -29_100, 15_000];
        let applied = normalize_peak(&mut samples, -1.0);
        assert!(applied);
        // Landing exactly on the ceiling (rather than overshooting) is the proof
        // it never clips: the target is below full scale, so an on-target peak is
        // by construction within range.
        assert!((peak_of(&samples) as f32 - target_amplitude(-1.0)).abs() <= 1.0);
        assert!(
            (peak_of(&samples) as f32) <= target_amplitude(-1.0) + 1.0,
            "normalization must never overshoot the ceiling"
        );
    }

    #[test]
    fn target_math_is_correct_for_a_known_peak() {
        // A peak of exactly half full scale (16384) normalized to 0 dBFS should
        // roughly double to full scale; the gain is target/peak = 32767/16384.
        let mut samples = vec![16_384i16, -16_384];
        let applied = normalize_peak(&mut samples, 0.0);
        assert!(applied);
        let gain = FULL_SCALE / 16_384.0;
        let expected = (16_384.0 * gain).round() as i16;
        assert_eq!(peak_of(&samples), expected);
        assert_eq!(expected, i16::MAX);
    }

    #[test]
    fn i16_min_peak_does_not_overflow() {
        // i16::MIN (-32768) has no positive twin; the saturating abs must treat
        // its magnitude as i16::MAX and, being already at full scale, leave the
        // buffer untouched rather than panicking on overflow.
        let mut samples = vec![i16::MIN, 0, 100];
        let before = samples.clone();
        let applied = normalize_peak(&mut samples, -1.0);
        assert!(!applied);
        assert_eq!(samples, before);
    }
}
