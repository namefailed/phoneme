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

/// Score-normalization mode for [`best_match_normalized`] (roadmap V2).
///
/// Raw cosine has a different scale per speaker (some voices sit closer to the
/// whole cohort than others), so one global `threshold` over-accepts for "central"
/// speakers and over-rejects for "outlier" ones. Cohort normalization re-centers
/// every comparison onto a common z-scale so a single threshold means the same
/// thing for everyone.
///
/// The cohort is the candidate set itself — the *other* enrolled speakers'
/// centroids. No external impostor set is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScoreNorm {
    /// No normalization — identical to [`best_match`]. The default, so enabling
    /// the feature is an explicit opt-in.
    #[default]
    Off,
    /// S-norm: z-score the raw cosine `probe·target` against the distribution of
    /// the probe's cosines to the rest of the cohort (mean/std of *probe vs the
    /// other candidates*). Normalizes away how "central" the probe voice is.
    SNorm,
    /// AS-norm (symmetric): average the probe-side z-score (as [`ScoreNorm::SNorm`])
    /// with the target-side z-score (the same cosine re-centered against the
    /// *target's* cosines to the rest of the cohort). Cancels per-speaker scale on
    /// both ends, so it does not matter which voice is the probe.
    ASNorm,
}

/// Mean and (population) standard deviation of `xs`, in f64 for stability.
///
/// Returns `None` when `xs` is empty (no cohort to normalize against) or its std
/// is non-positive / non-finite (a degenerate cohort, e.g. all-equal scores or a
/// single member) — the caller then falls back to the raw score rather than
/// dividing by ~zero and producing `inf`/`NaN`.
fn mean_std(xs: &[f32]) -> Option<(f64, f64)> {
    if xs.is_empty() {
        return None;
    }
    let n = xs.len() as f64;
    let mean = xs.iter().map(|&x| x as f64).sum::<f64>() / n;
    let var = xs.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    if std.is_finite() && std > 0.0 {
        Some((mean, std))
    } else {
        None
    }
}

/// Normalized score of `probe` against `candidates[target]`, under `mode`.
///
/// - [`ScoreNorm::Off`] returns the raw cosine unchanged.
/// - [`ScoreNorm::SNorm`] returns `(raw − μ_probe) / σ_probe`, where `μ/σ` are
///   over the probe's cosines to every *other* candidate (the cohort, target
///   excluded — leave-one-out so the target can't bias its own normalizer).
/// - [`ScoreNorm::ASNorm`] averages that with the symmetric target-side z-score
///   (the same raw cosine re-centered against the target's cosines to the other
///   candidates).
///
/// When a side's cohort is degenerate (fewer than two members, or zero spread —
/// see [`mean_std`]) that side falls back to the raw cosine, so a cohort of size
/// 1 gracefully degrades to raw with no `NaN`/divide-by-zero. AS-norm with one
/// degenerate side uses the well-defined side alone.
pub fn normalized_score(
    probe: &[f32],
    candidates: &[Vec<f32>],
    target: usize,
    mode: ScoreNorm,
) -> f32 {
    let raw = cosine_similarity(probe, &candidates[target]);
    if mode == ScoreNorm::Off {
        return raw;
    }

    // Probe side: probe vs every other candidate (leave-one-out on target).
    let probe_z = {
        let cohort: Vec<f32> = candidates
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != target)
            .map(|(_, c)| cosine_similarity(probe, c))
            .collect();
        mean_std(&cohort).map(|(mu, sd)| ((raw as f64 - mu) / sd) as f32)
    };

    match mode {
        ScoreNorm::Off => unreachable!("handled above"),
        ScoreNorm::SNorm => probe_z.unwrap_or(raw),
        ScoreNorm::ASNorm => {
            // Target side: the target centroid vs every other candidate.
            let target_z = {
                let target_vec = &candidates[target];
                let cohort: Vec<f32> = candidates
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != target)
                    .map(|(_, c)| cosine_similarity(target_vec, c))
                    .collect();
                mean_std(&cohort).map(|(mu, sd)| ((raw as f64 - mu) / sd) as f32)
            };
            match (probe_z, target_z) {
                (Some(p), Some(t)) => 0.5 * (p + t),
                (Some(p), None) => p,
                (None, Some(t)) => t,
                (None, None) => raw, // both cohorts degenerate → raw
            }
        }
    }
}

