//! Voiceprint verification calibration — a dev/eval metric that gives the
//! speaker-recognition match threshold a measured basis instead of a guess (V1).
//!
//! Named-speaker recognition ([`crate::voiceprint`]) accepts a probe voice as a
//! known speaker when the cosine similarity clears `voiceprint_match_threshold`
//! (shipped at ~0.5). That number was eyeballed. This module turns a set of
//! *labelled* voiceprints into the standard biometric calibration curve so the
//! bar can be chosen from data:
//!
//! - **Genuine trials** — every pair of voiceprints from the *same* speaker.
//! - **Impostor trials** — every pair from *different* speakers.
//! - **Score** — [`crate::voiceprint::cosine_similarity`], the exact comparison
//!   the live recognizer uses. We do not reinvent it; voiceprints are scored here
//!   identically to how they're scored in the pipeline.
//!
//! Sweeping a threshold across the observed score range yields, at each point,
//! the **FAR** (false-accept rate: impostor trials that scored *at or above* the
//! threshold — wrongly admitted) and the **FRR** (false-reject rate: genuine
//! trials that scored *below* it — wrongly turned away). The two cross at the
//! **EER** (equal error rate); the threshold there is the natural operating point
//! and the headline output for picking `voiceprint_match_threshold`.
//!
//! This module is the pure metric (trial generation + sweep), unit-tested with
//! hand-built synthetic vectors — no audio, no DB, no clock, no RNG in the
//! scoring path. It is a dev harness like [`crate::der`]; it is not wired into the
//! pipeline or IPC.

use crate::voiceprint::cosine_similarity;

/// A speaker label for a labelled voiceprint set. Identity is matched by this
/// key, not by vector content — two entries with the same id are the same person.
pub type SpeakerId = String;

/// One point on the verification calibration curve: at `threshold`, the
/// resulting false-accept and false-reject rates.
#[derive(Debug, Clone, PartialEq)]
pub struct CurvePoint {
    /// The decision threshold a trial's score is compared against.
    pub threshold: f32,
    /// False-accept rate: impostor trials scoring `>= threshold` / impostor
    /// trials. In `[0.0, 1.0]`; non-increasing as `threshold` rises.
    pub far: f32,
    /// False-reject rate: genuine trials scoring `< threshold` / genuine trials.
    /// In `[0.0, 1.0]`; non-decreasing as `threshold` rises.
    pub frr: f32,
}

/// The outcome of a verification calibration. `curve` is the swept FAR/FRR
/// points (ascending by threshold); `eer` / `eer_threshold` are the equal-error
/// crossing, both `None` when it is undefined (no genuine or no impostor trials).
#[derive(Debug, Clone, PartialEq)]
pub struct EerReport {
    /// The equal error rate (FAR ≈ FRR at the crossing), `0.0` = perfect
    /// separation, `~0.5` = chance. `None` when undefined (see [`Self::eer_threshold`]).
    pub eer: Option<f32>,
    /// The score threshold at which the crossing occurs — the candidate
    /// `voiceprint_match_threshold`. `None` when FAR and FRR never cross because
    /// one of the trial sets is empty.
    pub eer_threshold: Option<f32>,
    /// The full FAR/FRR sweep, one [`CurvePoint`] per distinct candidate
    /// threshold, ascending. Empty when there are no trials at all.
    pub curve: Vec<CurvePoint>,
    /// Number of same-speaker (genuine) trials scored.
    pub genuine_trials: usize,
    /// Number of different-speaker (impostor) trials scored.
    pub impostor_trials: usize,
}

impl EerReport {
    /// The empty/undefined report — no trials, no crossing. Returned for inputs
    /// that can't form a calibration (e.g. a single speaker → no impostors, or
    /// one vector per speaker → no genuine pairs).
    fn undefined(genuine_trials: usize, impostor_trials: usize) -> EerReport {
        EerReport {
            eer: None,
            eer_threshold: None,
            curve: Vec::new(),
            genuine_trials,
            impostor_trials,
        }
    }
}

