//! Auto-generated recording titles: a cheap text heuristic, with an optional
//! LLM pass on top.
//!
//! Only the heuristic lives here — it is pure text-in/text-out so it can be
//! tested without a daemon. The LLM pass (and the rule that an auto title
//! never overwrites a user-set one) is orchestrated by the pipeline.

/// Cap on a title's length, in characters. Cuts happen on a word boundary
/// below this, so real titles usually come out a little shorter.
const MAX_TITLE_CHARS: usize = 60;

/// Sentence/clause terminators. The first one ends the title — a title is the
/// first complete thought, not the whole transcript.
const TERMINATORS: &[char] = &['.', '!', '?', '…', ';'];

/// Leading filler words that say nothing about the content. Compared
/// case-insensitively with surrounding punctuation stripped, and skipped
/// repeatedly, so an opener like "Um, okay so" disappears as a unit.
const FILLERS: &[&str] = &[
    "um", "uh", "uhm", "erm", "er", "ah", "hmm", "mhm", "okay", "ok", "so", "alright", "well",
    "yeah", "anyway",
];

/// Derive a short display title from a transcript: the first meaningful
/// sentence or clause, minus leading filler and non-speech annotations,
/// whitespace collapsed, cut on a word boundary near `MAX_TITLE_CHARS` (60),
/// with no trailing punctuation.
///
/// Returns `None` when the text holds nothing usable (empty, whitespace,
/// annotations, or filler all the way down) — callers should then leave any
/// stored title alone rather than blanking it.
pub fn heuristic_title(text: &str) -> Option<String> {
    // Line by line: if the first line is filler-only ("Um, okay so"), the
    // next line gets its chance instead of the whole text yielding nothing.
    text.lines().find_map(title_from_line)
}

/// The title candidate from a single transcript line, or `None` when the line
/// contributes nothing (blank, annotation-only, filler-only).
fn title_from_line(line: &str) -> Option<String> {
    // Trim non-speech annotations and diarization markers off the front —
    // "[Music]", "(laughs)", "[Speaker 1]:" — so they can't become the title.
    let mut s = line.trim_start();
    while let Some(rest) =
        strip_leading_group(s, '[', ']').or_else(|| strip_leading_group(s, '(', ')'))
    {
        s = rest.trim_start_matches(':').trim_start();
    }

    let mut words: Vec<&str> = Vec::new();
    let mut len = 0usize; // chars, including joining spaces
    let mut skipping_fillers = true;
    for raw in s.split_whitespace() {
        if skipping_fillers {
            let bare: String = raw
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            if bare.is_empty() || FILLERS.contains(&bare.as_str()) {
                continue; // stray punctuation or filler — keep skipping
            }
            skipping_fillers = false;
        }

        // A terminator stuck to the end of a word ("trip.", "really?!") ends
        // the clause; the word itself is kept, the mark dropped. Interior
        // periods ("3.5", "v1.2.0") don't end anything.
        let word = raw.trim_end_matches(|c: char| TERMINATORS.contains(&c));
        let ends_clause = word.len() != raw.len();

        let word_chars = word.chars().count();
        if !word.is_empty() {
            if words.is_empty() && word_chars > MAX_TITLE_CHARS {
                // A single over-long "word" — unspaced text (CJK, a URL): cut
                // it at the cap on a char boundary, there is no word boundary.
                let cut: String = word.chars().take(MAX_TITLE_CHARS).collect();
                return finish(&cut);
            }
            let sep = if words.is_empty() { 0 } else { 1 };
            if len + sep + word_chars > MAX_TITLE_CHARS {
                break; // word boundary cut at the cap
            }
            words.push(word);
            len += sep + word_chars;
        }
        if ends_clause && !words.is_empty() {
            break;
        }
    }

    finish(&words.join(" "))
}