/// Like [`best_match`], but scores under a [`ScoreNorm`] mode and compares the
/// *normalized* score against `threshold` (roadmap V2).
///
/// With [`ScoreNorm::Off`] this is byte-for-byte [`best_match`] — same scores,
/// same threshold, same tie-breaking — so the default path is unchanged. With a
/// normalization mode the returned `score` is the normalized z-score (no longer a
/// cosine in `[-1, 1]`), so `threshold` must be a z-threshold (~1.5–3), not the
/// cosine `voiceprint_match_threshold`. Ties keep the lowest index, as `best_match`.
pub fn best_match_normalized(
    probe: &[f32],
    candidates: &[Vec<f32>],
    threshold: f32,
    mode: ScoreNorm,
) -> Option<(usize, f32)> {
    if mode == ScoreNorm::Off {
        // Exact delegation — provably identical behavior to the raw path.
        return best_match(probe, candidates, threshold);
    }
    let mut best: Option<(usize, f32)> = None;
    for i in 0..candidates.len() {
        let score = normalized_score(probe, candidates, i, mode);
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

    // --- Score normalization (V2) ---------------------------------------------

    #[test]
    fn off_mode_reproduces_raw_best_match_exactly() {
        // The whole opt-in contract: ScoreNorm::Off must equal best_match for any
        // input, including ties, sub-threshold cases, and empties.
        let check = |probe: &[f32], cands: &[Vec<f32>], thr: f32| {
            assert_eq!(
                best_match_normalized(probe, cands, thr, ScoreNorm::Off),
                best_match(probe, cands, thr),
                "Off path diverged from raw best_match for {probe:?} / {cands:?} @ {thr}"
            );
        };
        let p = vec![1.0, 0.0];
        check(&p, &[vec![0.0, 1.0], vec![0.9, 0.1], vec![0.99, 0.01]], 0.5);
        check(&p, &[vec![0.0, 1.0], vec![0.1, 0.99]], 0.8); // none clear
        check(&p, &[vec![1.0, 0.0], vec![1.0, 0.0]], 0.5); // exact tie → idx 0
        check(&p, &[], 0.5); // empty → None
        check(&p, &[vec![0.6, 0.8]], -1.0); // single candidate
    }

    #[test]
    fn cohort_of_one_falls_back_to_raw_no_nan() {
        // One candidate → no "others" to normalize against → both S-norm and
        // AS-norm must return the raw cosine, finite, never NaN/inf.
        let probe = vec![0.8, 0.6];
        let cands = vec![vec![0.6, 0.8]];
        let raw = cosine_similarity(&probe, &cands[0]);
        for mode in [ScoreNorm::SNorm, ScoreNorm::ASNorm] {
            let s = normalized_score(&probe, &cands, 0, mode);
            assert!(s.is_finite(), "{mode:?} produced non-finite {s}");
            assert!((s - raw).abs() < 1e-6, "{mode:?} should fall back to raw {raw}, got {s}");
        }
    }

    #[test]
    fn degenerate_zero_spread_cohort_falls_back_to_raw() {
        // All other candidates score the probe identically → cohort std 0 → no
        // valid z-score → fall back to raw, no divide-by-zero.
        let probe = vec![1.0, 0.0, 0.0];
        // Two cohort members equidistant from the probe (same cosine), plus the
        // target. Probe-vs-cohort scores are equal → std 0.
        let r = std::f32::consts::FRAC_1_SQRT_2;
        let cands = vec![
            vec![0.0, 1.0, 0.0], // target
            vec![0.0, r, r],
            vec![0.0, r, -r],
        ];
        let raw = cosine_similarity(&probe, &cands[0]);
        let s = normalized_score(&probe, &cands, 0, ScoreNorm::SNorm);
        assert!(s.is_finite());
        assert!((s - raw).abs() < 1e-6, "expected raw fallback {raw}, got {s}");
    }

    #[test]
    fn empty_candidates_is_none_for_all_modes() {
        let probe = vec![1.0, 0.0];
        for mode in [ScoreNorm::Off, ScoreNorm::SNorm, ScoreNorm::ASNorm] {
            assert!(best_match_normalized(&probe, &[], 0.0, mode).is_none());
        }
    }

    #[test]
    fn as_norm_is_symmetric() {
        // AS-norm averages a probe-side z-score and a target-side z-score over the
        // SAME cohort, so scoring A against B must equal scoring B against A when
        // both share one fixed cohort. Build it explicitly from the two-sided
        // formula so the property is tested without the leave-one-out plumbing
        // confusing which voice is excluded:
        //   AS(A,B) = 0.5 * [ (cos(A,B) - μ_A)/σ_A  +  (cos(A,B) - μ_B)/σ_B ]
        // where μ_X/σ_X are X's cosines to a shared cohort. Swapping A and B keeps
        // cos(A,B) and merely reorders the two averaged terms → identical.
        let a = vec![1.0, 0.2, 0.0];
        let b = vec![0.3, 1.0, 0.1];
        let cohort = [vec![0.0, 0.0, 1.0], vec![0.5, 0.5, 0.5], vec![0.2, 0.9, 0.3]];

        // Candidate list for direction A→B: [B, cohort...]; probe = A, target = B
        // at index 0. Cohort (leave-one-out on target B) = the shared cohort, and
        // A is NOT in the list, so the probe never self-matches.
        let mut cands_ab = vec![b.clone()];
        cands_ab.extend(cohort.iter().cloned());
        let s_ab = normalized_score(&a, &cands_ab, 0, ScoreNorm::ASNorm);

        // Direction B→A: [A, cohort...]; probe = B, target = A at index 0.
        let mut cands_ba = vec![a.clone()];
        cands_ba.extend(cohort.iter().cloned());
        let s_ba = normalized_score(&b, &cands_ba, 0, ScoreNorm::ASNorm);

        assert!(
            (s_ab - s_ba).abs() < 1e-5,
            "AS-norm asymmetric: A→B {s_ab} vs B→A {s_ba}"
        );
    }

    /// Build raw vs normalized genuine/impostor score lists from labelled
    /// speakers, scoring each probe against a fixed cohort of the *other*
    /// speakers' centroids — exactly the live recognizer's setup. Returns
    /// `((g_raw, i_raw), (g_norm, i_norm))`.
    #[allow(clippy::type_complexity)]
    fn raw_and_norm_trials(
        speakers: &[(&str, Vec<Vec<f32>>)],
        mode: ScoreNorm,
    ) -> ((Vec<f32>, Vec<f32>), (Vec<f32>, Vec<f32>)) {
        // One enrolled centroid per speaker = the cohort/candidate set.
        let centroids: Vec<Vec<f32>> = speakers
            .iter()
            .map(|(_, v)| mean_centroid(v).unwrap())
            .collect();

        let (mut g_raw, mut i_raw) = (Vec::new(), Vec::new());
        let (mut g_norm, mut i_norm) = (Vec::new(), Vec::new());
        for (si, (_, vecs)) in speakers.iter().enumerate() {
            for probe in vecs {
                for ti in 0..centroids.len() {
                    let raw = cosine_similarity(probe, &centroids[ti]);
                    let norm = normalized_score(probe, &centroids, ti, mode);
                    if ti == si {
                        g_raw.push(raw);
                        g_norm.push(norm);
                    } else {
                        i_raw.push(raw);
                        i_norm.push(norm);
                    }
                }
            }
        }
        ((g_raw, i_raw), (g_norm, i_norm))
    }

    #[test]
    fn snorm_separates_better_than_raw_with_uneven_spreads() {
        use crate::voiceprint_eval::compute_eer;

        // Three speakers placed on the unit circle. The trick that makes raw
        // cosine struggle: speaker C sits *between* A and B, so C's genuine
        // samples have a smaller cosine to C's own centroid than A's genuine
        // samples have to A's — a single global cosine bar can't fit all three.
        // S-norm re-centers each probe against its cohort, flattening that
        // per-speaker scale difference so genuine/impostor separate cleaner.
        let at = |deg: f32, jitter: f32| {
            let t = deg.to_radians() + jitter;
            vec![t.cos(), t.sin()]
        };
        let speakers: Vec<(&str, Vec<Vec<f32>>)> = vec![
            // A: tight cluster near 0°.
            ("a", vec![at(0.0, 0.00), at(0.0, 0.03), at(0.0, -0.03), at(0.0, 0.05)]),
            // B: tight cluster near 70°.
            ("b", vec![at(70.0, 0.00), at(70.0, 0.03), at(70.0, -0.03), at(70.0, 0.05)]),
            // C: WIDE cluster near 35° (between A and B) — large intra-speaker
            // spread, so its raw genuine scores run lower than A's/B's.
            ("c", vec![at(35.0, 0.00), at(35.0, 0.30), at(35.0, -0.30), at(35.0, 0.18)]),
        ];

        let ((g_raw, i_raw), (g_norm, i_norm)) = raw_and_norm_trials(&speakers, ScoreNorm::SNorm);

        let eer_raw = compute_eer(&g_raw, &i_raw).eer.expect("raw EER defined");
        let eer_norm = compute_eer(&g_norm, &i_norm).eer.expect("norm EER defined");
        // On this constructed set: raw EER ≈ 0.083, S-norm EER ≈ 0.042 (halved).

        assert!(
            eer_norm <= eer_raw + 1e-6,
            "S-norm should not worsen EER: raw {eer_raw} vs norm {eer_norm}"
        );
        // And it should strictly help on this constructed uneven-spread case.
        assert!(
            eer_norm < eer_raw,
            "S-norm should reduce EER here: raw {eer_raw} vs norm {eer_norm}"
        );
    }

    #[test]
    fn as_norm_also_does_not_worsen_eer() {
        use crate::voiceprint_eval::compute_eer;
        let at = |deg: f32, jitter: f32| {
            let t: f32 = deg.to_radians() + jitter;
            vec![t.cos(), t.sin()]
        };
        let speakers: Vec<(&str, Vec<Vec<f32>>)> = vec![
            ("a", vec![at(0.0, 0.00), at(0.0, 0.03), at(0.0, -0.03), at(0.0, 0.05)]),
            ("b", vec![at(70.0, 0.00), at(70.0, 0.03), at(70.0, -0.03), at(70.0, 0.05)]),
            ("c", vec![at(35.0, 0.00), at(35.0, 0.30), at(35.0, -0.30), at(35.0, 0.18)]),
        ];
        let ((g_raw, i_raw), (g_norm, i_norm)) = raw_and_norm_trials(&speakers, ScoreNorm::ASNorm);
        let eer_raw = compute_eer(&g_raw, &i_raw).eer.expect("raw EER defined");
        let eer_norm = compute_eer(&g_norm, &i_norm).eer.expect("norm EER defined");
        assert!(
            eer_norm <= eer_raw + 1e-6,
            "AS-norm should not worsen EER: raw {eer_raw} vs norm {eer_norm}"
        );
    }
}
