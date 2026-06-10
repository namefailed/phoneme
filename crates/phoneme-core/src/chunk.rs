//! Sentence-aware transcript chunking for semantic search.
//!
//! ## Why chunk at all?
//!
//! The embedding model (`all-MiniLM-L6-v2`) truncates to 256 tokens and produces
//! a *single* mean-pooled vector for whatever it's given. If we embed a whole
//! transcript:
//!
//! - Anything past ~256 tokens is silently dropped — a five-minute note only
//!   ever embeds its first ~150 words, so the back half is unsearchable.
//! - Even within the window, mean-pooling a long passage smears many distinct
//!   ideas into one averaged vector. A query that paraphrases *one* sentence
//!   ("the thing about the database migration") barely moves the cosine against
//!   a vector that also averages in ten unrelated sentences. This is the core
//!   reason "utter the likeness of something I said" underperforms today.
//!
//! ## The fix
//!
//! Split the transcript into overlapping, sentence-aware windows of a few
//! sentences each, embed every window, and at query time score a recording by
//! its **best-matching** chunk (max-sim). A spoken idea then competes on its own
//! tight vector instead of being diluted by the rest of the note, and the
//! overlap keeps an idea that straddles a sentence boundary from being split.
//!
//! These functions are deliberately pure (no model, no DB) so the chunking
//! policy is unit-tested directly.

/// Target number of word-ish tokens per chunk. A chunk is grown sentence by
/// sentence until adding the next sentence would exceed this, so chunks land
/// *around* this size while still breaking on sentence boundaries.
///
/// ~80 words is well inside the model's 256-token limit (English averages
/// ~1.3 subword tokens/word, so ~80 words ≈ ~105 tokens — never truncated) yet
/// large enough to carry a complete thought rather than a bare clause.
pub const CHUNK_TARGET_WORDS: usize = 80;

/// How many sentences of context to carry over between consecutive chunks.
///
/// Overlap is what stops a single idea from being cut in half at a chunk
/// boundary: if a thought spans the end of one chunk and the start of the next,
/// repeating the boundary sentence(s) guarantees at least one chunk contains the
/// whole thought. 1 sentence keeps storage growth modest (~1 extra sentence per
/// chunk) while covering the common "idea split across the seam" case.
pub const CHUNK_OVERLAP_SENTENCES: usize = 1;

/// Hard cap on chunks produced for one transcript. A pathologically long note
/// (hours of dictation) would otherwise produce hundreds of embeddings; capping
/// keeps the per-recording embedding cost and the brute-force search bounded.
/// Reached only by very long recordings; typical voice notes produce 1–5 chunks.
pub const MAX_CHUNKS_PER_RECORDING: usize = 64;

/// Split `text` into sentence-like units.
///
/// This is intentionally lightweight — no NLP model. We break after `.`, `!`,
/// `?`, and newlines, which covers dictated speech and Whisper output well.
/// Abbreviations ("e.g.") may over-split, but that is harmless here: an extra
/// boundary just yields slightly smaller chunks, and the overlap re-joins
/// adjacent fragments. Returns trimmed, non-empty sentences in order.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        // A sentence ends at terminal punctuation or a hard line break. We keep
        // the punctuation attached so the embedded text reads naturally.
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            current.clear();
        }
    }
    // Trailing text with no terminal punctuation is still a sentence.
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        sentences.push(trimmed.to_string());
    }

    sentences
}