/// Compute the verification calibration from labelled voiceprints.
///
/// `speakers` maps each speaker id to one or more embedding vectors (e.g. several
/// enrolled centroids for the same person). Genuine trials are all within-speaker
/// pairs; impostor trials are all cross-speaker pairs; both are scored with
/// [`cosine_similarity`]. See [`compute_eer`] for the sweep and crossing.
///
/// Convenience wrapper over [`compute_eer`] — use that directly to score against
/// pre-computed trial scores (e.g. from a different source or a cached run).
pub fn calibrate(speakers: &[(SpeakerId, Vec<Vec<f32>>)]) -> EerReport {
    let (genuine, impostor) = trial_scores(speakers);
    compute_eer(&genuine, &impostor)
}

/// Build the genuine and impostor trial-score lists from labelled voiceprints.
///
/// Returns `(genuine_scores, impostor_scores)`. Genuine = every unordered pair of
/// distinct vectors *within* one speaker; impostor = every unordered pair across
/// two *different* speakers. Each pair is scored once (`i < j` / `a < b`), so a
/// pair is never double-counted and a vector is never compared with itself.
/// Pure: the same input always yields the same scores in the same order.
pub fn trial_scores(speakers: &[(SpeakerId, Vec<Vec<f32>>)]) -> (Vec<f32>, Vec<f32>) {
    let mut genuine = Vec::new();
    let mut impostor = Vec::new();

    // Genuine: within-speaker vector pairs (i < j).
    for (_, vecs) in speakers {
        for i in 0..vecs.len() {
            for j in (i + 1)..vecs.len() {
                genuine.push(cosine_similarity(&vecs[i], &vecs[j]));
            }
        }
    }

    // Impostor: cross-speaker vector pairs (speaker a < b), every vector of one
    // against every vector of the other.
    for a in 0..speakers.len() {
        for b in (a + 1)..speakers.len() {
            for va in &speakers[a].1 {
                for vb in &speakers[b].1 {
                    impostor.push(cosine_similarity(va, vb));
                }
            }
        }
    }

    (genuine, impostor)
}

/// Compute the calibration curve and EER from pre-computed trial scores.
///
/// `genuine` are same-speaker scores (should be high), `impostor` are
/// different-speaker scores (should be low). The threshold is swept over every
/// distinct score observed across both sets (the only values at which FAR or FRR
/// can change), plus one point just below the minimum so the all-accept extreme
/// (FAR = 1, FRR = 0) is represented. At each threshold `t`:
///
/// - `FAR(t)` = fraction of `impostor` scoring `>= t` — impostors wrongly accepted.
/// - `FRR(t)` = fraction of `genuine` scoring `<  t` — genuine wrongly rejected.
///
/// As `t` rises FAR is non-increasing and FRR is non-decreasing, so the two cross
/// exactly once (modulo ties). The **EER** is read at that crossing: we scan
/// adjacent sweep points for the first where the sign of `FAR - FRR` flips, then
/// **linearly interpolate** both the threshold and the rate between the two
/// bracketing points (treating FAR and FRR as straight segments between samples),
/// giving a smoother EER than snapping to the nearest sample. If a sample lands
/// exactly on `FAR == FRR`, that point is the EER directly.
///
/// With no genuine trials or no impostor trials the EER is undefined (one rate
/// can't be computed), so [`EerReport::eer`] / [`EerReport::eer_threshold`] are
/// `None` — never a divide-by-zero or panic.
pub fn compute_eer(genuine: &[f32], impostor: &[f32]) -> EerReport {
    if genuine.is_empty() || impostor.is_empty() {
        return EerReport::undefined(genuine.len(), impostor.len());
    }

    // Candidate thresholds: every distinct observed score, ascending, plus a
    // sentinel just below the minimum so the all-accept end of the curve (every
    // trial scores >= t) is sampled. Distinct scores are the only thresholds at
    // which a count — and thus FAR or FRR — can change.
    let mut thresholds: Vec<f32> = genuine.iter().chain(impostor.iter()).copied().collect();
    thresholds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    thresholds.dedup();
    // Sentinel strictly below the lowest score so its FAR is 1.0 and FRR 0.0.
    let lowest = thresholds[0];
    thresholds.insert(0, lowest - 1.0);
    // Sentinel strictly above the highest score so its FAR is 0.0 and FRR 1.0.
    // Without it, when every genuine and impostor score is identical the curve
    // never reaches the all-reject end, `diff = far - frr` never changes sign, and
    // `find_eer` returns `None` instead of the ~0.5 chance EER. The extra point
    // gives the sign change the interpolation needs.
    let highest = *thresholds.last().unwrap();
    thresholds.push(highest + 1.0);

    let g = genuine.len() as f32;
    let im = impostor.len() as f32;
    let curve: Vec<CurvePoint> = thresholds
        .iter()
        .map(|&t| {
            let far = impostor.iter().filter(|&&s| s >= t).count() as f32 / im;
            let frr = genuine.iter().filter(|&&s| s < t).count() as f32 / g;
            CurvePoint {
                threshold: t,
                far,
                frr,
            }
        })
        .collect();

    let (eer, eer_threshold) = find_eer(&curve);
    EerReport {
        eer,
        eer_threshold,
        curve,
        genuine_trials: genuine.len(),
        impostor_trials: impostor.len(),
    }
}

