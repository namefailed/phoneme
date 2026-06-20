//! Deterministic filler-word removal — a pure text transform, no LLM.
//!
//! The recording pipeline's `Transform`/`Enrichment` steps are LLM-driven; this
//! is the non-LLM alternative. A `FillerRemoval` Playbook step rewrites the
//! running transcript by stripping spoken filler at word boundaries and tidying
//! the spacing/punctuation the removal leaves behind — fast, offline, and
//! repeatable (the same input always yields the same output).
//!
//! Conservative by design: the default word list is the unambiguous noise
//! ("um", "uh", "er", …). The longer phrase list ("you know", "i mean",
//! "sort of", "kind of", "like") is gated behind [`FillerConfig::aggressive`]
//! and OFF by default, because those carry real meaning — "I *like* it",
//! "*kind of* blue" — and stripping them blindly mangles the text. The
//! per-recording polish for the dictation fast lane lives in
//! [`crate::dictation`]; this module is the pipeline-step form, configurable via
//! `[filler]`.

use crate::config::FillerConfig;

/// Strip filler words/phrases from `text` per `cfg`, then tidy the whitespace and
/// punctuation the removal leaves behind.
///
/// What it does, in order:
/// 1. remove configured filler **phrases** (multi-word, e.g. "you know") —
///    only when [`FillerConfig::aggressive`] is on, since the built-in phrases
///    are real words in other contexts;
/// 2. remove configured filler **words** (single words, e.g. "um"), matched
///    case-insensitively at word boundaries — never inside another word
///    ("umbrella" keeps its "um");
/// 3. collapse the doubled spaces, drifted spaces-before-punctuation, and
///    leading punctuation the removals can leave (" ," → ",", a leading ", "
///    dropped).
///
/// Pure: no I/O, no global state, deterministic. Empty/whitespace input (or a
/// transcript that was nothing but filler) returns an empty string.
///
/// ```
/// use phoneme_core::config::FillerConfig;
/// use phoneme_core::filler::strip_fillers;
/// let cfg = FillerConfig::default();
/// assert_eq!(strip_fillers("um so uh yeah", &cfg), "so yeah");
/// // "like" is opt-in (aggressive), so a default run keeps it.
/// assert_eq!(strip_fillers("I like it", &cfg), "I like it");
/// ```
pub fn strip_fillers(text: &str, cfg: &FillerConfig) -> String {
    // Phrases first (aggressive only): a phrase like "kind of" must be removed
    // as a unit before the single-word pass, which would otherwise leave a
    // stranded "of". Each phrase is matched on whole-word boundaries, case-
    // insensitively, so "kind of" matches but "mankind office" never does.
    let mut working = text.to_string();
    if cfg.aggressive {
        for phrase in &cfg.phrases {
            working = remove_phrase(&working, phrase);
        }
    }

    // Single words: keep non-filler tokens; for a removed filler keep only any
    // trailing punctuation it carried ("uh," -> ",") so the surrounding comma or
    // terminator survives — tidy() then reattaches it to the previous word and
    // collapses any doubling. Splitting on whitespace means a filler is only ever
    // matched standalone, so "umbrella" / "there" are never touched.
    let kept: Vec<String> = working
        .split_whitespace()
        .filter_map(|word| {
            if is_filler_word(word, &cfg.words) {
                let trailing: String = word
                    .chars()
                    .rev()
                    .take_while(|c| !c.is_alphanumeric())
                    .collect::<Vec<char>>()
                    .into_iter()
                    .rev()
                    .collect();
                (!trailing.is_empty()).then_some(trailing)
            } else {
                Some(word.to_string())
            }
        })
        .collect();

    tidy(&kept.join(" "))
}

/// Whether `word` (a whitespace-delimited token, punctuation and all) is one of
/// the configured filler `words`, compared case-insensitively against its
/// alphanumeric core — so "Um," and "UH." match "um"/"uh", but a token whose
/// core differs ("umbrella") never does.
fn is_filler_word(word: &str, words: &[String]) -> bool {
    let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
    if bare.is_empty() {
        return false;
    }
    words.iter().any(|f| bare.eq_ignore_ascii_case(f))
}

/// Remove every whole-word, case-insensitive occurrence of `phrase` from `text`,
/// leaving a single space where it stood (tidy() collapses it afterwards). A
/// blank phrase is ignored. Whole-word so "sort of" matches "Sort of" but a
/// phrase never bites into the middle of a longer word.
fn remove_phrase(text: &str, phrase: &str) -> String {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return text.to_string();
    }
    let lower_text = text.to_lowercase();
    let lower_phrase = phrase.to_lowercase();
    let plen = lower_phrase.len();

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0; // byte index into the original `text`
    let mut search = 0; // byte index into `lower_text`
    while let Some(rel) = lower_text[search..].find(&lower_phrase) {
        let start = search + rel;
        let end = start + plen;
        // Whole-word: the char before/after the match must be a boundary, never
        // alphanumeric — otherwise it is a substring of a bigger word, skip it.
        let before_ok = start == 0
            || !lower_text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric());
        let after_ok = end == lower_text.len()
            || !lower_text[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric());
        if before_ok && after_ok {
            out.push_str(&text[cursor..start]);
            out.push(' '); // placeholder; tidy() squeezes it out
            cursor = end;
        }
        search = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Collapse the whitespace and punctuation artifacts a filler removal leaves:
