//! Re-derive the per-word / per-segment timing layers from an EDITED transcript.
//!
//! The Synced (per-word) and Timeline (per-segment) views — and click-to-seek —
//! are driven by `transcript_words` / `transcript_segments`, the "machine truth"
//! captured at transcription time. When the user edits the transcript *text*,
//! those layers drift: the prose says one thing, the timed words still say the
//! old thing. This module re-flows the existing timings onto the edited text by a
//! word-level diff so the views follow edits:
//!
//! - **Unchanged** words keep their exact original timing (and speaker).
//! - **Inserted** words are interpolated evenly into the time gap between the
//!   surrounding unchanged anchors.
//! - **Deleted** words drop out.
//!
//! There is **no model run** — it reuses the audio's already-known word timings,
//! so it's instant and works offline. Frame-accurate re-alignment of *edited*
//! words against the audio (true forced alignment) needs an aligner model and is
//! a roadmap item; for typed corrections, interpolation is indistinguishable in
//! practice and free.
//!
//! ## Speaker attribution is preserved, never invented
//!
//! A word's speaker comes from its `[Speaker N]` block marker when that marker is
//! the canonical numeric form; otherwise it **inherits** the matched original
//! word's speaker index. So a renamed `[Alice]` block (whose words still carry
//! the numeric index that `speaker_names` maps to "Alice") keeps displaying as
//! "Alice". Re-attributing text to a different speaker by hand-editing a marker is
//! intentionally out of scope here — that's the speaker-rename feature's job, and
//! trying to honor it would let a stray keystroke silently rewrite attribution.

use crate::types::{TranscriptSegment, TranscriptWord};

/// The re-derived timing layers for an edited transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Realigned {
    /// Whole-word timing layer (Synced view + click-to-seek), in reading order.
    pub words: Vec<TranscriptWord>,
    /// Sentence-grouped timing layer (Timeline view), in reading order.
    pub segments: Vec<TranscriptSegment>,
}

/// Re-derive words + segments for `edited_text` from the recording's existing
/// `old_words`. Returns `None` when re-alignment would be meaningless or
/// destructive, so the caller leaves the stored layers untouched:
/// - `old_words` is empty (no timings to borrow — e.g. a cloud transcript with
///   no per-word data, or a pre-word-capture recording);
/// - the edited text has no words (a cleared/whitespace transcript — don't wipe
///   the timeline on an accidental select-all-delete).
pub fn realign_transcript(edited_text: &str, old_words: &[TranscriptWord]) -> Option<Realigned> {
    if old_words.is_empty() {
        return None;
    }
    let old = merge_subword_tokens(old_words);
    if old.is_empty() {
        return None;
    }
    let edited = parse_edited(edited_text);
    if edited.is_empty() {
        return None;
    }

    // Align edited words to old whole-words by normalized text. `match_of[i]` is
    // the old-word index that edited word `i` reuses, or `None` if it's new.
    let match_of = align(&edited, &old);

    let words = build_words(&edited, &old, &match_of);
    let segments = build_segments(&edited, &words);
    Some(Realigned { words, segments })
}

// ---------------------------------------------------------------------------
// Old-side: collapse whisper subword tokens into whole words
// ---------------------------------------------------------------------------

/// A whole word reconstructed from the stored (possibly subword) tokens, with the
/// span that those tokens covered and the speaker the run was attributed to.
#[derive(Debug, Clone)]
struct OldWord {
    norm: String,
    start_ms: i64,
    end_ms: i64,
    speaker: Option<String>,
}

