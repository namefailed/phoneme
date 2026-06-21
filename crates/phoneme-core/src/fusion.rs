//! Hybrid retrieval: fusing the vector (semantic) ranking with the FTS5
//! (lexical) ranking, plus calibration of raw cosine scores into a relevance
//! percentage the UI can show.
//!
//! ## Why fuse instead of picking one?
//!
//! Vector search recalls by meaning ("the bit about the database migration"
//! matches "we need to move the schema over") but can miss an exact term it has
//! never seen: proper nouns, code identifiers, acronyms. Lexical FTS5 nails exact
//! terms but is blind to paraphrase. A user wants to recall "the likeness of
//! something I spoke about" and still find the recording when all they remember
//! is the one distinctive word. Fusing both gives the union of their strengths.
//!
//! ## Reciprocal Rank Fusion (RRF)
//!
//! RRF combines ranked lists without needing the two scoring scales to be
//! comparable (cosine sits in ~`[0,1]`; BM25 is unbounded and sign-flipped).
//! Each list contributes `1 / (k + rank)` per item, and contributions sum across
//! lists. An item ranked highly by either retriever floats up; an item ranked
//! well by both floats highest. That holds up far better than a single hard
//! cosine floor, which drops a genuinely relevant paraphrase hit the moment its
//! cosine sits just under the threshold.

use std::collections::HashMap;
use std::hash::Hash;

/// RRF dampening constant. The standard value from the original RRF paper
/// (Cormack et al., 2009). Larger `k` flattens the contribution curve so rank-1
/// and rank-10 differ less; 60 is the well-tested default and keeps top results
/// clearly ahead without letting a single list dominate.
pub const RRF_K: f32 = 60.0;

