//! Diarization Error Rate (DER) — a dev/eval metric for measuring how well the
//! local diarizer labels who-spoke-when against a hand-checked reference (#10).
//!
//! DER (collar-0, NIST-style) =
//! `(missed + false_alarm + confusion) / total_reference_speech`, all in seconds.
//! Before scoring, the hypothesis speakers are mapped onto the reference speakers
//! by the optimal (max-total-overlap) one-to-one assignment, so the label strings
//! don't matter (`[Speaker 1]` vs `A`); only who is grouped with whom. Lower is
//! better, and 0.0 is a perfect match.
//!
//! This module is the pure metric (parse + score), unit-tested without any audio.
//! A harness that runs the real diarizer on an audio fixture and scores it against
//! an RTTM lives behind a fixture set (see the dev docs); the metric itself is the
//! reusable, verifiable core.

use std::collections::HashSet;

/// One labelled span: `[start, end)` seconds attributed to `speaker`.
#[derive(Debug, Clone, PartialEq)]
pub struct DerSegment {
    /// Start time in seconds.
    pub start: f64,
    /// End time in seconds (exclusive).
    pub end: f64,
    /// The speaker label (any string; identity is matched by overlap, not name).
    pub speaker: String,
}

impl DerSegment {
    /// Build DER segments (a hypothesis to score) from the local diarizer's output
    /// spans, so a harness can feed [`compute_der`] directly with what the diarizer
    /// produced for a recording.
    pub fn from_spans(spans: &[crate::diarization::SpeakerSpan]) -> Vec<DerSegment> {
        spans
            .iter()
            .map(|s| DerSegment {
                start: s.start,
                end: s.end,
                speaker: s.label.clone(),
            })
            .collect()
    }
}

/// The breakdown of a DER computation. All component times are in seconds; `der`
/// is the dimensionless ratio.
#[derive(Debug, Clone, PartialEq)]
pub struct DerReport {
    /// `(missed + false_alarm + confusion) / total_reference` (0.0 = perfect).
    pub der: f64,
    /// Reference speech time with no mapped hypothesis speaker.
    pub missed: f64,
    /// Hypothesis speech time with no reference speaker (extra speech invented).
    pub false_alarm: f64,
    /// Time where both have a speaker but they map to different ones.
    pub confusion: f64,
    /// Total reference speech time (the DER denominator).
    pub total_reference: f64,
}

/// Parse RTTM `SPEAKER` lines into segments; other line types are ignored. RTTM
/// fields are whitespace-separated:
/// `SPEAKER <file> <chan> <start> <dur> <NA> <NA> <speaker> <NA> <NA>`, so the
/// start is field 3, duration field 4, and speaker field 7 (0-based). Malformed
/// or non-numeric lines are skipped rather than failing the whole parse.
pub fn parse_rttm(rttm: &str) -> Vec<DerSegment> {
    let mut out = Vec::new();
    for line in rttm.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 || !f[0].eq_ignore_ascii_case("SPEAKER") {
            continue;
        }
        let (Ok(start), Ok(dur)) = (f[3].parse::<f64>(), f[4].parse::<f64>()) else {
            continue;
        };
        if dur <= 0.0 {
            continue;
        }
        out.push(DerSegment {
            start,
            end: start + dur,
            speaker: f[7].to_string(),
        });
    }
    out
}

/// The distinct speakers active over `[a, b)` — a speaker whose segment fully
/// covers the elementary interval (guaranteed because the interval boundaries are
/// exactly the segment endpoints).
fn active(segs: &[DerSegment], a: f64, b: f64) -> HashSet<&str> {
    segs.iter()
        .filter(|s| s.start <= a && s.end >= b)
        .map(|s| s.speaker.as_str())
        .collect()
}