/// whisper.cpp emits subword + punctuation tokens, marking word starts with a
/// leading space (`leading_space == true`). Collapse each run (`true` then its
/// `false` continuations) into one whole word spanning the run, so the diff
/// compares like-for-like against the edited prose's whole words.
fn merge_subword_tokens(tokens: &[TranscriptWord]) -> Vec<OldWord> {
    let mut out: Vec<OldWord> = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let starts_word = i == 0 || t.leading_space;
        let n = normalize(&t.text);
        if starts_word || out.is_empty() {
            // A purely-punctuation token (empty after normalization) that starts
            // a "word" is folded into the previous word's span rather than
            // becoming an empty whole word the diff can never match.
            if n.is_empty() {
                if let Some(last) = out.last_mut() {
                    last.end_ms = last.end_ms.max(t.end_ms);
                    continue;
                }
            }
            out.push(OldWord {
                norm: n,
                start_ms: t.start_ms,
                end_ms: t.end_ms,
                speaker: t.speaker.clone(),
            });
        } else if let Some(last) = out.last_mut() {
            last.norm.push_str(&n);
            last.end_ms = last.end_ms.max(t.end_ms);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Edited-side: parse the `[Speaker N]` turn text into words
// ---------------------------------------------------------------------------

/// One word from the edited prose, tagged with the turn (block) it belongs to.
#[derive(Debug, Clone)]
struct EditedWord {
    /// The token exactly as the user typed it (spelling/case/punctuation kept).
    text: String,
    /// Normalized form used only for matching against [`OldWord::norm`].
    norm: String,
    /// Index of the `\n\n`-separated turn this word came from (segment grouping).
    block: usize,
    /// `Some("N")` when this word's turn started with a canonical `[Speaker N]`
    /// marker; `None` for an undiarized turn or a renamed/named marker (then the
    /// speaker is inherited from the matched original word).
    block_speaker: Option<String>,
}

/// Split the edited transcript into words, turn by turn. Turns are the `\n\n`
/// blocks the pipeline writes; a leading `[Speaker N]: ` / `[Name]: ` marker is
/// stripped (so it isn't tokenized as prose) and, when numeric, captured as the
/// turn's speaker.
fn parse_edited(text: &str) -> Vec<EditedWord> {
    let mut out = Vec::new();
    for (block, raw) in text.split("\n\n").enumerate() {
        let (block_speaker, body) = strip_marker(raw);
        for tok in body.split_whitespace() {
            let norm = normalize(tok);
            if norm.is_empty() {
                // Pure punctuation the user typed as its own token (e.g. a lone
                // "-"). Keep it as text on the previous word so spacing round-
                // trips, but it never participates in matching.
                if let Some(last) = out.last_mut() {
                    let last: &mut EditedWord = last;
                    if last.block == block {
                        last.text.push(' ');
                        last.text.push_str(tok);
                        continue;
                    }
                }
            }
            out.push(EditedWord {
                text: tok.to_string(),
                norm,
                block,
                block_speaker: block_speaker.clone(),
            });
        }
    }
    out
}

/// Strip a leading `[…]: ` speaker marker from a turn. Returns the numeric
/// speaker label (only for the canonical `[Speaker N]` form) and the remaining
/// body text. A non-numeric label (a rename like `[Alice]`) is still stripped but
/// reported as `None` so the speaker is inherited rather than guessed.
fn strip_marker(block: &str) -> (Option<String>, &str) {
    let trimmed = block.trim_start_matches([' ', '\n', '\t']);
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            let label = &rest[..close];
            let mut after = &rest[close + 1..];
            after = after.strip_prefix(':').unwrap_or(after);
            after = after.trim_start();
            let spk = label
                .strip_prefix("Speaker ")
                .and_then(|n| n.trim().parse::<u32>().ok())
                .map(|n| n.to_string());
            return (spk, after);
        }
    }
    (None, trimmed)
}

/// Lowercase and keep only alphanumerics — punctuation, case, and the leading-
/// space marker all wash out so "Don't," and "dont" match. Diff/matching only;
/// the original `text` is what gets stored.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Diff: align edited words to old whole-words (monotonic, by normalized text)
// ---------------------------------------------------------------------------