/// runs of spaces → one, a space before `,`/`.`/`!`/`?`/`;`/`:` dropped, a
/// leading orphan separator (", yeah" once the opener was stripped) removed,
/// and the result trimmed. Mirrors `dictation::normalize_spacing`'s intent so
/// the output never reads as " , word" or "word  word".
fn tidy(text: &str) -> String {
    const PUNCT: [char; 6] = [',', '.', '!', '?', ';', ':'];
    // One pass over whitespace-joined tokens: drop a space before punctuation, and
    // collapse a run of punctuation (possibly space-separated) down to the first
    // mark — so a filler removed between commas ("it was, , done") and a reattached
    // separator ("so , yeah") both read cleanly.
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    let mut last_nonspace: Option<char> = None;
    for c in collapsed.chars() {
        if PUNCT.contains(&c) {
            if out.ends_with(' ') {
                out.pop();
            }
            // Already sitting on a punctuation mark → this one is redundant.
            if last_nonspace.is_some_and(|p| PUNCT.contains(&p)) {
                continue;
            }
        }
        out.push(c);
        if c != ' ' {
            last_nonspace = Some(c);
        }
    }
    // A stripped leading filler can strand the separator it carried at the very
    // front (", yeah" / ". so"); drop any leading punctuation + the space after.
    out.trim_start_matches([',', '.', '!', '?', ';', ':', ' '])
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FillerConfig;

    #[test]
    fn strips_basic_fillers_default_list() {
        let cfg = FillerConfig::default();
        assert_eq!(strip_fillers("um so uh yeah", &cfg), "so yeah");
    }

    #[test]
    fn matching_is_case_insensitive() {
        let cfg = FillerConfig::default();
        assert_eq!(strip_fillers("Um, so UH, yeah", &cfg), "so, yeah");
        assert_eq!(strip_fillers("Hmm okay then", &cfg), "okay then");
    }

    #[test]
    fn keeps_real_words_containing_a_filler() {
        let cfg = FillerConfig::default();
        // "umbrella"/"there" embed "um"/"er" but are never standalone fillers.
        assert_eq!(
            strip_fillers("the umbrella is over there", &cfg),
            "the umbrella is over there"
        );
    }

    #[test]
    fn like_and_kind_of_are_kept_when_aggressive_off() {
        let cfg = FillerConfig::default(); // aggressive defaults OFF
        assert_eq!(strip_fillers("I like it", &cfg), "I like it");
        assert_eq!(
            strip_fillers("it was kind of blue", &cfg),
            "it was kind of blue"
        );
    }

    #[test]
    fn aggressive_strips_like_and_phrases() {
        let cfg = FillerConfig {
            aggressive: true,
            ..FillerConfig::default()
        };
        // "like" the filler goes; the whole-word phrases go as a unit (no
        // stranded "of"). What's left is the real content.
        assert_eq!(strip_fillers("I like it", &cfg), "I it");
        assert_eq!(
            strip_fillers("it was, you know, sort of done", &cfg),
            "it was, done"
        );
        assert_eq!(strip_fillers("that's kind of nice", &cfg), "that's nice");
    }

    #[test]
    fn aggressive_phrase_is_whole_word_only() {
        let cfg = FillerConfig {
            aggressive: true,
            ..FillerConfig::default()
        };
        // "mankind office" contains "kind of" as a substring across words only
        // by letters, not whole words — it must survive untouched.
        assert_eq!(
            strip_fillers("mankind official stuff", &cfg),
            "mankind official stuff"
        );
    }

    #[test]
    fn collapses_double_spaces_and_space_before_punctuation() {
        let cfg = FillerConfig::default();
        // Removing the interior "um" must not leave "report  is" or "report ,".
        assert_eq!(
            strip_fillers("the report um is done", &cfg),
            "the report is done"
        );
        assert_eq!(
            strip_fillers("the report um , done", &cfg),
            "the report, done"
        );
    }

    #[test]
    fn drops_leading_and_trailing_filler() {
        let cfg = FillerConfig::default();
        assert_eq!(
            strip_fillers("um the plan works uh", &cfg),
            "the plan works"
        );
        // A leading filler that carried a comma must not leave ", the".
        assert_eq!(strip_fillers("um, the plan works", &cfg), "the plan works");
    }

    #[test]
    fn empty_and_all_filler_yield_empty() {
        let cfg = FillerConfig::default();
        assert_eq!(strip_fillers("", &cfg), "");
        assert_eq!(strip_fillers("   ", &cfg), "");
        assert_eq!(strip_fillers("um uh er ah hmm", &cfg), "");
    }
}
