//! Diarization Error Rate (DER) — a dev/eval metric for measuring how well the
//! local diarizer labels who-spoke-when against a hand-checked reference (#10).
//!
//! DER (collar-0, NIST-style) =
//! `(missed + false_alarm + confusion) / total_reference_speech`, all in seconds.
//! Before scoring, the hypothesis speakers are mapped onto the reference speakers
//! by total overlap (greedily), so the *labels* don't matter — `[Speaker 1]` vs
//! `A` — only who is grouped with whom. Lower is better; 0.0 is a perfect match.
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
/// total ref↔hyp overlap (chosen greedily, which is exact for well-separated
/// speakers and a slight over-estimate of confusion only in pathological ties).
/// Overlapping speech is handled NIST-style: per interval the error is
/// `max(n_ref, n_hyp) - n_correct`. With no reference speech, `der` is 0.0 (any
/// hypothesis speech is still reported under `false_alarm`).
pub fn compute_der(reference: &[DerSegment], hypothesis: &[DerSegment]) -> DerReport {
    // Elementary-interval boundaries: every start/end from both sides, sorted.
    let mut bounds: Vec<f64> = reference
        .iter()
        .chain(hypothesis.iter())
        .flat_map(|s| [s.start, s.end])
        .collect();
    bounds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Merge near-equal boundaries (float accumulation from RTTM start+dur can make
    // 2.0+3.0 differ from a reference 5.0 by an epsilon) so a tiny spurious
    // sub-interval isn't scored as missed/false-alarm (audit L3).
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

    // Greedy max-overlap mapping hyp → ref (each used at most once).
    let mut pairs: Vec<(&str, &str, f64)> =
        overlap.iter().map(|(&(r, h), &o)| (r, h, o)).collect();
    pairs.sort_by(|x, y| y.2.partial_cmp(&x.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut map: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut used_ref: HashSet<&str> = HashSet::new();
    for (r, h, _) in pairs {
        if !map.contains_key(h) && !used_ref.contains(r) {
            map.insert(h, r);
            used_ref.insert(r);
        }
    }

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
        // Reference has two speakers; the diarizer heard ONE voice for both. One
        // half maps correctly, the other is confusion.
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