/// For each edited word, the index of the old whole-word it reuses (`Some`) or
/// `None` if it's newly inserted. The matching is monotonic (an LCS), so reused
/// old indices never go backwards — which keeps the rebuilt timeline ordered even
/// if the user shuffled words around.
fn align(edited: &[EditedWord], old: &[OldWord]) -> Vec<Option<usize>> {
    let n = edited.len();
    let m = old.len();
    let mut out = vec![None; n];

    // Trim the common prefix/suffix first: edits are usually local, so this keeps
    // the O(p·q) LCS below working only on the genuinely-changed middle.
    let mut lo = 0;
    while lo < n && lo < m && edited[lo].norm == old[lo].norm {
        out[lo] = Some(lo);
        lo += 1;
    }
    let mut hi_e = n;
    let mut hi_o = m;
    while hi_e > lo && hi_o > lo && edited[hi_e - 1].norm == old[hi_o - 1].norm {
        out[hi_e - 1] = Some(hi_o - 1);
        hi_e -= 1;
        hi_o -= 1;
    }

    // LCS over the residual edited[lo..hi_e] vs old[lo..hi_o].
    let p = hi_e - lo;
    let q = hi_o - lo;
    if p == 0 || q == 0 {
        return out;
    }
    // dp[i][j] = LCS length of edited[lo+i..] and old[lo+j..].
    let mut dp = vec![vec![0u32; q + 1]; p + 1];
    for i in (0..p).rev() {
        for j in (0..q).rev() {
            dp[i][j] = if edited[lo + i].norm == old[lo + j].norm {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < p && j < q {
        if edited[lo + i].norm == old[lo + j].norm {
            out[lo + i] = Some(lo + j);
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Assemble the new word layer (timings + speakers)
// ---------------------------------------------------------------------------

fn build_words(
    edited: &[EditedWord],
    old: &[OldWord],
    match_of: &[Option<usize>],
) -> Vec<TranscriptWord> {
    let n = edited.len();

    // Speakers: matched words take the old word's speaker; inserted words inherit
    // from the nearest matched neighbour (prev, else next). A numeric block marker
    // overrides both. Computed before timings so inheritance is independent of it.
    let mut inherited: Vec<Option<String>> = vec![None; n];
    for (i, m) in match_of.iter().enumerate() {
        if let Some(o) = m {
            inherited[i] = old[*o].speaker.clone();
        }
    }
    // forward-fill, then backward-fill, the gaps between matches
    let mut carry: Option<String> = None;
    for slot in inherited.iter_mut() {
        if slot.is_some() {
            carry = slot.clone();
        } else {
            *slot = carry.clone();
        }
    }
    let mut carry: Option<String> = None;
    for slot in inherited.iter_mut().rev() {
        if slot.is_some() {
            carry = slot.clone();
        } else {
            *slot = carry.clone();
        }
    }

    // Timings: matched words keep their span; runs of inserted words are spread
    // evenly across the time gap between the surrounding matched anchors.
    let span_start = old.first().map(|o| o.start_ms).unwrap_or(0);
    let span_end = old.last().map(|o| o.end_ms).unwrap_or(span_start);
    let mut starts = vec![0i64; n];
    let mut ends = vec![0i64; n];

    let mut i = 0;
    while i < n {
        match match_of[i] {
            Some(o) => {
                starts[i] = old[o].start_ms;
                ends[i] = old[o].end_ms;
                i += 1;
            }
            None => {
                // [i, j) is a maximal run of inserted words.
                let run_start = i;
                let mut j = i;
                while j < n && match_of[j].is_none() {
                    j += 1;
                }
                let a = if run_start == 0 {
                    span_start
                } else {
                    ends[run_start - 1]
                };
                let b = if j < n {
                    match_of[j].map(|o| old[o].start_ms).unwrap_or(span_end)
                } else {
                    span_end
                };
                let b = b.max(a); // never run backwards
                let k = (j - run_start) as i64;
                for (slot, idx) in (run_start..j).enumerate() {
                    let s = a + (b - a) * (slot as i64) / k;
                    let e = a + (b - a) * (slot as i64 + 1) / k;
                    starts[idx] = s;
                    ends[idx] = e;
                }
                i = j;
            }
        }
    }

    (0..n)
        .map(|i| {
            let speaker = edited[i]
                .block_speaker
                .clone()
                .or_else(|| inherited[i].clone());
            TranscriptWord {
                start_ms: starts[i],
                end_ms: ends[i],
                text: edited[i].text.clone(),
                // Whole, space-separated words — the Synced view joins on this.
                leading_space: true,
                speaker,
                // Interpolated/edited words have no provider confidence.
                confidence: None,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Assemble the new segment layer (sentence-grouped, per turn)
// ---------------------------------------------------------------------------

/// Group the rebuilt words into Timeline segments: split each turn into sentences
/// (on `.`/`?`/`!`) so the Timeline keeps phrase-level granularity, and stamp
/// each segment with the turn's speaker (numeric marker, else the run's dominant
/// inherited speaker). One segment per turn when a turn has no sentence break.
fn build_segments(edited: &[EditedWord], words: &[TranscriptWord]) -> Vec<TranscriptSegment> {
    let mut segments = Vec::new();
    let mut start = 0;
    while start < words.len() {
        let block = edited[start].block;
        // Find the end of this turn (block).
        let mut block_end = start;
        while block_end < words.len() && edited[block_end].block == block {
            block_end += 1;
        }
        // Within the turn, break into sentences.
        let mut s = start;
        while s < block_end {
            let mut e = s;
            while e < block_end {
                let ends_sentence = ends_sentence(&words[e].text);
                e += 1;
                if ends_sentence {
                    break;
                }
            }
            let speaker = edited[s]
                .block_speaker
                .clone()
                .or_else(|| dominant_speaker(words[s..e].iter().map(|w| w.speaker.as_deref())));
            let text = words[s..e]
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            segments.push(TranscriptSegment {
                start_ms: words[s].start_ms,
                end_ms: words[e - 1].end_ms,
                text,
                speaker,
            });
            s = e;
        }
        start = block_end;
    }
    segments
}

/// Whether a token ends a sentence — last non-quote/bracket char is `.`/`?`/`!`.
fn ends_sentence(tok: &str) -> bool {
    tok.trim_end_matches(['"', '\'', ')', ']', '»', '”', '’'])
        .ends_with(['.', '?', '!', '…'])
}

/// The most common speaker among a run of words (ties → first seen). Used for a
/// segment whose turn had no numeric marker (undiarized or renamed).
fn dominant_speaker<'a>(speakers: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for s in speakers.flatten() {
        if !counts.contains_key(s) {
            order.push(s);
        }
        *counts.entry(s).or_insert(0) += 1;
    }
    order
        .into_iter()
        .max_by_key(|s| counts[s])
        .map(|s| s.to_string())
}

#[cfg(test)]
#[path = "realign_test.rs"]
mod tests;
