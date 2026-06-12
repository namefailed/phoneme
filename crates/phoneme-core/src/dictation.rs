//! Dictation text polish — the zero-latency cleanup behind the in-place fast
//! lane (`[in_place].cleanup = "fast"`).
//!
//! Pure string rules, conservative by design: strip what is unambiguously
//! noise (filler words, whisper's non-speech annotations, stutter-doubled
//! words), then fix the mechanical bits (spacing, capitalization, terminal
//! punctuation). Anything that requires understanding the sentence —
//! self-corrections, tone, list formatting — is out of scope here; that's
//! what `cleanup = "llm"` is for.

/// Filler words dropped when they appear as standalone words. Deliberately
/// short: "like"/"so"/"well" carry meaning too often to strip safely.
const FILLERS: &[&str] = &["um", "uh", "uhm", "uhh", "umm", "erm", "ehm", "hmm"];

/// Apply the fast dictation polish to raw whisper output.
///
/// In order:
/// 1. drop whisper's bracketed/parenthesized non-speech annotations
///    (`[BLANK_AUDIO]`, `(coughs)`, `*music*`, …);
/// 2. drop standalone filler words (case-insensitive), eating one adjacent
///    comma so "so, um, anyway" → "so, anyway";
/// 3. collapse immediately doubled words ("the the" → "the");
/// 4. normalize whitespace and space-before-punctuation;
/// 5. capitalize the first letter and ensure terminal punctuation.
///
/// Empty/whitespace input returns an empty string.
pub fn fast_polish(raw: &str) -> String {
    let mut text = strip_annotations(raw);
    text = strip_fillers(&text);
    text = collapse_doubled_words(&text);
    text = normalize_spacing(&text);
    finish_sentence(&text)
}

/// Remove `[...]`, `(...)`, and `*...*` spans — whisper emits non-speech
/// annotations in these shapes (`[BLANK_AUDIO]`, `(upbeat music)`). Spans are
/// dropped only when SHORT (≤ 40 chars) and free of sentence punctuation, so
/// a real parenthetical the speaker dictated ("(see the attached doc)")
/// survives anything whisper would never emit as an annotation.
fn strip_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let (open, close) = match chars[i] {
            '[' => ('[', ']'),
            '(' => ('(', ')'),
            '*' => ('*', '*'),
            _ => {
                out.push(chars[i]);
                i += 1;
                continue;
            }
        };
        // Find the matching close within the annotation-size budget.
        let start = i + 1;
        let mut end = None;
        for (j, &c) in chars.iter().enumerate().skip(start) {
            if c == close {
                end = Some(j);
                break;
            }
            if j - start > 40 {
                break;
            }
        }
        match end {
            Some(j) => {
                let inner: String = chars[start..j].iter().collect();
                let looks_like_speech = inner.contains(['.', ',', '!', '?']) || inner.len() > 40;
                if looks_like_speech {
                    out.push(open);
                    out.push_str(&inner);
                    out.push(close);
                }
                i = j + 1;
            }
            None => {
                out.push(chars[i]);
                i = start;
            }
        }
    }
    out
}

/// Drop standalone filler words (case-insensitive). A comma immediately after
/// the filler is eaten with it, so "so, um, anyway" → "so, anyway" rather
/// than "so, , anyway".
fn strip_fillers(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
        let is_filler = !bare.is_empty() && FILLERS.iter().any(|f| bare.eq_ignore_ascii_case(f));
        if is_filler {
            // Keep punctuation the filler carried: "um," contributes its comma
            // to the previous word once ("well um, yes" → "well, yes"), but
            // never doubles one up.
            if word.ends_with(',') {
                if let Some(prev) = out.last_mut() {
                    if !prev.ends_with([',', '.', '!', '?', ';', ':']) {
                        prev.push(',');
                    }
                }
            }
            continue;
        }
        out.push(word.to_string());
    }
    out.join(" ")
}

/// Collapse an immediately repeated word — the classic dictation stutter
/// ("the the report", "I I think"). Case-insensitive on the comparison, keeps
/// the first occurrence, only when both are bare words (no punctuation), so
/// deliberate doubles like "that that" after a comma survive as typed… they
/// don't — by design: a true intentional double is far rarer in dictation
/// than the stutter, and the LLM mode exists for prose that needs fidelity.
fn collapse_doubled_words(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        if let Some(prev) = out.last() {
            let both_bare =
                prev.chars().all(char::is_alphanumeric) && word.chars().all(char::is_alphanumeric);
            if both_bare && prev.eq_ignore_ascii_case(word) {
                continue;
            }
        }
        out.push(word);
    }
    out.join(" ")
}

/// Collapse runs of whitespace and remove space before closing punctuation
/// (" hello , world ." → "hello, world.").
fn normalize_spacing(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
    {
        if matches!(c, ',' | '.' | '!' | '?' | ';' | ':') && prev_space {
            out.pop();
        }
        out.push(c);
        prev_space = c == ' ';
    }
    out
}

/// Capitalize the first alphabetic character and ensure the text ends with
/// sentence punctuation (adds a period when missing).
fn finish_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(trimmed.len() + 1);
    let mut capitalized = false;
    for c in trimmed.chars() {
        if !capitalized && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            capitalized = true;
        } else {
            out.push(c);
        }
    }
    if !out.ends_with(['.', '!', '?', '…', ':', '"', '\'']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polishes_a_typical_dictation() {
        assert_eq!(
            fast_polish("um so the the meeting went uh pretty well"),
            "So the meeting went pretty well."
        );
    }

    #[test]
    fn strips_whisper_annotations_but_keeps_real_parentheticals() {
        assert_eq!(
            fast_polish("[BLANK_AUDIO] hello there (coughs)"),
            "Hello there."
        );
        assert_eq!(
            fast_polish("see the doc (it has the numbers, all of them) for details"),
            "See the doc (it has the numbers, all of them) for details."
        );
    }

    #[test]
    fn eats_filler_commas_without_doubling() {
        assert_eq!(fast_polish("well um, yes"), "Well, yes.");
        assert_eq!(fast_polish("so, um, anyway"), "So, anyway.");
    }

    #[test]
    fn collapses_stutters_only_on_bare_words() {
        assert_eq!(
            fast_polish("I I think the the plan works"),
            "I think the plan works."
        );
        // The first "that" carries a comma → not a bare double, kept.
        assert_eq!(
            fast_polish("we know that, that one fails"),
            "We know that, that one fails."
        );
    }

    #[test]
    fn fixes_spacing_capitalization_and_terminal_punctuation() {
        assert_eq!(fast_polish("  hello ,  world  "), "Hello, world.");
        assert_eq!(fast_polish("already done!"), "Already done!");
    }

    #[test]
    fn empty_and_annotation_only_input_yield_empty() {
        assert_eq!(fast_polish("   "), "");
        assert_eq!(fast_polish("[BLANK_AUDIO]"), "");
    }
}
