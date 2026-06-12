//! Silence detection over a rolling window.
//!
//! Compares the RMS energy of the most-recent `window_ms` of audio against a
//! configurable dBFS threshold. When the rolling RMS falls below the threshold
//! for the full window, [`SilenceDetector::is_silent`] returns `true`.

use std::collections::VecDeque;

/// Rolling-window silence detector over canonical 16-bit PCM.
///
/// Feed captured `i16` samples in via [`SilenceDetector::push`]; query
/// [`SilenceDetector::is_silent`] to learn whether the most-recent window has
/// gone quiet. Used by the recorder's Oneshot mode to auto-stop after the
/// speaker stops talking. Not thread-safe — drive it from a single task.
#[derive(Debug)]
pub struct SilenceDetector {
    threshold_linear: f32, // 10 ^ (dbfs / 20)
    window_samples: usize,
    /// Squared samples (i16² as f32), oldest first.
    history: VecDeque<f32>,
    /// Running sum of `history`.
    sum_sq: f64,
}

impl SilenceDetector {
    /// Build a detector that fires once the trailing `window_ms` of audio sits
    /// below `threshold_dbfs`. `threshold_dbfs` is full-scale decibels (a
    /// negative number; quieter = more negative, e.g. -45.0). `window_ms` is the
    /// span that must stay quiet, converted to a sample count via `sample_rate`
    /// (Hz); the window is clamped to at least one sample.
    pub fn new(threshold_dbfs: f32, window_ms: u32, sample_rate: u32) -> Self {
        let threshold_linear = 10f32.powf(threshold_dbfs / 20.0);
        let window_samples = (window_ms as u64 * sample_rate as u64 / 1000) as usize;
        Self {
            threshold_linear,
            window_samples: window_samples.max(1),
            history: VecDeque::with_capacity(window_samples + 1),
            sum_sq: 0.0,
        }
    }

    /// Append new samples to the rolling window.
    pub fn push(&mut self, samples: &[i16]) {
        for &s in samples {
            let sq = (s as f32 / i16::MAX as f32).powi(2);
            self.history.push_back(sq);
            self.sum_sq += sq as f64;
            if self.history.len() > self.window_samples {
                if let Some(old) = self.history.pop_front() {
                    self.sum_sq -= old as f64;
                }
            }
        }
    }

    /// `true` iff the rolling window is full AND its RMS energy is below the
    /// configured threshold.
    pub fn is_silent(&self) -> bool {
        if self.history.len() < self.window_samples {
            return false;
        }
        // Clamp the running sum at zero: f64 non-associativity means that
        // after many adds followed by subtractions in a different order
        // (which is exactly what a loud → silent transition produces),
        // `sum_sq` can drift to a tiny negative value like -1e-13. Dividing
        // and `sqrt`ing a negative would yield `NaN`, and `NaN < threshold`
        // is always false — silently masking the silence.
        let mean_sq = self.sum_sq.max(0.0) / self.history.len() as f64;
        let rms = mean_sq.sqrt() as f32;
        rms < self.threshold_linear
    }

    /// Clear the rolling window. Call after a `start_recording` so the previous
    /// session's tail doesn't trigger silence in the new one.
    pub fn reset(&mut self) {
        self.history.clear();
        self.sum_sq = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_minus_45_converts_to_expected_linear() {
        // 10^(-45/20) ≈ 0.00562
        let det = SilenceDetector::new(-45.0, 100, 16_000);
        assert!((det.threshold_linear - 0.005623).abs() < 1e-4);
    }

    #[test]
    fn window_samples_computed_from_ms_and_rate() {
        let det = SilenceDetector::new(-45.0, 500, 16_000);
        assert_eq!(det.window_samples, 8000);
    }
}
