//! Tests for transcript re-alignment ([`super`]).

use super::*;

/// Build a stored word token. `ls` = leading_space (whisper's word-start marker).
fn tok(start: i64, end: i64, text: &str, ls: bool, speaker: Option<&str>) -> TranscriptWord {
    TranscriptWord {
        start_ms: start,
        end_ms: end,
        text: text.to_string(),
        leading_space: ls,
        speaker: speaker.map(str::to_string),
        confidence: Some(0.9),
    }
}

/// The classic whisper subword case: "I don't know." emitted as
/// `I` / `don` `'t` / `know` `.` with leading-space markers on word starts.
fn dont_know_tokens(speaker: Option<&str>) -> Vec<TranscriptWord> {
    vec![
        tok(0, 100, "I", true, speaker),
        tok(100, 300, "don", true, speaker),
        tok(300, 400, "'t", false, speaker),
        tok(400, 600, "know", true, speaker),
        tok(600, 650, ".", false, speaker),
    ]
}

#[test]
fn no_old_words_returns_none() {
    assert!(realign_transcript("anything at all", &[]).is_none());
}

#[test]
fn empty_edit_returns_none_so_layers_are_preserved() {
    let old = dont_know_tokens(Some("1"));
    assert!(realign_transcript("", &old).is_none());
    assert!(realign_transcript("   \n\n  ", &old).is_none());
}

#[test]
fn unchanged_text_preserves_every_timing_and_speaker() {
    let old = dont_know_tokens(Some("1"));
    let r = realign_transcript("[Speaker 1]: I don't know.", &old).unwrap();

    // Subword tokens collapse to three whole words with the merged spans.
    assert_eq!(r.words.len(), 3);
    assert_eq!(r.words[0].text, "I");
    assert_eq!((r.words[0].start_ms, r.words[0].end_ms), (0, 100));
    assert_eq!(r.words[1].text, "don't");
    assert_eq!((r.words[1].start_ms, r.words[1].end_ms), (100, 400));
    assert_eq!(r.words[2].text, "know.");
    assert_eq!((r.words[2].start_ms, r.words[2].end_ms), (400, 650));
    assert!(r.words.iter().all(|w| w.speaker.as_deref() == Some("1")));
    assert!(r.words.iter().all(|w| w.leading_space));

    // One sentence → one segment spanning the whole turn.
    assert_eq!(r.segments.len(), 1);
    assert_eq!(r.segments[0].text, "I don't know.");
    assert_eq!((r.segments[0].start_ms, r.segments[0].end_ms), (0, 650));
    assert_eq!(r.segments[0].speaker.as_deref(), Some("1"));
}

#[test]
fn matched_words_keep_timing_inserts_interpolate() {
    // "I don't know." → "I do not really know." : two inserted words land in the
    // gap between the kept "I" (ends 100) and "know" (starts 400).
    let old = dont_know_tokens(Some("1"));
    let r = realign_transcript("[Speaker 1]: I do not really know.", &old).unwrap();

    assert_eq!(
        r.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
        vec!["I", "do", "not", "really", "know."]
    );
    // Kept anchors keep exact timings.
    assert_eq!((r.words[0].start_ms, r.words[0].end_ms), (0, 100));
    assert_eq!((r.words[4].start_ms, r.words[4].end_ms), (400, 650));
    // The three inserted words (do/not/really) fill [100, 400) in order, no gaps,
    // strictly non-decreasing and bounded by the anchors.
    assert_eq!(r.words[1].start_ms, 100);
    assert_eq!(r.words[3].end_ms, 400);
    for pair in r.words.windows(2) {
        assert!(pair[0].end_ms <= pair[1].start_ms || pair[0].end_ms == pair[1].start_ms);
        assert!(pair[1].start_ms >= pair[0].start_ms);
    }
    // Inserted words inherit the surrounding speaker.
    assert!(r.words.iter().all(|w| w.speaker.as_deref() == Some("1")));
}

#[test]
fn deletions_drop_out() {
    let old = dont_know_tokens(Some("1"));
    let r = realign_transcript("[Speaker 1]: I know.", &old).unwrap();
    assert_eq!(
        r.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
        vec!["I", "know."]
    );
    assert_eq!((r.words[0].start_ms, r.words[0].end_ms), (0, 100));
    assert_eq!((r.words[1].start_ms, r.words[1].end_ms), (400, 650));
}