/// Compute the collar-0 DER of `hypothesis` against `reference`.
///
/// The timeline is split at every segment boundary from both sides; each
/// elementary interval is then scored against the speaker mapping that maximizes
/// total ref↔hyp overlap. That mapping is the optimal one-to-one assignment
/// (max-weight bipartite matching, see `max_overlap_assignment`), which gives
/// strict NIST DER rather than a greedy approximation that can strand a speaker
/// into confusion. Overlapping speech follows the NIST rule: per interval the
/// error is `max(n_ref, n_hyp) - n_correct`. With no reference speech, `der` is
/// 0.0, though any hypothesis speech is still reported under `false_alarm`.
pub fn compute_der(reference: &[DerSegment], hypothesis: &[DerSegment]) -> DerReport {
    // Elementary-interval boundaries: every start/end from both sides, sorted.
    let mut bounds: Vec<f64> = reference
        .iter()
        .chain(hypothesis.iter())
        .flat_map(|s| [s.start, s.end])
        .collect();
    bounds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Merge near-equal boundaries so a tiny spurious sub-interval doesn't get
    // scored as missed/false-alarm. Float accumulation from RTTM start+dur can
    // make e.g. 2.0+3.0 differ from a reference 5.0 by an epsilon.
    let mut deduped: Vec<f64> = Vec::with_capacity(bounds.len());
    for b in bounds {
        if deduped.last().is_none_or(|&last| (b - last).abs() > 1e-6) {
            deduped.push(b);
        }
    }
    let bounds = deduped;

    // Pass 1 — overlap matrix (ref speaker, hyp speaker) → co-active seconds.
    let mut overlap: std::collections::HashMap<(&str, &str), f64> =
        std::collections::HashMap::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = b - a;
        if d <= 0.0 {
            continue;
        }
        let ref_act = active(reference, a, b);
        let hyp_act = active(hypothesis, a, b);
        for r in &ref_act {
            for h in &hyp_act {
                *overlap.entry((*r, *h)).or_insert(0.0) += d;
            }
        }
    }

    // Optimal max-overlap one-to-one mapping hyp → ref (each used at most once).
    let map = max_overlap_assignment(&overlap);

    // Pass 2 — score each interval under the mapping.
    let (mut missed, mut false_alarm, mut confusion, mut total_reference) = (0.0, 0.0, 0.0, 0.0);
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = b - a;
        if d <= 0.0 {
            continue;
        }
        let ref_act = active(reference, a, b);
        let hyp_act = active(hypothesis, a, b);
        let n_ref = ref_act.len();
        let n_hyp = hyp_act.len();
        let n_correct = hyp_act
            .iter()
            .filter(|h| map.get(*h).is_some_and(|r| ref_act.contains(r)))
            .count();
        missed += d * n_ref.saturating_sub(n_hyp) as f64;
        false_alarm += d * n_hyp.saturating_sub(n_ref) as f64;
        confusion += d * (n_ref.min(n_hyp).saturating_sub(n_correct)) as f64;
        total_reference += d * n_ref as f64;
    }

    let der = if total_reference > 0.0 {
        (missed + false_alarm + confusion) / total_reference
    } else {
        0.0
    };
    DerReport {
        der,
        missed,
        false_alarm,
        confusion,
        total_reference,
    }
}

