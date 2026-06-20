//! Speaker voiceprints — the math behind cross-recording named-speaker
//! recognition (roadmap #9).
//!
//! A *voiceprint* is an L2-normalized centroid embedding for one speaker,
//! produced by the local diarizer (`diarization::cluster_centroids`). When a
//! speaker is named in one recording, that centroid is enrolled into a
//! cross-recording library; on later recordings each speaker's centroid is
//! cosine-matched against the library to recognize who's talking.
//!
//! This module is the pure, dependency-free math (similarity, aggregation,
//! matching). Persistence lives in [`crate::catalog`]; capture + recognition
//! wiring lives in the transcription pipeline.

/// Cosine similarity of two equal-length vectors, in `[-1.0, 1.0]`.
///
/// Returns `0.0` (rather than panicking or `NaN`) when the lengths differ or
/// either vector has zero magnitude — both are "no usable signal", which the
/// caller treats as "no match" against any sane threshold.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// L2-normalize `v` in place. A zero (or all-non-finite) vector is left
/// unchanged, so the result is never `NaN`.
pub fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 && norm.is_finite() {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Component-wise mean of several centroids, L2-normalized.
///
/// Returns `None` when `centroids` is empty or the vectors disagree on length
/// (a dimension mismatch means they came from different embedding models and
/// must not be averaged). Non-finite components are skipped per cell so one bad
/// sample can't poison the mean.
pub fn mean_centroid(centroids: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = centroids.first()?;
    let dim = first.len();
    if dim == 0 || centroids.iter().any(|c| c.len() != dim) {
        return None;
    }
    // Accumulate the per-component sum in f64 (then cast to f32 at the end) to
    // match `diarization::cluster_centroids`, which aggregates the identical mean
    // in f64 — keeping the two paths bit-for-bit consistent and avoiding f32
    // rounding drift over many samples.
    let mut sum = vec![0.0f64; dim];
    let mut counts = vec![0u32; dim];
    for c in centroids {
        for (i, &x) in c.iter().enumerate() {
            if x.is_finite() {
                sum[i] += x as f64;
                counts[i] += 1;
            }
        }
    }
    let mut mean = vec![0.0f32; dim];
    for ((m, &s), &n) in mean.iter_mut().zip(sum.iter()).zip(counts.iter()) {
        if n > 0 {
            *m = (s / n as f64) as f32;
        }
    }
    l2_normalize(&mut mean);
    Some(mean)
}

/// The best candidate for `probe` at or above `threshold`, as `(index, score)`.
///
/// `candidates[i]` is compared by cosine similarity; the highest scorer that
/// clears `threshold` wins. `None` when nothing qualifies (empty list, all
/// below the bar, or dimension mismatches). Ties keep the first (lowest index)
/// — stable for callers that order candidates by preference.
pub fn best_match(probe: &[f32], candidates: &[Vec<f32>], threshold: f32) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (i, c) in candidates.iter().enumerate() {
        let score = cosine_similarity(probe, c);
        if score >= threshold && best.is_none_or(|(_, b)| score > b) {
            best = Some((i, score));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_of_identical_unit_vectors_is_one() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_is_scale_invariant_and_orthogonal_is_zero() {
        let a = vec![3.0, 0.0];
        let b = vec![10.0, 0.0]; // same direction, different magnitude
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        let c = vec![0.0, 5.0]; // orthogonal
        assert!(cosine_similarity(&a, &c).abs() < 1e-6);
    }

    #[test]
    fn cosine_degrades_to_zero_on_bad_input() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0); // length mismatch
        assert_eq!(cosine_similarity(&[], &[]), 0.0); // empty
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0); // zero vector
    }

    #[test]
    fn mean_centroid_averages_then_normalizes() {
        let m = mean_centroid(&[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        // Mean is (0.5, 0.5); normalized that's (√½, √½).
        assert!((m[0] - 0.5f32.sqrt()).abs() < 1e-6);
        assert!((m[1] - 0.5f32.sqrt()).abs() < 1e-6);
        let norm: f32 = m.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mean_centroid_rejects_empty_or_mismatched() {
        assert!(mean_centroid(&[]).is_none());
        assert!(mean_centroid(&[vec![1.0, 0.0], vec![1.0]]).is_none());
    }

    #[test]
    fn best_match_picks_highest_above_threshold() {
        let probe = vec![1.0, 0.0];
        let candidates = vec![
            vec![0.0, 1.0],  // orthogonal → 0.0
            vec![0.9, 0.1],  // close
            vec![0.99, 0.01], // closest
        ];
        let (idx, score) = best_match(&probe, &candidates, 0.5).unwrap();
        assert_eq!(idx, 2);
        assert!(score > 0.9);
    }

    #[test]
    fn best_match_returns_none_below_threshold() {
        let probe = vec![1.0, 0.0];
        let candidates = vec![vec![0.0, 1.0], vec![0.1, 0.99]];
        assert!(best_match(&probe, &candidates, 0.8).is_none());
    }
}