/// Fuse multiple ranked lists into one ranking via Reciprocal Rank Fusion.
///
/// Each input list is an ordered slice of item keys, best-first. The returned
/// vector is `(key, fused_score)` sorted best-first. A key absent from a list
/// just contributes nothing for that list (no penalty beyond the missing
/// reward), which is what we want: a strong semantic-only hit shouldn't be
/// punished for not also being a lexical hit.
///
/// `weights`, if provided, scales each list's contribution (same length as
/// `lists`); `None` weights every list equally. Weighting lets us, e.g., trust
/// the semantic list a little more for paraphrase queries.
///
/// ```
/// use phoneme_core::reciprocal_rank_fusion;
/// // `b` is only mid-ranked in each list, but it appears in *both*, so the
/// // fused ranking floats it to the top.
/// let semantic = ["a", "b"];
/// let lexical = ["c", "b"];
/// let fused = reciprocal_rank_fusion(&[&semantic[..], &lexical[..]], None);
/// assert_eq!(fused[0].0, "b");
/// ```
pub fn reciprocal_rank_fusion<K>(lists: &[&[K]], weights: Option<&[f32]>) -> Vec<(K, f32)>
where
    K: Eq + Hash + Clone,
{
    let mut scores: HashMap<K, f32> = HashMap::new();

    for (li, list) in lists.iter().enumerate() {
        let weight = weights.and_then(|w| w.get(li).copied()).unwrap_or(1.0);
        for (rank, key) in list.iter().enumerate() {
            // rank is 0-based; RRF uses 1-based, hence `rank + 1`.
            let contribution = weight / (RRF_K + (rank as f32 + 1.0));
            *scores.entry(key.clone()).or_insert(0.0) += contribution;
        }
    }

    let mut fused: Vec<(K, f32)> = scores.into_iter().collect();
    // Sort by fused score descending. Ties keep whatever order the HashMap gave
    // them, which is fine since tied items are by definition equally ranked.
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

/// Calibrate a raw cosine similarity from `all-MiniLM-L6-v2` into a 0..1
/// relevance score the UI can render as a percentage.
///
/// Raw cosine from a mean-pooled sentence-transformer doesn't read as a
/// percentage. For this model an excellent paraphrase match lands around
/// 0.55–0.75, a loosely related one around 0.3–0.4, and unrelated text hovers
/// near 0.1 rather than 0 (the embeddings live in a cone, so even random
/// sentences share a positive baseline). Showing the user "38% relevant" for
/// what is actually a strong hit would be misleading.
///
/// So map cosine through a piecewise-linear ramp anchored at thresholds that are
/// empirically reasonable for this model:
/// - at or below [`COSINE_FLOOR`] → 0.0 (treated as noise)
/// - at or above [`COSINE_CEIL`]  → 1.0 (an essentially exact match)
/// - linear in between.
///
/// This stretches the model's useful band across the full 0–100% the user sees,
/// so the chip reads as strong, medium, or weak the way a human would judge it.
pub fn calibrate_cosine(cosine: f32) -> f32 {
    let c = cosine.clamp(-1.0, 1.0);
    if c <= COSINE_FLOOR {
        0.0
    } else if c >= COSINE_CEIL {
        1.0
    } else {
        (c - COSINE_FLOOR) / (COSINE_CEIL - COSINE_FLOOR)
    }
}

/// Cosine at/below which a match is treated as unrelated noise (calibrates to
/// 0%). Sentence-transformer cosines for unrelated text sit around 0.05–0.15.
pub const COSINE_FLOOR: f32 = 0.15;

/// Cosine at/above which a match is treated as essentially exact (calibrates to
/// 100%). For this model, near-duplicate phrasings reach ~0.7+; capping there
/// keeps a genuinely strong paraphrase from being stuck forever at "70%".
pub const COSINE_CEIL: f32 = 0.70;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_rewards_items_ranked_well_by_both_lists() {
        // `b` is mid in each list but appears in both; `a` is #1 in list1 only;
        // `c` is #1 in list2 only. An item both lists like should be competitive.
        let list1 = ["a", "b", "x", "y"];
        let list2 = ["c", "b", "z", "w"];
        let fused = reciprocal_rank_fusion(&[&list1[..], &list2[..]], None);

        let score = |k: &str| fused.iter().find(|(key, _)| *key == k).unwrap().1;
        // `b` (rank 2 in both) must beat `x`/`y`/`z`/`w` (single-list, lower rank).
        assert!(score("b") > score("x"));
        assert!(score("b") > score("z"));
        // And `b` benefits from appearing twice vs `a`/`c` appearing once.
        assert!(score("b") > score("a"));
        assert!(score("b") > score("c"));
    }

    #[test]
    fn rrf_orders_a_single_list_by_rank() {
        let list = ["first", "second", "third"];
        let fused = reciprocal_rank_fusion(&[&list[..]], None);
        let keys: Vec<&str> = fused.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec!["first", "second", "third"]);
    }

    #[test]
    fn rrf_missing_from_one_list_is_not_penalized_below_its_reward() {
        // A semantic-only hit (`sem`) ranked #1 in list1 but absent from list2
        // must still score exactly its single-list contribution, not be dropped.
        let list1 = ["sem", "other"];
        let list2 = ["other"];
        let fused = reciprocal_rank_fusion(&[&list1[..], &list2[..]], None);
        let sem = fused.iter().find(|(k, _)| *k == "sem").unwrap().1;
        assert!((sem - (1.0 / (RRF_K + 1.0))).abs() < 1e-6);
    }

    #[test]
    fn rrf_weights_scale_list_influence() {
        // With list2 weighted to zero, ordering collapses to list1's order.
        let list1 = ["a", "b"];
        let list2 = ["b", "a"];
        let fused = reciprocal_rank_fusion(&[&list1[..], &list2[..]], Some(&[1.0, 0.0]));
        let keys: Vec<&str> = fused.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[test]
    fn calibrate_maps_floor_and_ceiling() {
        assert_eq!(calibrate_cosine(COSINE_FLOOR), 0.0);
        assert_eq!(calibrate_cosine(0.05), 0.0, "unrelated noise => 0%");
        assert_eq!(calibrate_cosine(COSINE_CEIL), 1.0);
        assert_eq!(calibrate_cosine(0.95), 1.0, "near-exact => 100%");
    }

    #[test]
    fn calibrate_is_monotonic_and_spreads_the_useful_band() {
        // A strong paraphrase (~0.6) should read clearly higher than a loose one
        // (~0.3), and both should be well inside (0,1) — not crushed near 0.
        let loose = calibrate_cosine(0.30);
        let strong = calibrate_cosine(0.60);
        assert!(strong > loose);
        assert!(loose > 0.0 && loose < 1.0);
        assert!(strong > 0.5, "a 0.6 cosine should read as a strong match");
    }

    #[test]
    fn calibrate_clamps_out_of_range_input() {
        assert_eq!(calibrate_cosine(2.0), 1.0);
        assert_eq!(calibrate_cosine(-2.0), 0.0);
    }
}
