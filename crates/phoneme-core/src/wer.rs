//! Word Error Rate (WER) and Character Error Rate (CER) — dev/eval accuracy
//! metrics for measuring how closely a hypothesis transcript matches a
//! reference (#N3).
//!
//! `WER = (substitutions + insertions + deletions) / reference_word_count`
//! where edits are counted at the **word** level using Levenshtein distance.
//! CER applies the identical algorithm at the **character** level.
//!
//! # Tokenization
//!
//! Words are lowercased and stripped of ASCII punctuation before comparison.
//! "Hello, world!" and "hello world" are identical under this scheme. The
//! intent is simple robustness to trivial surface variation; if you need a
//! richer normalizer (e.g. numeral expansion) apply it before calling these
//! functions. See [`tokenize_words`] for the exact rules.
//!
//! # Edge cases
//!
//! | Situation | WER |
//! |-----------|-----|
//! | Identical transcripts | `wer = 0.0` |
//! | Empty hypothesis | `wer = 1.0` (all words deleted) |
//! | Many extra insertions | `wer` may exceed `1.0` — not capped |
//! | Empty reference | `None` (WER undefined; no denominator) |
//!
//! These match the convention used in standard ASR benchmarking (e.g. NIST
//! STM/CTM scoring). The caller decides how to handle `None` or `wer > 1`.
//!
//! This module is a pure metric (tokenize + edit distance), unit-tested without
//! any audio. It is not wired into the live pipeline; it is a dev/eval harness
//! like [`crate::der`] and [`crate::voiceprint_eval`].

/// The result of a WER or CER computation.
///
/// All counts refer to the **reference** side; WER / CER is
/// `(substitutions + insertions + deletions) / ref_units`. `None` when the
/// reference is empty (denominator is zero; the metric is undefined, not 0).
#[derive(Debug, Clone, PartialEq)]
pub struct WerReport {
    /// `(substitutions + insertions + deletions) / ref_units`. `None` when the
    /// reference contains no tokens (WER / CER is undefined, not 0).
    pub wer: Option<f64>,
    /// Tokens present in the reference that were replaced by a different token
    /// in the hypothesis.
    pub substitutions: usize,
    /// Tokens present in the hypothesis that have no corresponding reference
    /// token (extra words invented by the ASR).
    pub insertions: usize,
    /// Tokens present in the reference that are absent from the hypothesis
    /// (words the ASR dropped).
    pub deletions: usize,
    /// Number of tokens in the (normalized) reference — the WER denominator.
    pub ref_units: usize,
}