/// Approximate token/word count — whitespace-delimited words. Cheap and good
/// enough to keep chunks under the model limit; we never need exact tokenizer
/// counts here because [`CHUNK_TARGET_WORDS`] already leaves generous headroom.
fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// Break a transcript into overlapping, sentence-aware chunks suitable for
/// embedding.
///
/// Guarantees:
/// - Every chunk is non-empty and (barring a single sentence longer than the
///   target) stays near [`CHUNK_TARGET_WORDS`] words, comfortably under the
///   model's 256-token limit so no chunk is silently truncated.
/// - Consecutive chunks share [`CHUNK_OVERLAP_SENTENCES`] sentence(s) so an idea
///   spanning a boundary is wholly contained in at least one chunk.
/// - Short transcripts yield exactly one chunk (the whole text), so behavior for
///   one-liners is unchanged from the old whole-transcript embedding.
/// - At most [`MAX_CHUNKS_PER_RECORDING`] chunks are returned.
///
/// An empty / whitespace-only transcript yields no chunks.
pub fn chunk_transcript(text: &str) -> Vec<String> {
    let sentences = split_sentences(text);
    if sentences.is_empty() {
        return Vec::new();
    }

    // A short transcript is a single chunk — same as the historical behavior,
    // and avoids splitting a one-liner into awkward fragments.
    if word_count(text) <= CHUNK_TARGET_WORDS {
        return vec![sentences.join(" ")];
    }

    let mut chunks = Vec::new();
    let mut i = 0;
    while i < sentences.len() {
        let mut chunk_sentences: Vec<&str> = Vec::new();
        let mut words = 0;
        let mut j = i;
        // Grow the chunk sentence by sentence until we'd exceed the target.
        // Always include at least one sentence, even if it alone exceeds the
        // target (a single very long run-on still becomes its own chunk rather
        // than being dropped).
        while j < sentences.len() {
            let w = word_count(&sentences[j]);
            if !chunk_sentences.is_empty() && words + w > CHUNK_TARGET_WORDS {
                break;
            }
            chunk_sentences.push(&sentences[j]);
            words += w;
            j += 1;
        }

        chunks.push(chunk_sentences.join(" "));
        if chunks.len() >= MAX_CHUNKS_PER_RECORDING {
            break;
        }

        if j >= sentences.len() {
            break;
        }
        // Step the window forward, leaving `CHUNK_OVERLAP_SENTENCES` of the just-
        // emitted chunk as the start of the next. `max(i+1)` guarantees forward
        // progress so we can't loop forever when a single sentence fills a chunk.
        let advance = chunk_sentences.len().saturating_sub(CHUNK_OVERLAP_SENTENCES);
        i = (i + advance).max(i + 1);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sentences_breaks_on_terminal_punctuation() {
        let s = split_sentences("Hello there. How are you? I am fine! Done");
        assert_eq!(
            s,
            vec!["Hello there.", "How are you?", "I am fine!", "Done"]
        );
    }

    #[test]
    fn split_sentences_breaks_on_newlines() {
        // Dictated notes and Whisper output often use line breaks instead of
        // punctuation; those must still split.
        let s = split_sentences("first line\nsecond line\nthird");
        assert_eq!(s, vec!["first line", "second line", "third"]);
    }

    #[test]
    fn split_sentences_empty_input_yields_nothing() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   \n  ").is_empty());
    }

    #[test]
    fn short_transcript_is_a_single_chunk() {
        // A one-liner must embed as one chunk (unchanged from old behavior), not
        // be fragmented.
        let chunks = chunk_transcript("remind me to call the dentist tomorrow.");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "remind me to call the dentist tomorrow.");
    }

    #[test]
    fn empty_transcript_yields_no_chunks() {
        assert!(chunk_transcript("").is_empty());
        assert!(chunk_transcript("    ").is_empty());
    }

    #[test]
    fn long_transcript_splits_into_multiple_overlapping_chunks() {
        // Build a transcript well over the target so it must split.
        let sentence = "This is a sentence with several distinct words in it.";
        let words_per = word_count(sentence);
        // Enough sentences to exceed CHUNK_TARGET_WORDS at least twice.
        let n = (CHUNK_TARGET_WORDS / words_per) * 3 + 3;
        let transcript = vec![sentence; n].join(" ");

        let chunks = chunk_transcript(&transcript);
        assert!(
            chunks.len() >= 2,
            "a long transcript must split into multiple chunks, got {}",
            chunks.len()
        );
        // No chunk should be wildly over the target (allow one sentence of slop
        // plus the overlap sentence).
        for c in &chunks {
            assert!(
                word_count(c) <= CHUNK_TARGET_WORDS + words_per,
                "chunk exceeded target+slop: {} words",
                word_count(c)
            );
        }
    }

    #[test]
    fn chunks_overlap_at_boundaries() {
        // Each chunk after the first must begin with the trailing sentence of the
        // previous chunk, so an idea spanning the seam is never lost.
        let sentences: Vec<String> = (0..30)
            .map(|i| format!("Sentence number {i} carries some unique content here."))
            .collect();
        let transcript = sentences.join(" ");
        let chunks = chunk_transcript(&transcript);
        assert!(chunks.len() >= 2);

        for w in chunks.windows(2) {
            let prev_last = split_sentences(&w[0]).pop().unwrap();
            let next_first = split_sentences(&w[1]).first().unwrap().clone();
            assert_eq!(
                prev_last, next_first,
                "consecutive chunks must overlap by the boundary sentence"
            );
        }
    }

    #[test]
    fn a_single_overlong_sentence_still_becomes_one_chunk() {
        // A run-on with no terminal punctuation, longer than the target, must
        // not be dropped or loop forever — it becomes its own (truncatable) chunk.
        let long = vec!["word"; CHUNK_TARGET_WORDS * 2].join(" ");
        let chunks = chunk_transcript(&long);
        assert_eq!(chunks.len(), 1, "one overlong sentence => one chunk");
        assert!(chunks[0].split_whitespace().count() == CHUNK_TARGET_WORDS * 2);
    }

    #[test]
    fn chunk_count_is_capped() {
        // A pathologically long transcript can't blow up the embedding count.
        let sentence = "Short distinct sentence here now.";
        let transcript = vec![sentence; 5000].join(" ");
        let chunks = chunk_transcript(&transcript);
        assert!(
            chunks.len() <= MAX_CHUNKS_PER_RECORDING,
            "chunk count must be capped at {MAX_CHUNKS_PER_RECORDING}, got {}",
            chunks.len()
        );
    }
}