/// The optimal one-to-one mapping `hyp → ref` that maximizes total ref↔hyp
/// overlap, given a sparse overlap matrix keyed by `(ref, hyp)` → co-active
/// seconds.
///
/// This is the assignment NIST DER's confusion term depends on: each hypothesis
/// speaker is paired with at most one reference speaker (and vice versa) so that
/// the summed overlap of the chosen pairs is maximal. A greedy "take the biggest
/// overlap first" pass isn't optimal; it can lock a reference speaker to a
/// hypothesis that another hypothesis needed more, stranding a speaker into
/// confusion and inflating DER. So we solve it exactly with the Hungarian
/// (Kuhn–Munkres) algorithm on the small `ref × hyp` matrix. The speaker counts
/// are tiny, so the `O(n^3)` cost is irrelevant.
///
/// Only pairs with strictly positive overlap survive into the result. A forced
/// assignment onto a zero-overlap reference is meaningless and shouldn't count as
/// "correct".
fn max_overlap_assignment<'a>(
    overlap: &std::collections::HashMap<(&'a str, &'a str), f64>,
) -> std::collections::HashMap<&'a str, &'a str> {
    // Collect the distinct speakers on each side and index them.
    let mut refs: Vec<&str> = Vec::new();
    let mut hyps: Vec<&str> = Vec::new();
    let mut ref_idx: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut hyp_idx: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for &(r, h) in overlap.keys() {
        if !ref_idx.contains_key(r) {
            ref_idx.insert(r, refs.len());
            refs.push(r);
        }
        if !hyp_idx.contains_key(h) {
            hyp_idx.insert(h, hyps.len());
            hyps.push(h);
        }
    }
    let mut out: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    if refs.is_empty() || hyps.is_empty() {
        return out;
    }

    // Hungarian algorithm (Kuhn–Munkres, O(n^3) potentials form). It minimizes a
    // square cost matrix, so we make it square by padding the short side with
    // dummy rows/columns of cost 0 and solve `cost = MAX - overlap` to turn the
    // max-weight assignment into a min-cost one.
    let n = refs.len().max(hyps.len());
    let max_w = overlap.values().cloned().fold(0.0_f64, f64::max);
    // cost[i][j] for ref i, hyp j; padded cells default to `max_w` (overlap 0).
    let mut cost = vec![vec![max_w; n]; n];
    for (&(r, h), &o) in overlap {
        cost[ref_idx[r]][hyp_idx[h]] = max_w - o;
    }

    // Standard 1-indexed potential method. `p[j]` = which row is matched to
    // column `j` (0 = none); `way` reconstructs the augmenting path.
    let inf = f64::INFINITY;
    let mut u = vec![0.0_f64; n + 1];
    let mut v = vec![0.0_f64; n + 1];
    let mut p = vec![0usize; n + 1];
    let mut way = vec![0usize; n + 1];
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![inf; n + 1];
        let mut used = vec![false; n + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = 0usize;
            for j in 1..=n {
                if !used[j] {
                    let cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=n {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        // Augment along the path back to the free column.
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }

    // `p[j]` is the ref row matched to hyp column `j`. Keep only real pairs with
    // positive overlap (drop the padded dummies and zero-overlap forced matches).
    for j in 1..=n {
        let i = p[j];
        if i == 0 || i > refs.len() || j > hyps.len() {
            continue;
        }
        let (r, h) = (refs[i - 1], hyps[j - 1]);
        if overlap.get(&(r, h)).is_some_and(|&o| o > 0.0) {
            out.insert(h, r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, spk: &str) -> DerSegment {
        DerSegment {
            start,
            end,
            speaker: spk.to_string(),
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn perfect_match_is_zero_regardless_of_labels() {
        // Same who-spoke-when, different label strings → DER 0.
        let reference = vec![seg(0.0, 5.0, "A"), seg(5.0, 10.0, "B")];
        let hypothesis = vec![seg(0.0, 5.0, "1"), seg(5.0, 10.0, "2")];
        let r = compute_der(&reference, &hypothesis);
        assert!(approx(r.der, 0.0), "{r:?}");
        assert!(approx(r.total_reference, 10.0));
    }

    #[test]
    fn all_missed_is_one() {
        let reference = vec![seg(0.0, 10.0, "A")];
        let hypothesis: Vec<DerSegment> = vec![];
        let r = compute_der(&reference, &hypothesis);
        assert!(approx(r.der, 1.0), "{r:?}");
        assert!(approx(r.missed, 10.0));
    }

    #[test]
    fn false_alarm_with_no_reference_is_recorded_but_der_zero() {
        let reference: Vec<DerSegment> = vec![];
        let hypothesis = vec![seg(0.0, 4.0, "X")];
        let r = compute_der(&reference, &hypothesis);
        assert!(approx(r.false_alarm, 4.0), "{r:?}");
        assert!(approx(r.der, 0.0)); // no reference speech to divide by
    }

    #[test]
    fn one_speaker_split_across_two_reference_speakers_is_confusion() {
        // Reference has two speakers; the diarizer heard a single voice for both.
        // One half maps correctly, the other is confusion.
        let reference = vec![seg(0.0, 5.0, "A"), seg(5.0, 10.0, "B")];
        let hypothesis = vec![seg(0.0, 10.0, "X")];
        let r = compute_der(&reference, &hypothesis);
        assert!(approx(r.confusion, 5.0), "{r:?}");
        assert!(approx(r.missed, 0.0));
        assert!(approx(r.false_alarm, 0.0));
        assert!(approx(r.der, 0.5)); // 5s confusion / 10s reference
    }

    #[test]
    fn missed_and_false_alarm_partial() {
        // Reference A speaks 0–10. Hypothesis only catches 0–6, and invents a
        // speaker 10–12 (after the reference ends).
        let reference = vec![seg(0.0, 10.0, "A")];
        let hypothesis = vec![seg(0.0, 6.0, "X"), seg(10.0, 12.0, "Y")];
        let r = compute_der(&reference, &hypothesis);
        assert!(approx(r.missed, 4.0), "{r:?}"); // 6–10 missed
        assert!(approx(r.false_alarm, 2.0)); // 10–12 invented
        assert!(approx(r.der, 0.6)); // (4 + 2) / 10
    }

    #[test]
    fn parse_rttm_reads_speaker_lines() {
        let rttm = "\
SPEAKER meeting 1 0.00 5.00 <NA> <NA> A <NA> <NA>
SPEAKER meeting 1 5.00 5.00 <NA> <NA> B <NA> <NA>
; a comment line is ignored
SPEAKER meeting 1 bad dur <NA> <NA> C <NA> <NA>";
        let segs = parse_rttm(rttm);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], seg(0.0, 5.0, "A"));
        assert_eq!(segs[1], seg(5.0, 10.0, "B"));
    }

    #[test]
    fn optimal_mapping_beats_greedy_when_greedy_strands_a_speaker() {
        // Three reference speakers, each alone on a 10s slice (total 30s). The
        // hypothesis also has three speakers and exactly one is active per
        // sub-interval, so there is no missed/false-alarm speech — the DER is
        // pure confusion driven entirely by the ref↔hyp mapping.
        //
        // Overlap matrix (seconds):
        //         X    Y    Z
        //   A     1    9    .
        //   B     2    .    8
        //   C     .    3    7
        //
        // A greedy "take the biggest free overlap first" pass grabs (A,Y)=9 then
        // (B,Z)=8, which strands X entirely: nothing free is left for it, so all
        // of X's reference time falls into confusion. Greedy captures 9+8 = 17s,
        // leaving 13s confusion → DER 13/30 ≈ 0.4333.
        //
        // The optimal one-to-one assignment is X→B, Y→A, Z→C, capturing
        // 2+9+7 = 18s, so only 12s is confusion → DER 12/30 = 0.4 — strictly
        // lower, and the correct (NIST) answer.
        let reference = vec![
            seg(0.0, 10.0, "A"),
            seg(10.0, 20.0, "B"),
            seg(20.0, 30.0, "C"),
        ];
        let hypothesis = vec![
            seg(0.0, 1.0, "X"),   // A∩X = 1
            seg(1.0, 10.0, "Y"),  // A∩Y = 9
            seg(10.0, 12.0, "X"), // B∩X = 2
            seg(12.0, 20.0, "Z"), // B∩Z = 8
            seg(20.0, 23.0, "Y"), // C∩Y = 3
            seg(23.0, 30.0, "Z"), // C∩Z = 7
        ];
        let r = compute_der(&reference, &hypothesis);
        assert!(approx(r.missed, 0.0), "{r:?}");
        assert!(approx(r.false_alarm, 0.0), "{r:?}");
        // Optimal confusion is 12s, not the greedy 13s.
        assert!(approx(r.confusion, 12.0), "{r:?}");
        assert!(approx(r.total_reference, 30.0), "{r:?}");
        assert!(approx(r.der, 12.0 / 30.0), "{r:?}");
        // And strictly below what the old greedy mapping would have scored.
        assert!(r.der < 13.0 / 30.0, "{r:?}");
    }

    #[test]
    fn round_trips_through_rttm_to_zero_der() {
        let rttm = "\
SPEAKER m 1 0.0 3.0 <NA> <NA> spk_a <NA> <NA>
SPEAKER m 1 3.0 2.0 <NA> <NA> spk_b <NA> <NA>";
        let reference = parse_rttm(rttm);
        // A hypothesis with the same timing but swapped/renamed labels scores 0.
        let hypothesis = vec![seg(0.0, 3.0, "0"), seg(3.0, 5.0, "1")];
        let r = compute_der(&reference, &hypothesis);
        assert!(approx(r.der, 0.0), "{r:?}");
    }
}
