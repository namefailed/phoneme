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
    text = finish_sentence(&text);
    // Voice commands run LAST: the steps above split on whitespace and would
    // collapse an inserted newline, so the literal `\n`/`\n\n` are applied to
    // the otherwise-polished text where they survive to the typed output.
    apply_voice_commands(&text)
}

/// Spoken editing/formatting commands, applied to already-polished dictation.
///
/// Conservative by design — only the three the roadmap names, and only when a
/// command is its **own sentence/segment**, so a literal "on a new line of
/// text" mid-sentence is left untouched (the boundary rule). Recognized:
/// - "new line" / "newline" → a single line break (`\n`)
/// - "new paragraph" → a blank line (`\n\n`)
/// - "scratch that" / "delete that" → drop the preceding sentence (the thing
///   just dictated), plus any line break sitting right after it.
///
/// A leading connective ("and new line", "then scratch that") still matches.
/// Anything not recognized is left exactly as dictated. With no command phrase
/// present the input is returned byte-for-byte (normal dictation is untouched).
pub fn apply_voice_commands(text: &str) -> String {
    const PHRASES: &[&str] = &[
        "new line",
        "newline",
        "new paragraph",
        "scratch that",
        "delete that",
    ];
    let lower = text.to_ascii_lowercase();
    if !PHRASES.iter().any(|c| lower.contains(c)) {
        return text.to_string();
    }

    enum Piece {
        Text(String),
        Break(&'static str),
    }
    let mut pieces: Vec<Piece> = Vec::new();

    for seg in split_sentences(text) {
        match normalize_command(&seg).as_str() {
            "new line" | "newline" => pieces.push(Piece::Break("\n")),
            "new paragraph" => pieces.push(Piece::Break("\n\n")),
            "scratch that" | "delete that" => {
                // Retract the most recent dictated sentence, discarding any line
                // break that immediately preceded the command.
                while let Some(top) = pieces.pop() {
                    if matches!(top, Piece::Text(_)) {
                        break;
                    }
                }
            }
            _ => pieces.push(Piece::Text(seg)),
        }
    }

    let mut out = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(&t);
            }
            Piece::Break(b) => out.push_str(b),
        }
    }
    // Editing can promote a mid-text sentence to the start of the result or of a
    // new line; re-capitalize those starts so the output still reads cleanly.
    capitalize_sentence_starts(out.trim())
}

/// Uppercase the first alphabetic character at the start of the text and after
/// each line break (intervening spaces are skipped).
fn capitalize_sentence_starts(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut at_start = true;
    for c in s.chars() {
        if at_start && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            at_start = false;
        } else {
            out.push(c);
            if c == '\n' {
                at_start = true;
            } else if !c.is_whitespace() {
                at_start = false;
            }
        }
    }
    out
}

/// Split text into sentence segments on terminal punctuation (`.`/`!`/`?`),
/// keeping the punctuation with its sentence; blanks are dropped.
fn split_sentences(text: &str) -> Vec<String> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?') {
            segs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        segs.push(cur);
    }
    segs.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Normalize a segment for command matching: lowercase, drop trailing sentence