impl WerReport {
    fn compute(ref_units: usize, subs: usize, ins: usize, dels: usize) -> Self {
        let wer = if ref_units == 0 {
            None
        } else {
            Some((subs + ins + dels) as f64 / ref_units as f64)
        };
        WerReport {
            wer,
            substitutions: subs,
            insertions: ins,
            deletions: dels,
            ref_units,
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenization
// ---------------------------------------------------------------------------

/// Split `text` into a lowercase, punctuation-stripped word sequence.
///
/// Rules (all applied in order):
/// 1. Lowercase.
/// 2. Strip all ASCII punctuation characters (`!`, `"`, `#`, …, `/`, `:`, …,
///    `@`, `[`, …, `` ` ``, `{`, …, `~`).
/// 3. Split on ASCII whitespace.
/// 4. Drop empty tokens that remain after stripping.
///
/// This is intentionally simple. It handles the common case of trailing commas,
/// periods, and quotes without any language-specific logic. If your evaluation
/// pipeline needs number normalization or disfluency removal, apply those
/// transforms to both sides before calling [`compute_wer`].
pub fn tokenize_words(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split_ascii_whitespace()
        .map(|w| w.chars().filter(|c| !c.is_ascii_punctuation()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Split `text` into individual characters for CER computation.
///
/// Applies the same lowercase + punctuation-strip logic as [`tokenize_words`],
/// then yields every surviving character as a unit. Whitespace between words
/// is collapsed (multi-space becomes single space after `split_ascii_whitespace`
/// + rejoin), so the character sequence is comparable regardless of spacing.
pub fn tokenize_chars(text: &str) -> Vec<char> {
    // Re-join with a single space so inter-word spacing is canonical.
    let normalized = tokenize_words(text).join(" ");
    normalized.chars().collect()
}

// ---------------------------------------------------------------------------
// Levenshtein edit distance (word or character level)
// ---------------------------------------------------------------------------

/// Compute the Levenshtein edit distance between `ref_seq` and `hyp_seq` and
/// return the `(substitutions, insertions, deletions)` breakdown.
///
/// Uses the standard DP table with a single-row rolling optimization so memory
/// is `O(|ref|)` rather than `O(|ref| × |hyp|)`. The backtrace is implicit:
/// once we have the minimum-edit solution we recover the breakdown by walking
/// the full DP table, which adds a second `O(|ref| × |hyp|)` pass only when
/// the breakdown is needed (it always is here, since the struct exposes it).
///
/// For the tiny sequences in a normal ASR evaluation (a few hundred words at
/// most) this is always fast; the O(n²) concern only matters for book-length
/// texts.
fn levenshtein_breakdown<T: PartialEq>(ref_seq: &[T], hyp_seq: &[T]) -> (usize, usize, usize) {
    let n = ref_seq.len();
    let m = hyp_seq.len();

    // Full DP table for backtrace: dp[i][j] = edit distance between
    // ref[0..i] and hyp[0..j].
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate().take(n + 1) {
        row[0] = i; // delete i ref tokens
    }
    for (j, cell) in dp[0].iter_mut().enumerate().take(m + 1) {
        *cell = j; // insert j hyp tokens
    }
    for i in 1..=n {
        for j in 1..=m {
            if ref_seq[i - 1] == hyp_seq[j - 1] {
                dp[i][j] = dp[i - 1][j - 1]; // match
            } else {
                dp[i][j] = 1 + dp[i - 1][j - 1] // substitution
                    .min(dp[i][j - 1]) // insertion
                    .min(dp[i - 1][j]); // deletion
            }
        }
    }

    // Backtrace from (n, m) to (0, 0) to count operation types.
    let (mut subs, mut ins, mut dels) = (0, 0, 0);
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && ref_seq[i - 1] == hyp_seq[j - 1] {
            // Match — no edit.
            i -= 1;
            j -= 1;
        } else if i > 0 && j > 0 && dp[i][j] == dp[i - 1][j - 1] + 1 {
            // Substitution.
            subs += 1;
            i -= 1;
            j -= 1;
        } else if j > 0 && dp[i][j] == dp[i][j - 1] + 1 {
            // Insertion (extra hyp token).
            ins += 1;
            j -= 1;
        } else {
            // Deletion (dropped ref token).
            dels += 1;
            i -= 1;
        }
    }
    (subs, ins, dels)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the WER of `hypothesis` against `reference` at the word level.
///
/// Both strings are tokenized by [`tokenize_words`] before scoring, so
/// case and punctuation are ignored. Returns `None` in the `wer` field when
/// the reference tokenizes to zero words.
///
/// ```
/// use phoneme_core::wer::compute_wer;
///
/// let r = compute_wer("Hello, world!", "hello world");
/// assert_eq!(r.wer, Some(0.0));
///
/// let r = compute_wer("one two three", "one two");
/// assert_eq!(r.deletions, 1);
/// ```
pub fn compute_wer(reference: &str, hypothesis: &str) -> WerReport {
    let ref_words = tokenize_words(reference);
    let hyp_words = tokenize_words(hypothesis);
    let ref_units = ref_words.len();
    let (subs, ins, dels) = levenshtein_breakdown(&ref_words, &hyp_words);
    WerReport::compute(ref_units, subs, ins, dels)
}

/// Compute the CER of `hypothesis` against `reference` at the character level.
///
/// Both strings are normalized by [`tokenize_chars`] before scoring (lowercase,
/// punctuation stripped, inter-word spaces collapsed). Returns `None` in the
/// `wer` field when the reference normalizes to zero characters.
///
/// The returned [`WerReport`] uses the same struct; `ref_units` is the character
/// count of the normalized reference, and `wer` is the CER ratio.
///
/// ```
/// use phoneme_core::wer::compute_cer;
///
/// let r = compute_cer("cat", "cut");
/// assert_eq!(r.substitutions, 1);
/// assert_eq!(r.wer, Some(1.0 / 3.0));
/// ```
pub fn compute_cer(reference: &str, hypothesis: &str) -> WerReport {
    let ref_chars = tokenize_chars(reference);
    let hyp_chars = tokenize_chars(hypothesis);
    let ref_units = ref_chars.len();
    let (subs, ins, dels) = levenshtein_breakdown(&ref_chars, &hyp_chars);
    WerReport::compute(ref_units, subs, ins, dels)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- tokenize_words ------------------------------------------------------

    #[test]
    fn tokenize_lowercases_and_strips_punctuation() {
        assert_eq!(tokenize_words("Hello, World!"), vec!["hello", "world"]);
    }

    #[test]
    fn tokenize_handles_multiple_spaces() {
        assert_eq!(tokenize_words("a  b   c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn tokenize_empty_string_is_empty() {
        let v: Vec<String> = tokenize_words("");
        assert!(v.is_empty());
    }

    #[test]
    fn tokenize_all_punctuation_becomes_empty() {
        let v: Vec<String> = tokenize_words("!!! ???");
        assert!(v.is_empty());
    }

    // -- compute_wer — core cases -------------------------------------------

    #[test]
    fn identical_transcripts_are_zero() {
        let r = compute_wer("the cat sat", "the cat sat");
        assert_eq!(r.wer, Some(0.0));
        assert_eq!(r.substitutions, 0);
        assert_eq!(r.insertions, 0);
        assert_eq!(r.deletions, 0);
        assert_eq!(r.ref_units, 3);
    }

    #[test]
    fn case_and_punctuation_insensitive() {
        let r = compute_wer("Hello, World!", "hello world");
        assert_eq!(r.wer, Some(0.0));
    }

    #[test]
    fn one_substitution() {
        // "cat" → "bat": one sub out of 3 words → WER 1/3
        let r = compute_wer("the cat sat", "the bat sat");
        assert_eq!(r.substitutions, 1);
        assert_eq!(r.insertions, 0);
        assert_eq!(r.deletions, 0);
        assert_eq!(r.ref_units, 3);
        let wer = r.wer.unwrap();
        assert!((wer - 1.0 / 3.0).abs() < 1e-12, "got {wer}");
    }

    #[test]
    fn one_insertion() {
        // ref has 2 words, hyp has 3 → 1 insertion
        let r = compute_wer("hello world", "hello there world");
        assert_eq!(r.insertions, 1);
        assert_eq!(r.substitutions, 0);
        assert_eq!(r.deletions, 0);
        assert_eq!(r.ref_units, 2);
        let wer = r.wer.unwrap();
        assert!((wer - 0.5).abs() < 1e-12, "got {wer}");
    }

    #[test]
    fn one_deletion() {
        // ref has 3 words, hyp drops one → 1 deletion
        let r = compute_wer("one two three", "one three");
        assert_eq!(r.deletions, 1);
        assert_eq!(r.substitutions, 0);
        assert_eq!(r.insertions, 0);
        assert_eq!(r.ref_units, 3);
        let wer = r.wer.unwrap();
        assert!((wer - 1.0 / 3.0).abs() < 1e-12, "got {wer}");
    }

    #[test]
    fn empty_hypothesis_gives_wer_one() {
        // All ref words are deleted → WER = 1.0
        let r = compute_wer("hello world", "");
        assert_eq!(r.deletions, 2);
        assert_eq!(r.insertions, 0);
        assert_eq!(r.substitutions, 0);
        assert_eq!(r.wer, Some(1.0));
    }

    #[test]
    fn empty_reference_wer_is_none() {
        let r = compute_wer("", "something");
        assert_eq!(r.wer, None);
        assert_eq!(r.ref_units, 0);
    }

    #[test]
    fn both_empty_wer_is_none() {
        let r = compute_wer("", "");
        assert_eq!(r.wer, None);
        assert_eq!(r.ref_units, 0);
    }

    #[test]
    fn total_mismatch_wer_is_one() {
        // Every word is substituted → WER = 1.0
        let r = compute_wer("a b c", "x y z");
        assert_eq!(r.substitutions, 3);
        assert_eq!(r.wer, Some(1.0));
    }

    #[test]
    fn wer_can_exceed_one_with_many_insertions() {
        // ref = 1 word, hyp = 5 words: 0 subs, 4 ins, 0 dels → WER 4/1 = 4.0
        let r = compute_wer("ok", "ok a b c d");
        assert_eq!(r.insertions, 4);
        let wer = r.wer.unwrap();
        assert!(wer > 1.0, "expected wer > 1, got {wer}");
    }

    // -- compute_cer ---------------------------------------------------------

    #[test]
    fn cer_identical_is_zero() {
        let r = compute_cer("cat", "cat");
        assert_eq!(r.wer, Some(0.0));
    }

    #[test]
    fn cer_one_substitution() {
        // "cat" vs "cut": 1 sub out of 3 chars → CER 1/3
        let r = compute_cer("cat", "cut");
        assert_eq!(r.substitutions, 1);
        assert_eq!(r.ref_units, 3);
        let cer = r.wer.unwrap();
        assert!((cer - 1.0 / 3.0).abs() < 1e-12, "got {cer}");
    }

    #[test]
    fn cer_empty_reference_is_none() {
        let r = compute_cer("", "abc");
        assert_eq!(r.wer, None);
    }

    #[test]
    fn cer_strips_punctuation_same_as_wer() {
        // "Hi!" vs "Hi" — after stripping, both are "hi" → CER 0
        let r = compute_cer("Hi!", "Hi");
        assert_eq!(r.wer, Some(0.0));
    }
}