/// Find the equal-error crossing on an ascending FAR/FRR curve.
///
/// `diff = far - frr` starts `>= 0` (at the low end FAR is high, FRR low) and ends
/// `<= 0` (at the high end FAR is low, FRR high). We walk adjacent points looking
/// for where `diff` reaches or crosses zero:
///
/// - An exact `diff == 0` sample *is* the EER (its FAR == FRR).
/// - A sign change between two samples brackets the crossing; we linearly
///   interpolate the fractional position where `diff` hits zero and read both the
///   threshold and the error rate (FAR and FRR meet at the same value there) at
///   that position.
///
/// Returns `(eer, eer_threshold)`. Returns `None` only for a degenerate curve
/// that never reaches zero (shouldn't happen for non-empty trial sets, but kept
/// total rather than panicking).
fn find_eer(curve: &[CurvePoint]) -> (Option<f32>, Option<f32>) {
    if curve.is_empty() {
        return (None, None);
    }
    let diff = |p: &CurvePoint| p.far - p.frr;

    for w in curve.windows(2) {
        let (lo, hi) = (&w[0], &w[1]);
        let (dlo, dhi) = (diff(lo), diff(hi));

        if dlo == 0.0 {
            // Exact crossing sample (FAR == FRR here).
            return (Some(lo.far), Some(lo.threshold));
        }
        if dlo > 0.0 && dhi <= 0.0 {
            if dhi == 0.0 {
                return (Some(hi.far), Some(hi.threshold));
            }
            // diff goes + → -; the zero sits a fraction `f` of the way from lo to
            // hi. Interpolate threshold and rate linearly across that segment.
            let f = dlo / (dlo - dhi); // in (0, 1)
            let threshold = lo.threshold + f * (hi.threshold - lo.threshold);
            // FAR and FRR converge at the crossing; average the two interpolated
            // values so float drift between them can't bias the reported EER.
            let far_at = lo.far + f * (hi.far - lo.far);
            let frr_at = lo.frr + f * (hi.frr - lo.frr);
            let eer = 0.5 * (far_at + frr_at);
            return (Some(eer), Some(threshold));
        }
    }

    // The last sample can be the crossing if every prior diff stayed positive.
    if let Some(last) = curve.last() {
        if diff(last) == 0.0 {
            return (Some(last.far), Some(last.threshold));
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Two unit vectors `cos(theta)` apart in a 2-D plane. Lets a test place
    /// speaker clusters at chosen angular separations.
    fn at_angle(theta: f32) -> Vec<f32> {
        vec![theta.cos(), theta.sin()]
    }

    #[test]
    fn well_separated_clusters_have_near_zero_eer() {
        // Two tight clusters ~90° apart: same-speaker pairs ~1.0, cross-speaker
        // pairs ~0.0. They never overlap, so EER is ~0 and the crossing threshold
        // sits between the two score bands.
        let speakers = vec![
            (
                "alex".to_string(),
                vec![at_angle(0.00), at_angle(0.02), at_angle(-0.02)],
            ),
            (
                "blair".to_string(),
                vec![
                    at_angle(std::f32::consts::FRAC_PI_2),
                    at_angle(std::f32::consts::FRAC_PI_2 + 0.02),
                    at_angle(std::f32::consts::FRAC_PI_2 - 0.02),
                ],
            ),
        ];
        let r = calibrate(&speakers);
        assert_eq!(r.genuine_trials, 6); // 3 within each of 2 speakers
        assert_eq!(r.impostor_trials, 9); // 3 × 3 across
        let eer = r.eer.expect("defined");
        assert!(eer < 0.05, "expected near-zero EER, got {eer} ({r:?})");
        let thr = r.eer_threshold.expect("defined");
        assert!((0.0..1.0).contains(&thr), "threshold in band: {thr}");
    }

    #[test]
    fn perfect_separation_is_eer_zero() {
        // Genuine pairs are identical vectors (score 1.0); impostor pairs are
        // orthogonal (score 0.0). No overlap whatsoever → EER exactly 0.
        let speakers = vec![
            ("a".to_string(), vec![vec![1.0, 0.0], vec![1.0, 0.0]]),
            ("b".to_string(), vec![vec![0.0, 1.0], vec![0.0, 1.0]]),
        ];
        let r = calibrate(&speakers);
        assert_eq!(r.eer, Some(0.0), "{r:?}");
        let thr = r.eer_threshold.expect("defined");
        // A threshold anywhere in (0.0, 1.0) separates them perfectly.
        assert!(thr > 0.0 && thr <= 1.0, "{thr}");
    }

    #[test]
    fn identical_clusters_have_high_eer() {
        // Both "speakers" are tiny jitters around the *same* direction, so a
        // within-speaker pair and a cross-speaker pair are statistically the same
        // comparison — genuine and impostor score distributions coincide and the
        // verifier can't tell them apart → EER near chance (~0.5).
        let speakers = vec![
            (
                "a".to_string(),
                vec![
                    at_angle(0.00),
                    at_angle(0.02),
                    at_angle(0.04),
                    at_angle(0.06),
                ],
            ),
            (
                "b".to_string(),
                vec![
                    at_angle(0.01),
                    at_angle(0.03),
                    at_angle(0.05),
                    at_angle(0.07),
                ],
            ),
        ];
        let r = calibrate(&speakers);
        let eer = r.eer.expect("defined");
        assert!(
            (0.35..=0.65).contains(&eer),
            "overlapping clusters should be near chance, got {eer} ({r:?})"
        );
    }

    #[test]
    fn far_is_non_increasing_and_frr_non_decreasing() {
        // Monotonicity holds for any trial sets: as the threshold rises, fewer
        // impostors clear it (FAR ↓) and more genuine fall below it (FRR ↑).
        let genuine = vec![0.9, 0.7, 0.85, 0.6, 0.95];
        let impostor = vec![0.1, 0.3, 0.2, 0.5, 0.65];
        let r = compute_eer(&genuine, &impostor);
        for w in r.curve.windows(2) {
            assert!(
                w[1].far <= w[0].far + 1e-7,
                "FAR rose: {:?} -> {:?}",
                w[0],
                w[1]
            );
            assert!(
                w[1].frr >= w[0].frr - 1e-7,
                "FRR fell: {:?} -> {:?}",
                w[0],
                w[1]
            );
        }
        // Thresholds are strictly ascending.
        for w in r.curve.windows(2) {
            assert!(w[1].threshold > w[0].threshold, "{:?}", r.curve);
        }
        // EER lies between the two distributions' overlap and is a valid rate.
        let eer = r.eer.expect("defined");
        assert!((0.0..=1.0).contains(&eer), "{eer}");
    }

    #[test]
    fn curve_endpoints_are_all_accept_and_all_reject() {
        // Lowest threshold: accept everything → FAR 1.0, FRR 0.0.
        // Highest threshold: a genuine score equals the max, but impostors are
        // all below it → FAR 0.0, and at least the top genuine is still accepted.
        let genuine = vec![0.8, 0.9];
        let impostor = vec![0.1, 0.2];
        let r = compute_eer(&genuine, &impostor);
        let first = r.curve.first().unwrap();
        assert!(
            approx(first.far, 1.0) && approx(first.frr, 0.0),
            "{first:?}"
        );
        let last = r.curve.last().unwrap();
        assert!(approx(last.far, 0.0), "{last:?}");
    }

    #[test]
    fn exact_crossing_sample_is_taken_directly() {
        // Construct trials so a swept threshold lands exactly on FAR == FRR.
        // genuine {0.4, 0.6}, impostor {0.4, 0.6}: at t = 0.6, FAR = 1/2 (the 0.6
        // impostor), FRR = 1/2 (the 0.4 genuine) → exact EER 0.5 at threshold 0.6.
        let genuine = vec![0.4, 0.6];
        let impostor = vec![0.4, 0.6];
        let r = compute_eer(&genuine, &impostor);
        assert_eq!(r.eer, Some(0.5), "{r:?}");
        assert_eq!(r.eer_threshold, Some(0.6), "{r:?}");
    }

    #[test]
    fn fully_overlapping_scores_give_eer_one_half() {
        // Genuine and impostor distributions are byte-for-byte identical, so the
        // verifier can't separate them at all — EER must be ~0.5 (chance), not
        // `None`. The right-side sentinel above the max threshold is what gives the
        // FAR/FRR curve its all-reject end so the crossing interpolates here.
        let genuine = vec![0.3, 0.5, 0.7];
        let impostor = vec![0.3, 0.5, 0.7];
        let r = compute_eer(&genuine, &impostor);
        let eer = r.eer.expect("defined: the curve must reach a crossing");
        assert!(
            (eer - 0.5).abs() < 1e-6,
            "fully-overlapping scores should be exactly chance, got {eer} ({r:?})"
        );
        assert!(r.eer_threshold.is_some(), "{r:?}");
    }

    #[test]
    fn single_speaker_has_no_impostors_and_is_undefined() {
        let speakers = vec![(
            "only".to_string(),
            vec![vec![1.0, 0.0], vec![0.9, 0.1], vec![0.8, 0.2]],
        )];
        let r = calibrate(&speakers);
        assert_eq!(r.eer, None);
        assert_eq!(r.eer_threshold, None);
        assert!(r.curve.is_empty());
        assert_eq!(r.genuine_trials, 3); // 3 within-speaker pairs
        assert_eq!(r.impostor_trials, 0);
    }

    #[test]
    fn single_vector_per_speaker_has_no_genuine_and_is_undefined() {
        let speakers = vec![
            ("a".to_string(), vec![vec![1.0, 0.0]]),
            ("b".to_string(), vec![vec![0.0, 1.0]]),
        ];
        let r = calibrate(&speakers);
        assert_eq!(r.eer, None);
        assert_eq!(r.eer_threshold, None);
        assert!(r.curve.is_empty());
        assert_eq!(r.genuine_trials, 0);
        assert_eq!(r.impostor_trials, 1); // a × b
    }

    #[test]
    fn empty_input_is_undefined_without_panic() {
        let r = calibrate(&[]);
        assert_eq!(r.eer, None);
        assert_eq!(r.eer_threshold, None);
        assert!(r.curve.is_empty());
        assert_eq!(r.genuine_trials, 0);
        assert_eq!(r.impostor_trials, 0);
    }

    #[test]
    fn scores_use_the_recognizer_cosine_exactly() {
        // The harness must score with crate::voiceprint::cosine_similarity, so a
        // hand-computed cosine should equal the genuine trial score it produces.
        let speakers = vec![(
            "a".to_string(),
            vec![vec![1.0, 0.0], vec![3.0, 0.0]], // same direction → cosine 1.0
        )];
        let (genuine, impostor) = trial_scores(&speakers);
        assert_eq!(genuine.len(), 1);
        assert!(impostor.is_empty());
        assert!(approx(
            genuine[0],
            cosine_similarity(&[1.0, 0.0], &[3.0, 0.0])
        ));
        assert!(approx(genuine[0], 1.0));
    }
}