#[test]
fn numeric_marker_is_authoritative_for_speaker() {
    // Two turns; words keep the marker's speaker even though the old tokens all
    // carried speaker "1" (e.g. the user split a turn).
    let old = vec![
        tok(0, 100, "hello", true, Some("1")),
        tok(100, 200, "there", true, Some("1")),
        tok(200, 300, "friend", true, Some("1")),
    ];
    let r = realign_transcript("[Speaker 1]: hello there\n\n[Speaker 2]: friend", &old).unwrap();
    assert_eq!(r.words.len(), 3);
    assert_eq!(r.words[0].speaker.as_deref(), Some("1"));
    assert_eq!(r.words[1].speaker.as_deref(), Some("1"));
    assert_eq!(r.words[2].speaker.as_deref(), Some("2"));
    // Two turns → two segments with the matching speaker labels.
    assert_eq!(r.segments.len(), 2);
    assert_eq!(r.segments[0].speaker.as_deref(), Some("1"));
    assert_eq!(r.segments[1].speaker.as_deref(), Some("2"));
}

#[test]
fn renamed_marker_inherits_the_underlying_index() {
    // After a rename the turn header is "[Alice]:" but the words still map to
    // index "1" via speaker_names — re-align must keep "1", not invent a label.
    let old = vec![
        tok(0, 100, "hello", true, Some("1")),
        tok(100, 200, "world", true, Some("1")),
    ];
    let r = realign_transcript("[Alice]: hello world", &old).unwrap();
    assert!(r.words.iter().all(|w| w.speaker.as_deref() == Some("1")));
    assert_eq!(r.segments[0].speaker.as_deref(), Some("1"));
}

#[test]
fn undiarized_text_stays_undiarized() {
    let old = vec![
        tok(0, 100, "just", true, None),
        tok(100, 200, "me", true, None),
    ];
    let r = realign_transcript("just me talking", &old).unwrap();
    assert!(r.words.iter().all(|w| w.speaker.is_none()));
    assert!(r.segments[0].speaker.is_none());
}

#[test]
fn sentences_split_into_separate_segments() {
    let old = vec![
        tok(0, 100, "Hello.", true, Some("1")),
        tok(100, 200, "How", true, Some("1")),
        tok(200, 300, "are", true, Some("1")),
        tok(300, 400, "you?", true, Some("1")),
    ];
    let r = realign_transcript("[Speaker 1]: Hello. How are you?", &old).unwrap();
    assert_eq!(r.segments.len(), 2);
    assert_eq!(r.segments[0].text, "Hello.");
    assert_eq!((r.segments[0].start_ms, r.segments[0].end_ms), (0, 100));
    assert_eq!(r.segments[1].text, "How are you?");
    assert_eq!((r.segments[1].start_ms, r.segments[1].end_ms), (100, 400));
}

#[test]
fn fully_rewritten_text_spreads_across_the_span() {
    // No words match — interpolate the new words across the whole [start,end].
    let old = dont_know_tokens(Some("1")); // span 0..650
    let r = realign_transcript("[Speaker 1]: completely different sentence", &old).unwrap();
    assert_eq!(r.words.len(), 3);
    assert_eq!(r.words.first().unwrap().start_ms, 0);
    assert_eq!(r.words.last().unwrap().end_ms, 650);
    // Strictly ordered across the span.
    for pair in r.words.windows(2) {
        assert!(pair[1].start_ms >= pair[0].start_ms);
    }
}

#[test]
fn reordered_text_keeps_timings_monotonic() {
    // The LCS only matches in order, so even a shuffle can't produce a timeline
    // that runs backwards (a hard requirement for the Timeline view).
    let old = vec![
        tok(0, 100, "alpha", true, Some("1")),
        tok(100, 200, "beta", true, Some("1")),
        tok(200, 300, "gamma", true, Some("1")),
    ];
    let r = realign_transcript("[Speaker 1]: gamma alpha beta", &old).unwrap();
    for pair in r.words.windows(2) {
        assert!(
            pair[1].start_ms >= pair[0].start_ms,
            "timeline must never run backwards: {:?}",
            r.words
        );
    }
}

#[test]
fn punctuation_and_case_differences_still_match() {
    let old = vec![
        tok(0, 100, "Hello", true, Some("1")),
        tok(100, 200, "World", true, Some("1")),
    ];
    // Different case + added punctuation — normalization makes these the same
    // words, so timings are preserved and nothing is treated as inserted.
    let r = realign_transcript("[Speaker 1]: hello, world!", &old).unwrap();
    assert_eq!(r.words.len(), 2);
    assert_eq!((r.words[0].start_ms, r.words[0].end_ms), (0, 100));
    assert_eq!((r.words[1].start_ms, r.words[1].end_ms), (100, 200));
    // The user's exact text (with punctuation) is kept.
    assert_eq!(r.words[0].text, "hello,");
    assert_eq!(r.words[1].text, "world!");
}