/// Final polish shared by both cut paths: drop trailing punctuation (a cap or
/// clause cut can leave "milk, eggs," or a dangling dash behind) and map an
/// empty result to `None`.
fn finish(joined: &str) -> Option<String> {
    let title = joined.trim_end_matches(|c: char| !c.is_alphanumeric());
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Strip one leading `<open>…<close>` group (e.g. `[Music]`), returning the
/// remainder, or `None` when the string doesn't start with such a group.
fn strip_leading_group(s: &str, open: char, close: char) -> Option<&str> {
    let rest = s.strip_prefix(open)?;
    let end = rest.find(close)?;
    Some(&rest[end + close.len_utf8()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace_yield_nothing() {
        assert_eq!(heuristic_title(""), None);
        assert_eq!(heuristic_title("   \n\t  \n"), None);
    }

    #[test]
    fn filler_only_text_yields_nothing() {
        assert_eq!(heuristic_title("um, uh... okay so. alright"), None);
        assert_eq!(heuristic_title("Hmm. Uh."), None);
    }

    #[test]
    fn takes_the_first_sentence_without_trailing_punctuation() {
        assert_eq!(
            heuristic_title("Plan the Denver trip. Also call the bank tomorrow."),
            Some("Plan the Denver trip".into())
        );
        assert_eq!(
            heuristic_title("Did the deploy finish? Check the logs."),
            Some("Did the deploy finish".into())
        );
    }

    #[test]
    fn strips_leading_filler_then_keeps_the_meat() {
        assert_eq!(
            heuristic_title("Um, okay so the quarterly numbers look fine."),
            Some("the quarterly numbers look fine".into())
        );
        assert_eq!(
            heuristic_title("Alright. Groceries for the week"),
            Some("Groceries for the week".into())
        );
    }

    #[test]
    fn skips_annotations_and_speaker_markers() {
        assert_eq!(
            heuristic_title("[Music] (laughs) Welcome back everyone."),
            Some("Welcome back everyone".into())
        );
        assert_eq!(
            heuristic_title("[Speaker 1]: Let's review the roadmap."),
            Some("Let's review the roadmap".into())
        );
    }

    #[test]
    fn filler_only_first_line_falls_through_to_the_next() {
        assert_eq!(
            heuristic_title("Um okay so\nShopping list for Saturday\nmilk eggs"),
            Some("Shopping list for Saturday".into())
        );
    }

    #[test]
    fn long_text_is_cut_at_a_word_boundary_under_the_cap() {
        let text = "This is a very long opening sentence that keeps going and going far beyond any sensible title length";
        let title = heuristic_title(text).unwrap();
        assert!(
            title.chars().count() <= 60,
            "got {} chars",
            title.chars().count()
        );
        assert!(
            text.starts_with(&title),
            "the cut keeps a prefix of the text"
        );
        assert!(!title.ends_with(' '), "no dangling separator");
        // Word boundary: the title's last word is complete in the source.
        let last = title.split_whitespace().last().unwrap();
        assert!(text.split_whitespace().any(|w| w == last));
    }

    #[test]
    fn short_text_is_kept_whole() {
        assert_eq!(heuristic_title("Buy milk"), Some("Buy milk".into()));
        assert_eq!(heuristic_title("Groceries"), Some("Groceries".into()));
    }

    #[test]
    fn unicode_text_counts_chars_not_bytes() {
        assert_eq!(
            heuristic_title("Réunion d'équipe à 9h. Détails plus tard."),
            Some("Réunion d'équipe à 9h".into())
        );
        // Unspaced CJK: one giant "word" — cut at the char cap, never inside
        // a code point.
        let cjk = "会議の議事録".repeat(20);
        let title = heuristic_title(&cjk).unwrap();
        assert_eq!(title.chars().count(), 60);
        assert!(cjk.starts_with(&title));
    }

    #[test]
    fn interior_periods_do_not_end_the_clause() {
        assert_eq!(
            heuristic_title("Upgrade to v2.5 before the demo"),
            Some("Upgrade to v2.5 before the demo".into())
        );
    }
}