/// punctuation, and strip a single leading connective ("and"/"then"/"ok").
fn normalize_command(seg: &str) -> String {
    let mut s = seg.trim().to_ascii_lowercase();
    s = s.trim_end_matches([' ', '.', '!', '?', ',']).to_string();
    for lead in ["and ", "then ", "ok ", "okay "] {
        if let Some(rest) = s.strip_prefix(lead) {
            s = rest.trim().to_string();
            break;
        }
    }
    s
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

/// Drop standalone filler words (case-insensitive). Punctuation the filler
/// carried is eaten with it but transferred to the previous word, so
/// "so, um, anyway" → "so, anyway" (not "so, , anyway") and "that's all um.
/// moving on" → "that's all. moving on" (not a run-on "that's all moving on").
fn strip_fillers(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
        let is_filler = !bare.is_empty() && FILLERS.iter().any(|f| bare.eq_ignore_ascii_case(f));
        if is_filler {
            // Keep the punctuation the filler carried: a trailing comma OR a
            // terminal `.`/`!`/`?` ("um," or "um.") moves to the previous word
            // once, so dropping the filler never merges two sentences into a
            // run-on. Never doubles a mark up when the previous word already
            // ends in sentence punctuation.
            let carried = word
                .chars()
                .last()
                .filter(|c| matches!(c, ',' | '.' | '!' | '?'));
            if let Some(mark) = carried {
                if let Some(prev) = out.last_mut() {
                    if !prev.ends_with([',', '.', '!', '?', ';', ':']) {
                        prev.push(mark);
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

/// The minimal end-edit to turn already-typed `streamed` text into `final_text`:
/// `(backspaces, insert)` — delete that many trailing characters, then type
/// `insert`. Shares the longest common prefix, so only the divergent tail is
/// rewritten.
///
/// This is streaming-type's stop reconciliation: words were typed live from the
/// rolling preview, and the final batch transcript is more accurate, so this
/// patches the typed text to the final one with the fewest visible keystrokes.
///
/// The returned `backspaces` is the number of UTF-16 code UNITS in the deleted
/// tail, not the number of Rust `char`s (audit M4). A Windows Backspace deletes
/// one UTF-16 code unit, so a deleted astral character (emoji, some CJK/IME
/// output) is two backspaces, not one — counting `char`s would under-delete and
/// leave a stray surrogate-pair half behind. The common-prefix split stays on
/// `char` boundaries (never mid-code-point); only the COUNT is in UTF-16 units.
pub fn reconcile_edit(streamed: &str, final_text: &str) -> (usize, String) {
    let s: Vec<char> = streamed.chars().collect();
    let f: Vec<char> = final_text.chars().collect();
    let common = s.iter().zip(f.iter()).take_while(|(a, b)| a == b).count();
    // Count the deleted tail in UTF-16 code units to match how a Windows
    // Backspace deletes (one code unit per press), so astral text round-trips.
    let backspaces: usize = s[common..].iter().map(|c| c.len_utf16()).sum();
    let insert: String = f[common..].iter().collect();
    (backspaces, insert)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_noop_when_identical() {
        assert_eq!(
            reconcile_edit("hello world", "hello world"),
            (0, String::new())
        );
    }

    #[test]
    fn reconcile_patches_only_the_divergent_tail() {
        // "wrld" → "world": common prefix "hello w", delete "rld" (3), type "orld".
        assert_eq!(
            reconcile_edit("hello wrld", "hello world"),
            (3, "orld".into())
        );
    }

    #[test]
    fn reconcile_handles_empty_sides() {
        assert_eq!(reconcile_edit("", "typed in"), (0, "typed in".into()));
        assert_eq!(reconcile_edit("backspace all", ""), (13, String::new()));
    }

    #[test]
    fn reconcile_backspaces_count_utf16_units_for_astral_text() {
        // A Windows Backspace deletes one UTF-16 code unit, so an astral emoji
        // (two code units) in the deleted tail is TWO backspaces, not one — a
        // char count would under-delete and strand a surrogate half (audit M4).
        // "hi 😀" → "hi" : the trailing " 😀" is space(1) + emoji(2) = 3 units.
        assert_eq!(reconcile_edit("hi 😀", "hi"), (3, String::new()));
        // Replacing the emoji keeps the "hi " prefix and deletes only the emoji
        // (2 units), then types the replacement.
        assert_eq!(reconcile_edit("hi 😀", "hi !"), (2, "!".into()));
    }

    #[test]
    fn reconcile_appends_when_final_extends_streamed() {
        // The common case: the final transcript just has more/finished words.
        assert_eq!(
            reconcile_edit("the meeting went", "the meeting went well."),
            (0, " well.".into())
        );
    }

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
    fn filler_transfers_its_terminal_punctuation_not_just_a_comma() {
        // A filler carrying a sentence-ender keeps the boundary: dropping "um."
        // must not merge two sentences into a run-on (audit LOW). fast_polish
        // only capitalizes the first letter, so the second sentence stays as
        // dictated — the point of this fix is the preserved `. ` boundary.
        assert_eq!(
            fast_polish("that's all um. moving on"),
            "That's all. moving on."
        );
        assert_eq!(fast_polish("done uh! next"), "Done! next.");
        assert_eq!(fast_polish("really uh? maybe"), "Really? maybe.");
        // Still doesn't double a mark the previous word already carries.
        assert_eq!(fast_polish("stop. um. go"), "Stop. go.");
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

    #[test]
    fn voice_command_new_line_when_its_own_segment() {
        // "new line" said as its own utterance → a line break between sentences.
        assert_eq!(
            fast_polish("first point. new line. second point."),
            "First point.\nSecond point."
        );
    }

    #[test]
    fn voice_command_new_paragraph() {
        assert_eq!(
            fast_polish("intro here. new paragraph. body here."),
            "Intro here.\n\nBody here."
        );
    }

    #[test]
    fn voice_command_scratch_that_drops_prior_sentence() {
        assert_eq!(
            fast_polish("send it tomorrow. scratch that. send it today."),
            "Send it today."
        );
    }

    #[test]
    fn voice_command_leading_connective_still_matches() {
        assert_eq!(
            fast_polish("line one. and new line. line two."),
            "Line one.\nLine two."
        );
    }

    #[test]
    fn literal_new_line_mid_sentence_is_not_a_command() {
        // The boundary rule: "new line" inside a longer sentence stays literal.
        assert_eq!(
            fast_polish("put it on a new line of text"),
            "Put it on a new line of text."
        );
    }

    #[test]
    fn no_command_phrase_leaves_text_untouched() {
        // The fast path: normal dictation must be byte-identical to fast_polish
        // without the voice-command step.
        assert_eq!(apply_voice_commands("Hello, world."), "Hello, world.");
    }
}
