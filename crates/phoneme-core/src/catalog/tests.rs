use super::*;

#[test]
fn replace_ignore_case_handles_counts_overlap_and_unicode() {
    // Basic multi-match, case-insensitive.
    let (n, s) = replace_ignore_case("The THE the", "the", "x");
    assert_eq!(n, 3);
    assert_eq!(s, "x x x");

    // No match → zero, original returned.
    let (n, s) = replace_ignore_case("hello", "zzz", "q");
    assert_eq!(n, 0);
    assert_eq!(s, "hello");

    // A replacement containing the needle doesn't recurse (we advance past the
    // inserted text), so "a" → "aa" over "aaa" yields exactly 3 replacements.
    let (n, s) = replace_ignore_case("aaa", "a", "aa");
    assert_eq!(n, 3);
    assert_eq!(s, "aaaaaa");

    // Unicode needle, mixed case, with surrounding multibyte text intact.
    let (n, s) = replace_ignore_case("Café CAFÉ déjà", "café", "tea");
    assert_eq!(n, 2);
    assert_eq!(s, "tea tea déjà");

    // Regex metacharacters in the needle are matched literally (escaped):
    // "a.b" matches "a.b"/"A.B" but not "axb".
    let (n, s) = replace_ignore_case("a.b axb A.B", "a.b", "Z");
    assert_eq!(n, 2);
    assert_eq!(s, "Z axb Z");

    // A `$`-bearing replacement is inserted verbatim (no capture expansion).
    let (n, s) = replace_ignore_case("price here", "price", "$5");
    assert_eq!(n, 1);
    assert_eq!(s, "$5 here");

    // Empty needle is a no-op (never an every-position match).
    let (n, s) = replace_ignore_case("text", "", "x");
    assert_eq!(n, 0);
    assert_eq!(s, "text");
}

#[test]
fn parse_status_round_trips_all_variants_incl_paused() {
    // Every status the DB can hold must round-trip through
    // `parse_status`/`as_str`. A missing arm (it once lacked "paused") makes
    // one unparseable row error the whole `list()`/`get()` query, not just that
    // row — so any new status needs an arm here and a case in this list.
    for s in [
        "recording",
        "paused",
        "queued",
        "transcribing",
        "cleaning_up",
        "summarizing",
        "tagging",
        "hook_running",
        "done",
        "transcribe_failed",
        "hook_failed",
        "cleanup_failed",
        "summarize_failed",
        "title_failed",
        "tag_failed",
        "cancelled",
    ] {
        assert_eq!(
            parse_status(s).unwrap().as_str(),
            s,
            "status {s} did not round-trip through parse_status/as_str"
        );
    }
}

#[tokio::test]
async fn update_error_persists_kind_and_message_and_a_retry_clears_them() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // A failure writes both columns on the row itself, so the reason
    // survives a restart (not just the live event / quarantine JSON).
    db.update_error(&r.id, "whisper_error", "the model file is missing")
        .await
        .unwrap();
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(got.error_kind.as_deref(), Some("whisper_error"));
    assert_eq!(
        got.error_message.as_deref(),
        Some("the model file is missing")
    );

    // A later successful (re-)transcription clears the stale failure reason
    // so the recording no longer reads as failed.
    db.update_transcript(&r.id, "clean text", "raw text", "tiny")
        .await
        .unwrap();
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(got.error_kind, None, "a successful retry clears error_kind");
    assert_eq!(
        got.error_message, None,
        "a successful retry clears error_message"
    );
}

#[tokio::test]
async fn ai_activity_round_trips_filters_and_orders_newest_first() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    // Migration applied: the table exists and starts empty.
    assert!(db.list_ai_activity(None, 50).await.unwrap().is_empty());

    // Two sessions for recording "a", one for "b".
    db.insert_ai_activity("a", "cleaning_up", "p1", "r1")
        .await
        .unwrap();
    db.insert_ai_activity("b", "summarizing", "p2", "r2")
        .await
        .unwrap();
    db.insert_ai_activity("a", "summarizing", "p3", "r3")
        .await
        .unwrap();

    // Global list is newest-first (id DESC == insert order DESC).
    let all = db.list_ai_activity(None, 50).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].response, "r3", "most recent session first");
    assert_eq!(all[2].response, "r1", "oldest session last");

    // The per-recording filter returns only that recording's sessions.
    let a = db.list_ai_activity(Some("a"), 50).await.unwrap();
    assert_eq!(a.len(), 2);
    assert!(a.iter().all(|e| e.recording_id == "a"));
    assert_eq!(a[0].stage, "summarizing", "newest 'a' session first");
    assert_eq!(a[1].stage, "cleaning_up");

    // `limit` caps the result.
    assert_eq!(db.list_ai_activity(None, 1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn ai_activity_caps_oversized_prompt_and_response() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // A normal-sized field round-trips verbatim — the popout still sees it all.
    db.insert_ai_activity("a", "cleaning_up", "short prompt", "short response")
        .await
        .unwrap();
    let small = &db.list_ai_activity(Some("a"), 1).await.unwrap()[0];
    assert_eq!(small.prompt, "short prompt");
    assert_eq!(small.response, "short response");

    // A field past the cap is truncated on a char boundary with a marker, so
    // the stored row can't grow without bound. Use multi-byte chars to prove
    // truncation never splits one (a byte-offset cut would panic or corrupt).
    let huge = "é".repeat(AI_ACTIVITY_FIELD_MAX_CHARS + 500);
    db.insert_ai_activity("b", "summarizing", &huge, &huge)
        .await
        .unwrap();
    let big = &db.list_ai_activity(Some("b"), 1).await.unwrap()[0];
    assert!(big.prompt.ends_with("… [truncated]"), "prompt not marked");
    assert!(
        big.response.ends_with("… [truncated]"),
        "response not marked"
    );
    // Kept chars = the cap; the marker is the only thing past it.
    let kept = big.prompt.chars().take(AI_ACTIVITY_FIELD_MAX_CHARS).count();
    assert_eq!(kept, AI_ACTIVITY_FIELD_MAX_CHARS);
    assert!(big.prompt.chars().count() < huge.chars().count());
}

#[tokio::test]
async fn dictation_history_inserts_lists_newest_first_and_gets_by_id() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    // Migration applied: the table exists and starts empty.
    assert!(db.list_dictation_history(50).await.unwrap().is_empty());

    db.insert_dictation_history("first dictation", Some("code"))
        .await
        .unwrap();
    db.insert_dictation_history("second dictation", None)
        .await
        .unwrap();
    db.insert_dictation_history("third dictation", Some("slack"))
        .await
        .unwrap();

    // Newest first (id DESC == insert order DESC); fields round-trip.
    let all = db.list_dictation_history(50).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].text, "third dictation");
    assert_eq!(all[0].app.as_deref(), Some("slack"));
    assert_eq!(all[0].char_count, "third dictation".chars().count() as i64);
    assert_eq!(all[1].text, "second dictation");
    assert_eq!(all[1].app, None, "a None app stores and reads back as NULL");
    assert_eq!(all[2].text, "first dictation");

    // `limit` caps the result (clamped to >= 1 too).
    assert_eq!(db.list_dictation_history(1).await.unwrap().len(), 1);
    assert_eq!(db.list_dictation_history(0).await.unwrap().len(), 1);

    // get-by-id: hit returns the text, miss returns None.
    let newest_id = all[0].id;
    assert_eq!(
        db.get_dictation_history(newest_id)
            .await
            .unwrap()
            .as_deref(),
        Some("third dictation")
    );
    assert_eq!(db.get_dictation_history(999_999).await.unwrap(), None);
}

#[tokio::test]
async fn dictation_history_prunes_to_keep_on_insert() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    // Insert more than the keep window; every insert prunes to the newest N.
    for i in 0..(DICTATION_HISTORY_KEEP + 10) {
        db.insert_dictation_history(&format!("dictation {i}"), None)
            .await
            .unwrap();
    }
    let all = db
        .list_dictation_history(DICTATION_HISTORY_KEEP)
        .await
        .unwrap();
    assert_eq!(all.len() as i64, DICTATION_HISTORY_KEEP);
    // The newest entry is the last inserted; the oldest kept is offset by 10.
    assert_eq!(
        all[0].text,
        format!("dictation {}", DICTATION_HISTORY_KEEP + 10 - 1)
    );
    assert_eq!(all[all.len() - 1].text, "dictation 10");
}

#[tokio::test]
async fn dictation_history_delete_and_clear_report_counts() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    db.insert_dictation_history("a", None).await.unwrap();
    db.insert_dictation_history("b", None).await.unwrap();
    let rows = db.list_dictation_history(50).await.unwrap();
    let an_id = rows[0].id;

    // delete returns true for a hit, false for an unknown id.
    assert!(db.delete_dictation_history(an_id).await.unwrap());
    assert!(!db.delete_dictation_history(an_id).await.unwrap());
    assert_eq!(db.list_dictation_history(50).await.unwrap().len(), 1);

    // clear returns the number of rows removed, then leaves an empty table.
    assert_eq!(db.clear_dictation_history().await.unwrap(), 1);
    assert!(db.list_dictation_history(50).await.unwrap().is_empty());
    assert_eq!(db.clear_dictation_history().await.unwrap(), 0);
}

#[tokio::test]
async fn dictation_history_caps_oversized_text_but_keeps_real_length() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    // Multi-byte chars prove the char-boundary cut never splits a character.
    let huge = "é".repeat(DICTATION_HISTORY_TEXT_MAX_CHARS + 500);
    db.insert_dictation_history(&huge, None).await.unwrap();
    let row = &db.list_dictation_history(1).await.unwrap()[0];
    assert!(row.text.ends_with("… [truncated]"), "text not marked");
    // The stored text is capped at the ceiling (+ marker); char_count reports the
    // dictation's REAL length, not the truncated one.
    let kept = row
        .text
        .chars()
        .take(DICTATION_HISTORY_TEXT_MAX_CHARS)
        .count();
    assert_eq!(kept, DICTATION_HISTORY_TEXT_MAX_CHARS);
    assert_eq!(row.char_count, huge.chars().count() as i64);
}

#[tokio::test]
async fn list_filters_kind_and_favorites_in_sql_before_pagination() {
    // Kind and favorites must be filtered in SQL, before LIMIT/OFFSET. Doing it
    // client-side after pagination lets a page contain almost none of the
    // chosen kind, and leaves favorites past the first page unreachable.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    // 30 single voice notes, the first 25 of them starred…
    let mut singles = Vec::new();
    for i in 0..30 {
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();
        if i < 25 {
            db.set_favorite(&r.id, true).await.unwrap();
        }
        singles.push(r.id);
    }
    // …plus 5 meeting tracks (never starred).
    for _ in 0..5 {
        db.insert(&embedded_recording(Some("meeting-1")))
            .await
            .unwrap();
    }

    // Kind filters match on meeting_id presence, across the whole set.
    let single_only = db
        .list(&ListFilter {
            kind: Some(crate::types::ListKind::Single),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(single_only.len(), 30);
    assert!(single_only.iter().all(|r| r.meeting_id.is_none()));

    let meeting_only = db
        .list(&ListFilter {
            kind: Some(crate::types::ListKind::Meeting),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(meeting_only.len(), 5);
    assert!(meeting_only.iter().all(|r| r.meeting_id.is_some()));

    // The crux: page 3 of the favorites view (limit 10, offset 20) must hold
    // the remaining 5 starred recordings. With post-pagination filtering this
    // page would be empty or full of unstarred rows.
    let fav_page3 = db
        .list(&ListFilter {
            favorite: Some(true),
            limit: Some(10),
            offset: Some(20),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        fav_page3.len(),
        5,
        "favorites beyond page 1 must be reachable (25 favorites → 5 on page 3)"
    );
    assert!(fav_page3.iter().all(|r| r.favorite));

    // Some(false) is the complement: only unstarred rows.
    let unstarred = db
        .list(&ListFilter {
            favorite: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        unstarred.len(),
        10,
        "5 unstarred singles + 5 meeting tracks"
    );
    assert!(unstarred.iter().all(|r| !r.favorite));

    // Kind + favorites compose.
    let fav_singles = db
        .list(&ListFilter {
            kind: Some(crate::types::ListKind::Single),
            favorite: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(fav_singles.len(), 25);
}

#[tokio::test]
async fn list_pins_float_to_top_and_filters_in_sql() {
    // Pinned recordings always sort first (independent of the date order), and
    // the `pinned` filter / `set_pinned` round-trip work like favorites.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // Insert oldest → newest so the natural (newest-first) order is the reverse
    // of insertion; pin the OLDEST one so we can prove it jumps to the front.
    let mut ids = Vec::new();
    for i in 0..5 {
        // Distinct started_at via from_datetime so the date sort is deterministic.
        let dt = Local::now() - chrono::Duration::minutes((10 - i) as i64);
        let id = RecordingId::from_datetime(dt);
        let r = Recording {
            id: id.clone(),
            started_at: dt,
            ..embedded_recording(None)
        };
        db.insert(&r).await.unwrap();
        ids.push(id);
    }
    let oldest = ids[0].clone();

    // No pin yet: pure newest-first, so the oldest is last.
    let before = db.list(&ListFilter::default()).await.unwrap();
    assert_eq!(before.last().unwrap().id.as_str(), oldest.as_str());
    assert!(before.iter().all(|r| !r.pinned));

    // Pin the oldest: it must now lead the list, ahead of the newer rows.
    db.set_pinned(&oldest, true).await.unwrap();
    let after = db.list(&ListFilter::default()).await.unwrap();
    assert_eq!(
        after.first().unwrap().id.as_str(),
        oldest.as_str(),
        "a pinned recording floats to the top regardless of date order"
    );
    assert!(after.first().unwrap().pinned);

    // Pins still lead even when the user sorts oldest-first (pinned DESC wins).
    let asc = db
        .list(&ListFilter {
            sort_desc: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(asc.first().unwrap().id.as_str(), oldest.as_str());

    // The `pinned` filter keeps only pinned rows; `Some(false)` is the complement.
    let only_pinned = db
        .list(&ListFilter {
            pinned: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(only_pinned.len(), 1);
    assert!(only_pinned.iter().all(|r| r.pinned));
    let unpinned = db
        .list(&ListFilter {
            pinned: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(unpinned.len(), 4);
    assert!(unpinned.iter().all(|r| !r.pinned));

    // Unpinning reverts the sort: the oldest drops back to last.
    db.set_pinned(&oldest, false).await.unwrap();
    let reverted = db.list(&ListFilter::default()).await.unwrap();
    assert_eq!(reverted.last().unwrap().id.as_str(), oldest.as_str());

    // The KindCounts pinned badge tracks the live count.
    db.set_pinned(&ids[1], true).await.unwrap();
    db.set_pinned(&ids[2], true).await.unwrap();
    let counts = db.kind_counts().await.unwrap();
    assert_eq!(counts.pinned, 2);
}

#[test]
fn test_sanitize_fts5_query() {
    // Bare words become quoted prefix terms, AND-ed.
    assert_eq!(sanitize_fts5_query("hello"), "\"hello\"*");
    assert_eq!(
        sanitize_fts5_query("hello world"),
        "\"hello\"* AND \"world\"*"
    );
    assert_eq!(sanitize_fts5_query("   spaces   "), "\"spaces\"*");
    assert_eq!(sanitize_fts5_query(""), "");

    // Punctuation inside a term is kept (quoted, for FTS5 to tokenize) rather
    // than stripped, so these stay single terms instead of prefix-AND soup.
    assert_eq!(sanitize_fts5_query("O'Connor"), "\"O'Connor\"*");
    assert_eq!(sanitize_fts5_query("react-router"), "\"react-router\"*");
    assert_eq!(
        sanitize_fts5_query("std::collections::HashMap"),
        "\"std::collections::HashMap\"*"
    );

    // An explicitly quoted span is an exact phrase (no trailing prefix star).
    assert_eq!(sanitize_fts5_query("\"fix the bug\""), "\"fix the bug\"");
    // A quoted phrase plus a trailing bare word: phrase exact + word prefix.
    assert_eq!(
        sanitize_fts5_query("\"fix the bug\" now"),
        "\"fix the bug\" AND \"now\"*"
    );

    // Injection attempt: the user's quotes are consumed as phrase delimiters,
    // never passed through raw, so the output's quotes stay balanced and the
    // FTS5 operators in the payload become literal phrase tokens.
    assert_eq!(
        sanitize_fts5_query("foo\" OR bar"),
        "\"foo\"* AND \"OR bar\""
    );
}

/// A minimal `Done` recording for embedding/search tests. `semantic_search`
/// JOINs embeddings to recordings, so the row must exist before embedding.
fn embedded_recording(meeting_id: Option<&str>) -> Recording {
    Recording {
        id: RecordingId::new(),
        started_at: Local::now(),
        duration_ms: 1000,
        audio_path: "x.wav".into(),
        transcript: Some("t".into()),
        model: Some("tiny".into()),
        status: RecordingStatus::Done,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: meeting_id.map(|s| s.to_string()),
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    }
}

#[tokio::test]
async fn semantic_search_ranks_by_cosine_and_respects_limit() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    let b = embedded_recording(None);
    let c = embedded_recording(None);
    for r in [&a, &b, &c] {
        db.insert(r).await.unwrap();
    }
    // Orthonormal vectors: query [1,0,0] is identical to `a`, orthogonal to b/c.
    db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
    db.upsert_embedding(&b.id, &[0.0, 1.0, 0.0]).await.unwrap();
    db.upsert_embedding(&c.id, &[0.0, 0.0, 1.0]).await.unwrap();

    // min_score -1.0 keeps everything so we can assert ordering.
    let results = db
        .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
        .await
        .unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0.as_str(), a.id.as_str(), "best match first");
    assert!(
        (results[0].1 - 1.0).abs() < 1e-6,
        "identical vector scores ~1.0"
    );
    assert!(
        results[0].1 >= results[1].1 && results[1].1 >= results[2].1,
        "results must be sorted high→low"
    );

    // `limit` caps the result count.
    let top1 = db.semantic_search(&[1.0, 0.0, 0.0], 1, -1.0).await.unwrap();
    assert_eq!(top1.len(), 1);
    assert_eq!(top1[0].0.as_str(), a.id.as_str());
}

#[tokio::test]
async fn semantic_search_min_score_filters_low_matches() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
    // Orthogonal query → cosine 0.0, under a 0.5 floor → dropped.
    let results = db.semantic_search(&[0.0, 1.0, 0.0], 10, 0.5).await.unwrap();
    assert!(
        results.is_empty(),
        "below-floor matches must be filtered out"
    );
}

#[tokio::test]
async fn semantic_search_skips_dimension_mismatch_without_panicking() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let good = embedded_recording(None);
    let bad = embedded_recording(None);
    db.insert(&good).await.unwrap();
    db.insert(&bad).await.unwrap();
    db.upsert_embedding(&good.id, &[1.0, 0.0, 0.0])
        .await
        .unwrap();
    // Wrong dimension (2 vs the query's 3): must be skipped, not scored on a
    // truncated prefix, and not panic.
    db.upsert_embedding(&bad.id, &[1.0, 0.0]).await.unwrap();

    let results = db
        .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "the mismatched-dim embedding is skipped");
    assert_eq!(results[0].0.as_str(), good.id.as_str());
}

#[tokio::test]
async fn semantic_search_collapses_meeting_tracks() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mic = embedded_recording(Some("meeting-1"));
    let sys = embedded_recording(Some("meeting-1"));
    let solo = embedded_recording(None);
    for r in [&mic, &sys, &solo] {
        db.insert(r).await.unwrap();
    }
    // Both tracks of meeting-1 are highly similar to the query; solo isn't.
    db.upsert_embedding(&mic.id, &[1.0, 0.0, 0.0])
        .await
        .unwrap();
    db.upsert_embedding(&sys.id, &[0.99, 0.01, 0.0])
        .await
        .unwrap();
    db.upsert_embedding(&solo.id, &[0.0, 1.0, 0.0])
        .await
        .unwrap();

    let results = db
        .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
        .await
        .unwrap();
    // The meeting's two tracks collapse to one entry (best-scoring track),
    // alongside the standalone recording.
    assert_eq!(results.len(), 2);
    let meeting_hits = results
        .iter()
        .filter(|(id, _)| id.as_str() == mic.id.as_str() || id.as_str() == sys.id.as_str())
        .count();
    assert_eq!(
        meeting_hits, 1,
        "meeting tracks must collapse to one result"
    );
}

#[tokio::test]
async fn upsert_chunk_embeddings_replaces_prior_chunks() {
    // Re-embedding (a re-transcription or a manual edit) must replace a
    // recording's chunk vectors, never leave stale ones from the old text
    // behind. Otherwise an edited note keeps matching phrases it no longer
    // contains. Store three chunks, then re-embed with two and assert the third
    // is gone.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    db.insert(&a).await.unwrap();

    db.upsert_chunk_embeddings(
        &a.id,
        &[
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ],
    )
    .await
    .unwrap();

    // A query identical to the second chunk finds the recording.
    let r = db.vector_ranking(&[0.0, 1.0, 0.0]).await.unwrap();
    assert_eq!(r.len(), 1);
    assert!((r[0].2 - 1.0).abs() < 1e-6, "best chunk is the exact match");

    // Re-embed with only two chunks; the third (z-axis) must be dropped.
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]])
        .await
        .unwrap();
    // The old z-axis chunk is gone: a z-axis query now only matches by the
    // shared positive baseline (here, exactly 0 against the two remaining
    // orthogonal chunks), not 1.0.
    let r2 = db.vector_ranking(&[0.0, 0.0, 1.0]).await.unwrap();
    assert!(
        r2.is_empty() || r2[0].2 < 0.5,
        "stale chunk must not survive a re-embed (got {r2:?})"
    );

    // Empty re-embed clears all chunks.
    db.upsert_chunk_embeddings(&a.id, &[]).await.unwrap();
    let none = db.list_recordings_without_chunk_embeddings().await.unwrap();
    assert!(
        none.iter().any(|rec| rec.id.as_str() == a.id.as_str()),
        "after clearing, the recording reappears as needing chunks"
    );
}

#[tokio::test]
async fn vector_ranking_scores_by_best_chunk_not_average() {
    // The core of paraphrase recall: a recording is ranked by its best-matching
    // chunk (max-sim), not by an averaged whole-note vector. Recording `a` has
    // many unrelated chunks plus one chunk that nails the query; it must still
    // rank top, because that one chunk competes on its own tight vector instead
    // of being diluted by the rest of the note.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    let b = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.insert(&b).await.unwrap();

    // `a`: one chunk exactly on the query axis, several pulling other ways.
    db.upsert_chunk_embeddings(
        &a.id,
        &[
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![1.0, 0.0, 0.0], // the matching chunk
            vec![0.0, 1.0, 0.0],
        ],
    )
    .await
    .unwrap();
    // `b`: a single chunk only loosely aligned with the query.
    db.upsert_chunk_embeddings(&b.id, &[vec![0.6, 0.8, 0.0]])
        .await
        .unwrap();

    let ranking = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    assert_eq!(ranking.len(), 2);
    assert_eq!(
        ranking[0].1.as_str(),
        a.id.as_str(),
        "the recording with the best single chunk wins (max-sim, not mean)"
    );
    assert!(
        (ranking[0].2 - 1.0).abs() < 1e-6,
        "best-chunk cosine is the exact-match chunk's score, not an average"
    );
}

#[tokio::test]
async fn vector_ranking_falls_back_to_legacy_whole_recording_vector() {
    // During the backfill window a recording may still have only a legacy
    // whole-recording vector and no chunks. It must remain searchable via the
    // `embeddings` table fallback, and once chunks exist they supersede the
    // legacy vector (no double-counting, no stale legacy score winning).
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let legacy_only = embedded_recording(None);
    let chunked = embedded_recording(None);
    db.insert(&legacy_only).await.unwrap();
    db.insert(&chunked).await.unwrap();

    // legacy_only: only the old whole-recording vector, loosely on-axis.
    db.upsert_embedding(&legacy_only.id, &[0.8, 0.6, 0.0])
        .await
        .unwrap();
    // chunked: a stale legacy vector plus a fresh, better chunk vector. The
    // chunk must win; the legacy row must be ignored for this recording.
    db.upsert_embedding(&chunked.id, &[0.0, 0.0, 1.0])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&chunked.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();

    let ranking = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    assert_eq!(ranking.len(), 2, "both recordings are searchable");
    // chunked's fresh chunk (cosine 1.0) beats legacy_only's 0.8.
    assert_eq!(ranking[0].1.as_str(), chunked.id.as_str());
    assert!((ranking[0].2 - 1.0).abs() < 1e-6);
    // And the chunked recording is scored from its chunk, not its stale
    // legacy vector (which was orthogonal → would have scored 0.0).
    let legacy_score = ranking
        .iter()
        .find(|(_key, id, _score)| id.as_str() == legacy_only.id.as_str())
        .unwrap()
        .2;
    assert!(
        (legacy_score - 0.8).abs() < 1e-6,
        "legacy-only recording scored from its whole-recording vector"
    );
}

#[tokio::test]
async fn embedding_cache_warms_and_returns_identical_results() {
    // (a) The cache must be transparent: a query warms the snapshot, and a
    // repeated query against the warm cache returns byte-identical rankings.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    let b = embedded_recording(None);
    let c = embedded_recording(None);
    for r in [&a, &b, &c] {
        db.insert(r).await.unwrap();
    }
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&b.id, &[vec![0.0, 1.0, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&c.id, &[vec![0.0, 0.0, 1.0]])
        .await
        .unwrap();

    // Writes leave the cache cold; the first query warms it.
    assert_eq!(db.cached_vector_count(), None, "cold after writes");
    let first = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    assert_eq!(
        db.cached_vector_count(),
        Some(3),
        "first query warms the snapshot with all three chunk vectors"
    );

    // A second query reads from the warm cache and must produce the same
    // per-recording cosine scores. The order between equal scores isn't
    // guaranteed by either path — both build from a HashMap and sort unstably —
    // so compare on id→score, the contract that actually matters.
    let second = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    let as_scores = |r: &[(String, RecordingId, f32)]| {
        r.iter()
            .map(|(_k, id, s)| (id.as_str().to_string(), *s))
            .collect::<std::collections::HashMap<_, _>>()
    };
    assert_eq!(
        as_scores(&first),
        as_scores(&second),
        "a cached query returns the same per-recording scores as the cold one"
    );
    assert_eq!(first[0].1.as_str(), a.id.as_str(), "best match still first");
    assert!(
        (first[0].2 - 1.0).abs() < 1e-6,
        "the on-axis recording scores ~1.0 from the warm cache"
    );

    // hybrid_search shares the same cache; its top hit must be stable.
    let h1 = db
        .hybrid_search("x", &[1.0, 0.0, 0.0], 10, -1.0, None)
        .await
        .unwrap();
    let h2 = db
        .hybrid_search("x", &[1.0, 0.0, 0.0], 10, -1.0, None)
        .await
        .unwrap();
    let scores1: std::collections::HashMap<_, _> = h1
        .iter()
        .map(|(id, s)| (id.as_str().to_string(), *s))
        .collect();
    let scores2: std::collections::HashMap<_, _> = h2
        .iter()
        .map(|(id, s)| (id.as_str().to_string(), *s))
        .collect();
    assert_eq!(
        scores1, scores2,
        "hybrid_search yields the same per-recording relevance over the warm cache"
    );
}

#[tokio::test]
async fn retention_hard_delete_invalidates_the_embedding_cache() {
    // A retention sweep that hard-deletes recordings cascade-drops their
    // embeddings; the warm cache has to be invalidated, or the deleted vectors
    // keep surfacing as ghost hits in search.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap(); // warm the cache
    assert!(db.cached_vector_count().is_some(), "warm after the query");

    // Hard delete every terminal recording (max_count = 0, delete_audio off).
    let cfg = crate::config::RetentionConfig {
        max_count: Some(0),
        ..Default::default()
    };
    db.apply_retention(&cfg).await.unwrap();
    assert_eq!(
        db.cached_vector_count(),
        None,
        "a retention hard-delete must invalidate the embedding cache"
    );
}

#[tokio::test]
async fn audio_only_retention_does_not_drop_the_cache() {
    // delete_audio mode only blanks audio_path (it keeps the row and
    // embeddings), so it must not needlessly drop a warm cache.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    assert!(db.cached_vector_count().is_some());
    let cfg = crate::config::RetentionConfig {
        max_count: Some(0),
        delete_audio: true,
        ..Default::default()
    };
    db.apply_retention(&cfg).await.unwrap();
    assert!(
        db.cached_vector_count().is_some(),
        "audio-only retention keeps embeddings, so the cache should stay warm"
    );
}

#[tokio::test]
async fn reembed_invalidates_cache_and_changes_ranking() {
    // (b) The correctness invariant: a re-embed must invalidate the cache so
    // the changed vector takes effect. A stale cached vector returning the old
    // ranking would be the worst possible bug.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    let b = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.insert(&b).await.unwrap();

    // Initially `a` is on the query axis and wins; `b` is orthogonal.
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&b.id, &[vec![0.0, 1.0, 0.0]])
        .await
        .unwrap();

    let before = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    assert_eq!(
        before[0].1.as_str(),
        a.id.as_str(),
        "a wins before re-embed"
    );
    assert!(db.cached_vector_count().is_some(), "warm after the query");

    // Re-embed `b` so it now nails the query and `a` becomes orthogonal —
    // this is the re-transcribe / ReembedAll write path.
    db.upsert_chunk_embeddings(&b.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&a.id, &[vec![0.0, 1.0, 0.0]])
        .await
        .unwrap();
    // Incremental patching keeps the cache warm but updates the two changed
    // recordings in place, rather than dropping the whole snapshot. Still two
    // vectors cached; the point is they're the new ones, proven next.
    assert_eq!(
        db.cached_vector_count(),
        Some(2),
        "the re-embed patched the snapshot in place, not dropped it"
    );

    let after = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    assert_eq!(
        after[0].1.as_str(),
        b.id.as_str(),
        "the changed vector flips the ranking — the patch is not stale"
    );

    // clear_all_embeddings (ReembedAll) must also invalidate.
    db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap(); // re-warm
    assert!(db.cached_vector_count().is_some());
    db.clear_all_embeddings().await.unwrap();
    assert_eq!(
        db.cached_vector_count(),
        None,
        "clear_all_embeddings invalidates the snapshot"
    );
    // And the legacy whole-recording path (semantic_search) invalidates too.
    db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
    db.semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
        .await
        .unwrap();
    assert!(
        db.cached_vector_count().is_some(),
        "semantic_search warms it"
    );
    db.upsert_embedding(&a.id, &[0.0, 1.0, 0.0]).await.unwrap();
    assert_eq!(
        db.cached_vector_count(),
        Some(1),
        "upsert_embedding patches the one recording in place (still warm)"
    );
    // A delete cascades the recording's embeddings away — the patch removes it
    // from the warm cache (here that empties it, but the snapshot stays warm).
    db.semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
        .await
        .unwrap();
    assert!(db.cached_vector_count().is_some());
    db.delete(&a.id).await.unwrap();
    assert_eq!(
        db.cached_vector_count(),
        Some(0),
        "delete patches the recording out (warm, now empty), not a full drop"
    );
}

#[tokio::test]
async fn rebuild_does_not_clobber_an_invalidation_that_raced_it() {
    // The lost-invalidation TOCTOU: `embedding_corpus` snapshots the
    // generation before its SQL reads and only caches when the generation is
    // unchanged at store time. If `invalidate_embedding_cache` (a racing
    // embedding write) lands between the snapshot and the store, the store has
    // to leave the slot cold so the writer's fresh data wins — otherwise a
    // pre-write snapshot would be cached and search would go stale.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();

    // Simulate the race exactly: take the gen snapshot a rebuild would take,
    // then let an invalidation land before the store.
    let gen_at_miss = db.embedding_cache_gen.load(Ordering::Acquire);
    let raced_corpus = Arc::new(EmbeddingCorpus {
        chunks: vec![Arc::new(CachedVector {
            id: a.id.as_str().to_string(),
            meeting_id: None,
            vector: Some(vec![1.0, 0.0, 0.0]),
        })],
        legacy: vec![],
    });
    db.invalidate_embedding_cache(); // the racing write bumps the generation
    db.store_corpus_if_current(raced_corpus, gen_at_miss);
    assert_eq!(
        db.cached_vector_count(),
        None,
        "a snapshot taken before a racing invalidation must NOT be cached"
    );

    // Control: with no racing invalidation, the same store does cache (so the
    // guard isn't just refusing to ever cache).
    let gen_now = db.embedding_cache_gen.load(Ordering::Acquire);
    let fresh_corpus = Arc::new(EmbeddingCorpus {
        chunks: vec![Arc::new(CachedVector {
            id: a.id.as_str().to_string(),
            meeting_id: None,
            vector: Some(vec![1.0, 0.0, 0.0]),
        })],
        legacy: vec![],
    });
    db.store_corpus_if_current(fresh_corpus, gen_now);
    assert_eq!(
        db.cached_vector_count(),
        Some(1),
        "an uncontested store caches the snapshot"
    );
}

#[tokio::test]
async fn warm_cache_hit_shares_the_same_corpus_arc() {
    // A warm hit returns the same Arc (O(1) clone), not a deep copy of every
    // vector. Two reads with no intervening write must hand back Arcs that point
    // at one allocation.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    let first = db.embedding_corpus().await.unwrap(); // miss → caches
    let second = db.embedding_corpus().await.unwrap(); // warm hit
    assert!(
        Arc::ptr_eq(&first, &second),
        "a warm hit must clone the Arc, not deep-copy the corpus"
    );
}

#[tokio::test]
async fn embedding_cache_is_bounded_by_the_vector_cap() {
    // (c) The cache must not grow without bound. The loader stores a corpus
    // only when `chunks + legacy <= MAX_CACHED_VECTORS`; an over-cap corpus
    // takes the else branch and stays uncached (queries fall back to SQLite),
    // so memory is bounded no matter how large the library grows. We exercise
    // that decision through `cap_allows_caching`, then confirm a real small
    // corpus is in fact cached and never exceeds the cap — without inserting
    // hundreds of thousands of rows.
    assert!(
        Catalog::cap_allows_caching(MAX_CACHED_VECTORS),
        "a corpus exactly at the cap is still cached"
    );
    assert!(
        !Catalog::cap_allows_caching(MAX_CACHED_VECTORS + 1),
        "a corpus one over the cap is NOT cached, so the cache is bounded"
    );

    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    db.insert(&a).await.unwrap();
    // A small corpus (1 vector) is comfortably under the cap and IS cached.
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
    let count = db.cached_vector_count().expect("a small corpus is cached");
    assert!(
        count <= MAX_CACHED_VECTORS,
        "a cached corpus never exceeds the cap"
    );
}

#[tokio::test]
async fn more_like_this_excludes_source_and_ranks_by_similarity() {
    // The recall flow: open a recording → find its semantic neighbours from
    // the vectors already in the catalog. The source itself must never be a
    // result, neighbours come back best-first with calibrated scores, and
    // near-orthogonal noise is dropped by the relevance floor.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let source = embedded_recording(None);
    let close = embedded_recording(None);
    let loose = embedded_recording(None);
    let unrelated = embedded_recording(None);
    for r in [&source, &close, &loose, &unrelated] {
        db.insert(r).await.unwrap();
    }

    // Source has two chunks; its (renormalized) mean is ~[0.707, 0.707, 0].
    db.upsert_chunk_embeddings(&source.id, &[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]])
        .await
        .unwrap();
    // close: cosine vs the centroid ≈ 0.707 (calibrates to 1.0, the ceiling).
    db.upsert_chunk_embeddings(&close.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    // loose: cosine ≈ 0.42 (calibrates to ~0.5 — clearly mid-strength).
    db.upsert_chunk_embeddings(&loose.id, &[vec![0.6, 0.0, 0.8]])
        .await
        .unwrap();
    // unrelated: orthogonal to the centroid → calibrated 0 → floored out.
    db.upsert_chunk_embeddings(&unrelated.id, &[vec![0.0, 0.0, 1.0]])
        .await
        .unwrap();

    let results = db.more_like_this(&source.id, 10, 0.12).await.unwrap();
    let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        !ids.contains(&source.id.as_str()),
        "the source recording must never be in its own results"
    );
    assert_eq!(
        ids,
        vec![close.id.as_str(), loose.id.as_str()],
        "neighbours best-first; orthogonal noise floored out"
    );
    assert!(
        results[0].1 > results[1].1,
        "scores must be calibrated and descending, got {results:?}"
    );

    // `limit` caps the result count at the top of the ranking.
    let top1 = db.more_like_this(&source.id, 1, 0.12).await.unwrap();
    assert_eq!(top1.len(), 1);
    assert_eq!(top1[0].0.as_str(), close.id.as_str());
}

#[tokio::test]
async fn more_like_this_excludes_the_sources_own_meeting_sibling() {
    // A meeting's two tracks have near-identical transcripts, so the
    // sibling track would always trivially rank #1 — useless as a
    // recommendation. Exclusion is by the meeting dedupe key, so the
    // sibling is dropped along with the source while other recordings
    // still surface.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mic = embedded_recording(Some("meeting-1"));
    let sys = embedded_recording(Some("meeting-1"));
    let other = embedded_recording(None);
    for r in [&mic, &sys, &other] {
        db.insert(r).await.unwrap();
    }
    db.upsert_chunk_embeddings(&mic.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&sys.id, &[vec![0.99, 0.01, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&other.id, &[vec![0.9, 0.1, 0.0]])
        .await
        .unwrap();

    let results = db.more_like_this(&mic.id, 10, 0.12).await.unwrap();
    let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        vec![other.id.as_str()],
        "the source's own meeting sibling must be excluded, got {ids:?}"
    );
}

#[tokio::test]
async fn more_like_this_falls_back_to_the_legacy_whole_recording_vector() {
    // A source from before per-chunk embedding (backfill pending) still
    // works: its legacy whole-recording vector is the query.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let legacy_source = embedded_recording(None);
    let neighbour = embedded_recording(None);
    db.insert(&legacy_source).await.unwrap();
    db.insert(&neighbour).await.unwrap();
    db.upsert_embedding(&legacy_source.id, &[1.0, 0.0, 0.0])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&neighbour.id, &[vec![0.95, 0.05, 0.0]])
        .await
        .unwrap();

    let results = db
        .more_like_this(&legacy_source.id, 10, 0.12)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.as_str(), neighbour.id.as_str());
}

#[tokio::test]
async fn more_like_this_errors_clearly_when_source_missing_or_not_indexed() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // Unknown id → NotFound (not the "not indexed" message).
    let ghost = RecordingId::new();
    let err = db.more_like_this(&ghost, 10, 0.12).await.unwrap_err();
    assert!(
        matches!(err, crate::error::Error::NotFound { .. }),
        "missing recording must be NotFound, got {err:?}"
    );

    // Existing but never embedded → a clear "not indexed yet" error the UI
    // can show verbatim.
    let bare = embedded_recording(None);
    db.insert(&bare).await.unwrap();
    let err = db.more_like_this(&bare.id, 10, 0.12).await.unwrap_err();
    assert!(
        matches!(&err, crate::error::Error::Internal(msg) if msg.contains("isn't indexed")),
        "unembedded recording must report it isn't indexed, got {err:?}"
    );
}

#[tokio::test]
async fn hybrid_search_recalls_a_paraphrase_where_keyword_match_misses() {
    // The headline requirement: "utter the likeness of something I spoke about
    // and get the proper search results."
    //
    // We simulate the embedding space directly (the ONNX model isn't bundled in
    // tests). The query and the target recording's transcript share no word, so
    // FTS5 (lexical) returns nothing for them — a naive keyword search misses
    // entirely. But their vectors are nearly identical (high cosine), modelling
    // a paraphrase. Hybrid search must still surface the right recording, ranked
    // first, with an honest relevance score.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // The recording the user is trying to recall. Its transcript talks about
    // moving the schema over; the query below ("database migration") shares none
    // of these words, so lexical search can't find it.
    let mut target = embedded_recording(None);
    target.transcript = Some("we should shift the records across to the new store".into());
    // A distractor whose words overlap the query's domain words a bit but
    // whose meaning (and vector) is unrelated.
    let mut distractor = embedded_recording(None);
    distractor.transcript = Some("lunch plans for friday afternoon".into());
    db.insert(&target).await.unwrap();
    db.insert(&distractor).await.unwrap();

    // Query vector ("the bit about the database migration"). The target's
    // matching chunk vector is nearly identical (paraphrase); the distractor
    // points elsewhere.
    let query_vec = [1.0_f32, 0.0, 0.0];
    db.upsert_chunk_embeddings(&target.id, &[vec![0.98, 0.20, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&distractor.id, &[vec![0.0, 0.0, 1.0]])
        .await
        .unwrap();

    // Sanity check: a pure keyword search for the query terms finds nothing —
    // the words appear in neither transcript. This is the gap vectors close.
    let lexical = db.lexical_ranking("database migration").await.unwrap();
    assert!(
        lexical.is_empty(),
        "precondition: naive keyword search must miss the paraphrase"
    );

    // Hybrid search, same min_relevance the daemon uses (0.12). Despite the
    // lexical miss, the semantic signal surfaces the target, ranked first.
    let results = db
        .hybrid_search("database migration", &query_vec, 10, 0.12, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "paraphrase must be recalled by meaning"
    );
    assert_eq!(
        results[0].0.as_str(),
        target.id.as_str(),
        "the paraphrased recording must rank first"
    );
    // The displayed relevance is the calibrated best-chunk cosine — a strong
    // paraphrase (cosine ~0.98) should read as a strong match, not single
    // digits.
    assert!(
        results[0].1 > 0.5,
        "a strong paraphrase should read as a strong relevance, got {}",
        results[0].1
    );
}

#[tokio::test]
async fn hybrid_search_keeps_exact_term_hit_despite_weak_cosine() {
    // The complement to paraphrase recall: when the user remembers one
    // distinctive word, an exact lexical hit must surface even if its vector
    // barely aligns with the query — never filtered out by the relevance floor.
    // This is the "union of strengths" guarantee.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mut named = embedded_recording(None);
    named.transcript = Some("the Kubernetes rollout notes are attached".into());
    db.insert(&named).await.unwrap();
    // Its vector is essentially orthogonal to the query (weak cosine), so a
    // semantic-only path with a 0.12 floor would drop it.
    db.upsert_chunk_embeddings(&named.id, &[vec![0.0, 1.0, 0.0]])
        .await
        .unwrap();

    // The user types the exact distinctive term; the query vector is the
    // unrelated x-axis.
    let results = db
        .hybrid_search("Kubernetes", &[1.0, 0.0, 0.0], 10, 0.12, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "the exact-term hit must survive");
    assert_eq!(results[0].0.as_str(), named.id.as_str());
    assert!(
        results[0].1 > 0.0,
        "a lexical-only hit gets an honest non-zero relevance floor, not 0%"
    );
}

#[tokio::test]
async fn hybrid_search_collapses_a_meeting_across_both_retrievers() {
    // Cross-retriever dedupe: a meeting's two tracks share a meeting_id. If the
    // vector retriever's best track differs from the lexical retriever's best
    // track, fusing on raw recording id would surface the same meeting twice.
    // Fusing on the meeting-stable dedupe key must collapse it to one row.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mic = embedded_recording(Some("meeting-x"));
    let mut sys = embedded_recording(Some("meeting-x"));
    // Put the distinctive lexical term on the system track only, and the strong
    // semantic vector on the mic track only, so each retriever prefers a
    // different track of the same meeting.
    sys.transcript = Some("the quarterly Kubernetes review".into());
    db.insert(&mic).await.unwrap();
    db.insert(&sys).await.unwrap();

    // Mic track: chunk vector strongly on the query axis (semantic winner).
    db.upsert_chunk_embeddings(&mic.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    // System track: vector points elsewhere, but it carries the exact term.
    db.upsert_chunk_embeddings(&sys.id, &[vec![0.0, 1.0, 0.0]])
        .await
        .unwrap();

    let results = db
        .hybrid_search("Kubernetes", &[1.0, 0.0, 0.0], 10, 0.12, None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        1,
        "the meeting's two tracks must collapse to a single result, got {results:?}"
    );
    // The surviving row is one of the meeting's tracks.
    assert!(results[0].0.as_str() == mic.id.as_str() || results[0].0.as_str() == sys.id.as_str());
}

#[tokio::test]
async fn test_insert_and_get() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = Recording {
        id: RecordingId::new(),
        started_at: Local::now(),
        duration_ms: 5000,
        audio_path: "foo.wav".into(),
        transcript: Some("hello world".into()),
        model: Some("tiny".into()),
        status: RecordingStatus::Done,
        error_kind: None,
        error_message: None,
        hook_command: Some("to-stdout.ps1".into()),
        hook_exit_code: Some(0),
        hook_duration_ms: Some(100),
        transcribed_at: Some(Local::now()),
        hook_ran_at: Some(Local::now()),
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    db.insert(&r).await.expect("insert");

    let fetched = db
        .get(&r.id)
        .await
        .expect("get recording")
        .expect("is some");
    assert_eq!(fetched.id.as_str(), r.id.as_str());
    assert_eq!(fetched.audio_path, r.audio_path);
    assert_eq!(fetched.transcript.as_deref(), Some("hello world"));
    assert_eq!(fetched.status, RecordingStatus::Done);

    // Test list
    let filter = ListFilter {
        limit: Some(10),
        ..Default::default()
    };
    let list = db.list(&filter).await.expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id.as_str(), r.id.as_str());
}

#[tokio::test]
async fn original_transcript_preserved_across_user_edit() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = Recording {
        id: RecordingId::new(),
        started_at: Local::now(),
        duration_ms: 1000,
        audio_path: "x.wav".into(),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    db.insert(&r).await.expect("insert");

    // Machine transcription stores transcript + original.
    db.update_transcript(&r.id, "machine output", "machine output", "ggml-base")
        .await
        .expect("machine transcript");
    assert_eq!(
        db.get_original_transcript(&r.id).await.unwrap().as_deref(),
        Some("machine output")
    );

    // A user edit changes the transcript but preserves the original.
    db.update_user_transcript(&r.id, "edited by the user")
        .await
        .expect("user edit");
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(got.transcript.as_deref(), Some("edited by the user"));
    // The transcription model is preserved — a hand edit shows up via the
    // user_edited flag / "Edited" column, not by overwriting the model field.
    assert_eq!(got.model.as_deref(), Some("ggml-base"));
    assert!(
        got.user_edited,
        "a manual edit must set the user_edited flag"
    );
    assert_eq!(
        db.get_original_transcript(&r.id).await.unwrap().as_deref(),
        Some("machine output")
    );
}

#[tokio::test]
async fn notes_round_trip_and_survive_transcription() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = Recording {
        id: RecordingId::new(),
        started_at: Local::now(),
        duration_ms: 1000,
        audio_path: "x.wav".into(),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    db.insert(&r).await.expect("insert");

    // Fresh insert: notes default to NULL.
    assert_eq!(db.get(&r.id).await.unwrap().unwrap().notes, None);

    // Notes round-trip through update_notes + get.
    db.update_notes(&r.id, "remember to follow up")
        .await
        .expect("update notes");
    assert_eq!(
        db.get(&r.id).await.unwrap().unwrap().notes.as_deref(),
        Some("remember to follow up")
    );

    // (Re-)transcription writes the transcript columns but must not touch notes.
    db.update_transcript(&r.id, "machine output", "machine output", "ggml-base")
        .await
        .expect("machine transcript");
    let after_transcribe = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(
        after_transcribe.transcript.as_deref(),
        Some("machine output")
    );
    assert_eq!(
        after_transcribe.notes.as_deref(),
        Some("remember to follow up"),
        "re-transcription must not clear notes"
    );

    // A manual transcript edit must also preserve notes.
    db.update_user_transcript(&r.id, "edited by the user")
        .await
        .expect("user edit");
    assert_eq!(
        db.get(&r.id).await.unwrap().unwrap().notes.as_deref(),
        Some("remember to follow up"),
        "user transcript edit must not clear notes"
    );
}

#[tokio::test]
async fn set_title_auto_writes_never_overwrite_a_user_title() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // Fresh rows are untitled and auto-owned (the migration default).
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(got.title, None);
    assert!(got.title_is_auto, "fresh rows must be auto-owned");

    // An auto write lands while the title is auto-owned, and a later auto write
    // (e.g. a retranscribe) refreshes both the title and its recorded model.
    assert!(db
        .set_title(&r.id, Some("first pass"), true, Some("gemma3"))
        .await
        .unwrap());
    assert!(db
        .set_title(&r.id, Some("second pass"), true, Some("gemma3:4b"))
        .await
        .unwrap());
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(got.title.as_deref(), Some("second pass"));
    assert!(got.title_is_auto);
    assert_eq!(
        got.title_model.as_deref(),
        Some("gemma3:4b"),
        "an auto refresh records the new title model"
    );

    // The user takes ownership; from now on auto writes are no-ops, and the
    // stale auto-title model is cleared (a user title never shows one).
    assert!(db
        .set_title(&r.id, Some("My title"), false, None)
        .await
        .unwrap());
    assert!(
        !db.set_title(&r.id, Some("auto again"), true, Some("x"))
            .await
            .unwrap(),
        "an auto write must be skipped once the user owns the title"
    );
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(got.title.as_deref(), Some("My title"));
    assert!(!got.title_is_auto, "title_is_auto = 0 wins forever");
    assert_eq!(got.title_model, None, "a user title carries no model");

    // Clearing (None) empties the title and reverts ownership to auto, so
    // the next pipeline run may fill it again.
    assert!(db.set_title(&r.id, None, true, None).await.unwrap());
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(got.title, None);
    assert!(got.title_is_auto, "a cleared title reverts to auto-owned");
    assert!(db
        .set_title(&r.id, Some("fresh auto"), true, Some("llama3"))
        .await
        .unwrap());
    assert_eq!(
        db.get(&r.id).await.unwrap().unwrap().title.as_deref(),
        Some("fresh auto")
    );

    // Unknown ids report no update.
    assert!(!db
        .set_title(&RecordingId::new(), Some("x"), false, None)
        .await
        .unwrap());
}

#[tokio::test]
async fn meeting_session_two_tracks_share_meeting_id_and_round_trip() {
    // Meeting Mode (v1.6): a meeting produces two recordings that share a
    // freshly-minted meeting_id and differ only by `track`. Both must round-trip
    // through insert/get/list, and a fresh single-track recording leaves both
    // columns NULL.
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");

    let meeting_id = "meeting-abc123".to_string();
    let make = |track: &str| Recording {
        id: RecordingId::new(),
        started_at: Local::now(),
        duration_ms: 1000,
        audio_path: format!("{track}.wav"),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: Some(meeting_id.clone()),
        meeting_name: None,
        track: Some(track.to_string()),
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    let mic = make("mic");
    let system = make("system");
    db.insert(&mic).await.expect("insert mic");
    db.insert(&system).await.expect("insert system");

    // Each row round-trips with its meeting_id + track intact.
    let got_mic = db.get(&mic.id).await.unwrap().unwrap();
    let got_sys = db.get(&system.id).await.unwrap().unwrap();
    assert_eq!(got_mic.meeting_id.as_deref(), Some("meeting-abc123"));
    assert_eq!(got_mic.track.as_deref(), Some("mic"));
    assert_eq!(got_sys.meeting_id.as_deref(), Some("meeting-abc123"));
    assert_eq!(got_sys.track.as_deref(), Some("system"));

    // The two recordings share one meeting_id.
    assert_eq!(got_mic.meeting_id, got_sys.meeting_id);

    // A normal single-track recording leaves both columns NULL.
    let solo = Recording {
        id: RecordingId::new(),
        started_at: Local::now(),
        duration_ms: 1000,
        audio_path: "solo.wav".into(),
        transcript: None,
        model: None,
        status: RecordingStatus::Recording,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    db.insert(&solo).await.expect("insert solo");
    let got_solo = db.get(&solo.id).await.unwrap().unwrap();
    assert_eq!(got_solo.meeting_id, None);
    assert_eq!(got_solo.track, None);

    // Both meeting rows are visible via list().
    let all = db.list(&ListFilter::default()).await.unwrap();
    let with_session: Vec<_> = all
        .iter()
        .filter(|r| r.meeting_id.as_deref() == Some("meeting-abc123"))
        .collect();
    assert_eq!(with_session.len(), 2, "both meeting tracks must be listed");
}

#[tokio::test]
async fn meeting_digest_set_get_upsert_and_delete() {
    // The whole-meeting digest (the meeting-scope twin of `summary`): set →
    // read back → regenerate (upsert overwrites) → delete. Keyed by meeting_id,
    // independent of the recordings table.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let meeting_id = "meeting-digest-1";

    // Nothing stored yet.
    assert!(db.meeting_digest(meeting_id).await.unwrap().is_none());

    // Store a digest with a model; it round-trips with both fields intact.
    db.update_meeting_digest(meeting_id, "Overview: shipped v2.", Some("llama3.2:3b"))
        .await
        .unwrap();
    let got = db.meeting_digest(meeting_id).await.unwrap().unwrap();
    assert_eq!(got.meeting_id, meeting_id);
    assert_eq!(got.digest, "Overview: shipped v2.");
    assert_eq!(got.digest_model.as_deref(), Some("llama3.2:3b"));

    // Regenerate: the upsert replaces the digest + model in place (one row per
    // meeting), not a second row.
    db.update_meeting_digest(meeting_id, "Revised digest.", None)
        .await
        .unwrap();
    let got = db.meeting_digest(meeting_id).await.unwrap().unwrap();
    assert_eq!(got.digest, "Revised digest.");
    assert_eq!(
        got.digest_model, None,
        "a None model clears the stored model"
    );

    // Delete removes it; a second delete is a harmless no-op.
    db.delete_meeting_digest(meeting_id).await.unwrap();
    assert!(db.meeting_digest(meeting_id).await.unwrap().is_none());
    db.delete_meeting_digest(meeting_id).await.unwrap();
}

#[tokio::test]
async fn period_digest_set_get_upsert_and_delete() {
    // The period digest (the date-window rollup): set → read back → regenerate
    // (upsert overwrites on the range key) → delete. Keyed by range, independent
    // of the recordings table.
    use chrono::TimeZone;
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let key = "2026-06-21T00:00:00+00:00|2026-06-21T23:59:59+00:00";
    let since = Local.with_ymd_and_hms(2026, 6, 21, 0, 0, 0).unwrap();
    let until = Local.with_ymd_and_hms(2026, 6, 21, 23, 59, 59).unwrap();

    // Nothing stored yet.
    assert!(db.period_digest(key).await.unwrap().is_none());

    // Store a digest; it round-trips with every field intact.
    db.update_period_digest(
        key,
        "2026-06-21",
        since,
        until,
        "Overview: shipped the digest feature.",
        Some("llama3.2:3b"),
        3,
    )
    .await
    .unwrap();
    let got = db.period_digest(key).await.unwrap().unwrap();
    assert_eq!(got.key, key);
    assert_eq!(got.label, "2026-06-21");
    assert_eq!(got.since, since);
    assert_eq!(got.until, until);
    assert_eq!(got.digest, "Overview: shipped the digest feature.");
    assert_eq!(got.digest_model.as_deref(), Some("llama3.2:3b"));
    assert_eq!(got.source_count, 3);

    // Regenerate the same range: the upsert replaces the row in place (keyed by
    // range), not a second row — and a re-run with a new label/count updates them.
    db.update_period_digest(key, "today", since, until, "Revised rollup.", None, 5)
        .await
        .unwrap();
    let got = db.period_digest(key).await.unwrap().unwrap();
    assert_eq!(got.digest, "Revised rollup.");
    assert_eq!(got.label, "today");
    assert_eq!(got.digest_model, None);
    assert_eq!(got.source_count, 5);

    // Exactly one row for the key (the upsert overwrote, didn't append).
    let all = db.list_all_period_digests().await.unwrap();
    assert_eq!(all.len(), 1);

    // Delete removes it; a second delete is a harmless no-op.
    db.delete_period_digest(key).await.unwrap();
    assert!(db.period_digest(key).await.unwrap().is_none());
    db.delete_period_digest(key).await.unwrap();
}

#[tokio::test]
async fn list_all_period_digests_orders_newest_range_first() {
    use chrono::TimeZone;
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let older_since = Local.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let older_until = Local.with_ymd_and_hms(2026, 6, 7, 23, 59, 59).unwrap();
    let newer_since = Local.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap();
    let newer_until = Local.with_ymd_and_hms(2026, 6, 21, 23, 59, 59).unwrap();

    db.update_period_digest(
        "older",
        "early June",
        older_since,
        older_until,
        "old",
        None,
        1,
    )
    .await
    .unwrap();
    db.update_period_digest(
        "newer",
        "mid June",
        newer_since,
        newer_until,
        "new",
        None,
        2,
    )
    .await
    .unwrap();

    let all = db.list_all_period_digests().await.unwrap();
    assert_eq!(all.len(), 2);
    // Newest range (later `since`) leads.
    assert_eq!(all[0].key, "newer");
    assert_eq!(all[1].key, "older");
}

// ── Named speakers ────────────────────────────────────────────────────────

#[tokio::test]
async fn speaker_names_set_get_rename_and_clear() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // No names initially.
    assert!(db.speaker_names_for(&r.id).await.unwrap().is_empty());

    // Set two distinct speaker names; they come back ordered by index.
    db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();
    db.set_speaker_name(&r.id, 2, "Alex").await.unwrap();
    let names = db.speaker_names_for(&r.id).await.unwrap();
    assert_eq!(
        names,
        vec![
            SpeakerName {
                speaker_label: 1,
                name: "Sarah".into()
            },
            SpeakerName {
                speaker_label: 2,
                name: "Alex".into()
            },
        ]
    );

    // Re-setting the same label updates in place (upsert, not a duplicate row).
    db.set_speaker_name(&r.id, 1, "Sarah Connor").await.unwrap();
    let names = db.speaker_names_for(&r.id).await.unwrap();
    assert_eq!(names.len(), 2, "rename must not add a row");
    assert_eq!(names[0].name, "Sarah Connor");

    // Names are trimmed on the way in.
    db.set_speaker_name(&r.id, 2, "  Alex P.  ").await.unwrap();
    assert_eq!(
        db.speaker_names_for(&r.id).await.unwrap()[1].name,
        "Alex P."
    );

    // A blank/whitespace name clears the mapping (reverts to "Speaker N").
    db.set_speaker_name(&r.id, 1, "   ").await.unwrap();
    let names = db.speaker_names_for(&r.id).await.unwrap();
    assert_eq!(
        names,
        vec![SpeakerName {
            speaker_label: 2,
            name: "Alex P.".into()
        }],
        "clearing speaker 1 leaves only speaker 2"
    );
}

#[tokio::test]
async fn set_speaker_name_if_absent_seeds_then_never_overwrites() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // Absent → the friendly default is seeded (trimmed like set_speaker_name).
    db.set_speaker_name_if_absent(&r.id, 1, "  You  ")
        .await
        .unwrap();
    assert_eq!(
        db.speaker_names_for(&r.id).await.unwrap(),
        vec![SpeakerName {
            speaker_label: 1,
            name: "You".into()
        }]
    );

    // A blank name never seeds an empty default (no-op).
    db.set_speaker_name_if_absent(&r.id, 2, "   ")
        .await
        .unwrap();
    assert_eq!(
        db.speaker_names_for(&r.id).await.unwrap().len(),
        1,
        "blank name must not seed a row"
    );

    // Present → a later if-absent write is a no-op, preserving a user rename.
    db.set_speaker_name(&r.id, 1, "Alice").await.unwrap();
    db.set_speaker_name_if_absent(&r.id, 1, "You")
        .await
        .unwrap();
    assert_eq!(
        db.speaker_names_for(&r.id).await.unwrap(),
        vec![SpeakerName {
            speaker_label: 1,
            name: "Alice".into()
        }],
        "if-absent must NOT clobber an existing user rename"
    );
}

#[tokio::test]
async fn speaker_names_are_populated_by_get_and_list() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();

    // get() carries the speaker-name map (backs the detail view).
    let got = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(
        got.speaker_names,
        vec![SpeakerName {
            speaker_label: 1,
            name: "Sarah".into()
        }]
    );

    // list() carries it too.
    let listed = db.list(&ListFilter::default()).await.unwrap();
    let row = listed.iter().find(|x| x.id == r.id).unwrap();
    assert_eq!(row.speaker_names.len(), 1);
    assert_eq!(row.speaker_names[0].name, "Sarah");
}

#[tokio::test]
async fn speaker_names_populated_per_track_by_list_by_meeting() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mic = embedded_recording(Some("m-1"));
    let sys = embedded_recording(Some("m-1"));
    db.insert(&mic).await.unwrap();
    db.insert(&sys).await.unwrap();
    // Each track keeps its own per-recording speaker names.
    db.set_speaker_name(&mic.id, 1, "Me").await.unwrap();
    db.set_speaker_name(&sys.id, 1, "Caller").await.unwrap();

    let tracks = db.list_by_meeting("m-1").await.unwrap();
    assert_eq!(tracks.len(), 2);
    for t in &tracks {
        let expected = if t.id == mic.id { "Me" } else { "Caller" };
        assert_eq!(
            t.speaker_names,
            vec![SpeakerName {
                speaker_label: 1,
                name: expected.into()
            }]
        );
    }
}

#[tokio::test]
async fn speaker_names_cascade_deleted_with_recording() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();

    db.delete(&r.id).await.unwrap();
    // The FK ON DELETE CASCADE must drop the orphaned name rows.
    assert!(
        db.speaker_names_for(&r.id).await.unwrap().is_empty(),
        "speaker names must be cascade-deleted with their recording"
    );
}

#[tokio::test]
async fn task_done_state_survives_a_daemon_restart() {
    // File-backed so dropping the pool and reopening the same DB genuinely
    // simulates a daemon restart (an in-memory DB would vanish on drop).
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("catalog.db");
    let r = embedded_recording(None);
    {
        let db = Catalog::open(&db_path).await.unwrap();
        db.insert(&r).await.unwrap();
        db.set_tasks(
            &r.id,
            &[Task {
                id: 0,
                text: "Email Bob".into(),
                due_hint: None,
                done: false,
            }],
        )
        .await
        .unwrap();
        let task_id = db.list_tasks(&r.id).await.unwrap()[0].id;
        let affected = db.set_task_done(&r.id, task_id, true).await.unwrap();
        assert_eq!(affected, 1, "the toggle must match exactly one row");
        // `db` drops here → pool closed, simulating shutdown.
    }
    // Reopen the same file: the completed flag must still be set.
    let db = Catalog::open(&db_path).await.unwrap();
    let tasks = db.list_tasks(&r.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(
        tasks[0].done,
        "a completed task must survive a daemon restart"
    );
}

#[tokio::test]
async fn re_extraction_preserves_done_across_a_minor_reword() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    db.set_tasks(
        &r.id,
        &[Task {
            id: 0,
            text: "Email Bob".into(),
            due_hint: None,
            done: false,
        }],
    )
    .await
    .unwrap();
    let id = db.list_tasks(&r.id).await.unwrap()[0].id;
    db.set_task_done(&r.id, id, true).await.unwrap();

    // Re-extraction rewords it (case + trailing punctuation + spacing); the tick carries.
    db.set_tasks(
        &r.id,
        &[Task {
            id: 0,
            text: "  email   bob.".into(),
            due_hint: None,
            done: false,
        }],
    )
    .await
    .unwrap();
    let after = db.list_tasks(&r.id).await.unwrap();
    assert_eq!(after.len(), 1);
    assert!(
        after[0].done,
        "a minor reword must keep the user's completed tick"
    );

    // A genuinely different task does NOT inherit the tick.
    db.set_tasks(
        &r.id,
        &[Task {
            id: 0,
            text: "Call Alice".into(),
            due_hint: None,
            done: false,
        }],
    )
    .await
    .unwrap();
    assert!(
        !db.list_tasks(&r.id).await.unwrap()[0].done,
        "an unrelated task must start unchecked"
    );
}

#[tokio::test]
async fn add_task_appends_a_manual_task_that_survives_reextraction() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    // An LLM extraction, then a hand-added task.
    db.set_tasks(
        &r.id,
        &[Task {
            id: 0,
            text: "Ship it".into(),
            due_hint: None,
            done: false,
        }],
    )
    .await
    .unwrap();
    let manual_id = db
        .add_task(&r.id, "Call the client", Some("today"))
        .await
        .unwrap();
    assert!(manual_id > 0);

    // Re-extraction returns a different LLM set; the manual task must remain while
    // the old extracted task is replaced.
    db.set_tasks(
        &r.id,
        &[Task {
            id: 0,
            text: "Ship it v2".into(),
            due_hint: None,
            done: false,
        }],
    )
    .await
    .unwrap();
    let texts: Vec<String> = db
        .list_tasks(&r.id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect();
    assert!(
        texts.iter().any(|t| t == "Call the client"),
        "the manual task must survive re-extraction"
    );
    assert!(
        texts.iter().any(|t| t == "Ship it v2"),
        "the new extracted task is present"
    );
    assert!(
        !texts.iter().any(|t| t == "Ship it"),
        "the old extracted task was replaced"
    );
}

#[tokio::test]
async fn update_and_delete_task_are_scoped_to_their_recording() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    let b = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.insert(&b).await.unwrap();
    let id = db.add_task(&a.id, "draft", None).await.unwrap();

    // Wrong recording → no row touched.
    assert_eq!(
        db.update_task(&b.id, id, "hijacked", None).await.unwrap(),
        0
    );
    assert_eq!(db.delete_task(&b.id, id).await.unwrap(), 0);

    // Right recording → edit then delete.
    assert_eq!(
        db.update_task(&a.id, id, "final draft", Some("Mon"))
            .await
            .unwrap(),
        1
    );
    let t = db.list_tasks(&a.id).await.unwrap().remove(0);
    assert_eq!(t.text, "final draft");
    assert_eq!(t.due_hint.as_deref(), Some("Mon"));
    assert_eq!(db.delete_task(&a.id, id).await.unwrap(), 1);
    assert!(db.list_tasks(&a.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn reorder_tasks_sets_the_user_order() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    db.set_tasks(
        &r.id,
        &[
            Task {
                id: 0,
                text: "one".into(),
                due_hint: None,
                done: false,
            },
            Task {
                id: 0,
                text: "two".into(),
                due_hint: None,
                done: false,
            },
            Task {
                id: 0,
                text: "three".into(),
                due_hint: None,
                done: false,
            },
        ],
    )
    .await
    .unwrap();
    let ids: Vec<i64> = db
        .list_tasks(&r.id)
        .await
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    // Reverse the order, then read it back.
    let reversed: Vec<i64> = ids.iter().rev().copied().collect();
    db.reorder_tasks(&r.id, &reversed).await.unwrap();
    let after: Vec<String> = db
        .list_tasks(&r.id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect();
    assert_eq!(after, vec!["three", "two", "one"]);
}

#[tokio::test]
async fn add_entity_survives_reextraction_and_delete_is_scoped() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    db.set_entities(
        &r.id,
        &[Entity {
            kind: "person".into(),
            value: "Ada".into(),
        }],
    )
    .await
    .unwrap();
    assert_eq!(db.add_entity(&r.id, "org", "ACME").await.unwrap(), 1);

    // Re-extraction replaces only the LLM rows; the manual entity stays.
    db.set_entities(
        &r.id,
        &[Entity {
            kind: "person".into(),
            value: "Grace".into(),
        }],
    )
    .await
    .unwrap();
    let vals: Vec<String> = db
        .list_entities(&r.id)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.value)
        .collect();
    assert!(
        vals.iter().any(|v| v == "ACME"),
        "manual entity survives re-extraction"
    );
    assert!(vals.iter().any(|v| v == "Grace"));
    assert!(!vals.iter().any(|v| v == "Ada"), "old LLM entity replaced");

    // Delete is keyed by (kind, value).
    assert_eq!(db.delete_entity(&r.id, "org", "ACME").await.unwrap(), 1);
    assert!(!db
        .list_entities(&r.id)
        .await
        .unwrap()
        .iter()
        .any(|e| e.value == "ACME"));
}

#[tokio::test]
async fn merge_entities_folds_variants_across_the_library() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    let b = embedded_recording(None);
    db.insert(&a).await.unwrap();
    db.insert(&b).await.unwrap();
    db.set_entities(
        &a.id,
        &[Entity {
            kind: "org".into(),
            value: "ACME".into(),
        }],
    )
    .await
    .unwrap();
    db.set_entities(
        &b.id,
        &[
            Entity {
                kind: "org".into(),
                value: "acme corp".into(),
            },
            Entity {
                kind: "org".into(),
                value: "Acme Corp".into(),
            },
        ],
    )
    .await
    .unwrap();

    // Fold both variants into the canonical "Acme Corp".
    db.merge_entities("org", &["ACME".into(), "acme corp".into()], "Acme Corp")
        .await
        .unwrap();

    let facet: Vec<_> = db
        .entity_facets()
        .await
        .unwrap()
        .into_iter()
        .filter(|f| f.kind == "org")
        .collect();
    assert_eq!(facet.len(), 1, "variants fold into one facet row");
    assert_eq!(facet[0].value, "Acme Corp");
    assert_eq!(facet[0].count, 2, "both recordings count once");
}

#[tokio::test]
async fn retention_audio_only_keeps_rows_and_is_idempotent() {
    // delete_audio = true: the WAV path is returned for deletion and blanked on
    // the row, but the row itself (transcript, metadata) survives, and a second
    // sweep finds nothing left to reclaim.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mut r = embedded_recording(None);
    r.started_at = Local::now() - chrono::Duration::days(90);
    db.insert(&r).await.unwrap();

    let cfg = crate::config::RetentionConfig {
        max_age_days: Some(30),
        max_count: None,
        delete_audio: true,
    };
    let paths = db.apply_retention(&cfg).await.unwrap();
    assert_eq!(paths, vec!["x.wav".to_string()]);

    let row = db.get(&r.id).await.unwrap().expect("row must survive");
    assert_eq!(row.audio_path, "", "audio path blanked after reclaim");
    assert_eq!(row.transcript.as_deref(), Some("t"), "transcript kept");

    let again = db.apply_retention(&cfg).await.unwrap();
    assert!(again.is_empty(), "second sweep must be a no-op");
}

#[tokio::test]
async fn retention_default_deletes_row_and_audio_together() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mut r = embedded_recording(None);
    r.started_at = Local::now() - chrono::Duration::days(90);
    db.insert(&r).await.unwrap();

    let cfg = crate::config::RetentionConfig {
        max_age_days: Some(30),
        max_count: None,
        delete_audio: false,
    };
    let paths = db.apply_retention(&cfg).await.unwrap();
    assert_eq!(paths.len(), 1);
    assert!(db.get(&r.id).await.unwrap().is_none(), "row deleted");
}

#[tokio::test]
async fn clear_all_tag_suggestions_sweeps_every_recording() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let a = embedded_recording(None);
    let b = embedded_recording(None);
    let c = embedded_recording(None); // never had suggestions
    db.insert(&a).await.unwrap();
    db.insert(&b).await.unwrap();
    db.insert(&c).await.unwrap();
    db.set_tag_suggestions(&a.id, &["alpha".into()])
        .await
        .unwrap();
    db.set_tag_suggestions(&b.id, &["beta".into(), "gamma".into()])
        .await
        .unwrap();

    let cleared = db.clear_all_tag_suggestions().await.unwrap();
    assert_eq!(cleared, 2, "only rows that HAD suggestions count");
    for id in [&a.id, &b.id, &c.id] {
        let rec = db.get(id).await.unwrap().unwrap();
        assert!(rec.tag_suggestions.is_empty());
    }
    // Sweep again: nothing left to clear.
    assert_eq!(db.clear_all_tag_suggestions().await.unwrap(), 0);
}

#[tokio::test]
async fn add_tag_is_case_insensitive() {
    // "Code" and "code" are the same tag: the second add must reuse the first
    // row (same id, casing, and color) instead of minting a byte-wise-unique
    // duplicate.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let first = db.add_tag("Code", Some("#f00")).await.unwrap();
    let second = db.add_tag("code", None).await.unwrap();
    assert_eq!(first.id, second.id, "casing variants must reuse the tag");
    assert_eq!(second.name, "Code", "the first-created casing wins");
    assert_eq!(second.color.as_deref(), Some("#f00"), "existing color kept");
}

#[tokio::test]
async fn segments_replace_round_trip_and_cascade() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // No segments yet is a normal (empty) state, not an error.
    assert!(db.segments_for(&r.id).await.unwrap().is_empty());

    let first = vec![
        TranscriptSegment {
            start_ms: 0,
            end_ms: 1200,
            text: "hello".into(),
            speaker: Some("1".into()),
        },
        TranscriptSegment {
            start_ms: 1200,
            end_ms: 2500,
            text: "hi there".into(),
            speaker: Some("2".into()),
        },
    ];
    db.replace_segments(&r.id, &first).await.unwrap();
    assert_eq!(db.segments_for(&r.id).await.unwrap(), first);

    // A retranscribe replaces the timeline — fewer rows must not leave stale
    // tail segments behind.
    let second = vec![TranscriptSegment {
        start_ms: 0,
        end_ms: 900,
        text: "rerun".into(),
        speaker: None,
    }];
    db.replace_segments(&r.id, &second).await.unwrap();
    assert_eq!(db.segments_for(&r.id).await.unwrap(), second);

    db.delete(&r.id).await.unwrap();
    assert!(
        db.segments_for(&r.id).await.unwrap().is_empty(),
        "segments must be cascade-deleted with their recording"
    );
}

#[tokio::test]
async fn words_replace_round_trip_and_cascade() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // No words yet is a normal (empty) state, not an error.
    assert!(db.words_for(&r.id).await.unwrap().is_empty());

    // A mix of present and absent confidence — the nullable column must
    // round-trip both (a whisper-family word has `None`, a Deepgram word
    // has a score). Ordering by idx is the timeline order.
    let first = vec![
        TranscriptWord {
            start_ms: 0,
            end_ms: 400,
            text: "hello".into(),
            leading_space: true,
            speaker: Some("1".into()),
            confidence: Some(0.97),
        },
        TranscriptWord {
            start_ms: 400,
            end_ms: 900,
            text: "there".into(),
            leading_space: true,
            speaker: Some("1".into()),
            confidence: None,
        },
    ];
    db.replace_words(&r.id, &first).await.unwrap();
    let got = db.words_for(&r.id).await.unwrap();
    assert_eq!(got, first, "words round-trip in idx order, confidence kept");
    assert_eq!(got[0].confidence, Some(0.97));
    assert_eq!(got[1].confidence, None, "a NULL confidence stays None");

    // A retranscribe replaces the word timeline — fewer rows must not leave
    // stale tail words behind.
    let second = vec![TranscriptWord {
        start_ms: 0,
        end_ms: 500,
        text: "rerun".into(),
        leading_space: false, // a continuation token — must round-trip as false
        speaker: None,
        confidence: Some(0.5),
    }];
    db.replace_words(&r.id, &second).await.unwrap();
    assert_eq!(db.words_for(&r.id).await.unwrap(), second);

    db.delete(&r.id).await.unwrap();
    assert!(
        db.words_for(&r.id).await.unwrap().is_empty(),
        "words must be cascade-deleted with their recording"
    );
}

// ── In-recording speaker correction (U1) ───────────────────────────────

/// Read the prose transcript text straight from the row.
async fn transcript_text(db: &Catalog, id: &RecordingId) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT transcript FROM recordings WHERE id = ?")
        .bind(id.as_str())
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

fn seg(start_ms: i64, end_ms: i64, text: &str, speaker: &str) -> TranscriptSegment {
    TranscriptSegment {
        start_ms,
        end_ms,
        text: text.into(),
        speaker: Some(speaker.into()),
    }
}

/// Seed a two-speaker diarized recording: segments, the matching prose
/// transcript, and one word per segment.
async fn seed_diarized(db: &Catalog) -> Recording {
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    let segs = vec![
        seg(0, 1000, "hello there", "1"),
        seg(1000, 2000, "hi yourself", "2"),
        seg(2000, 3000, "how are you", "1"),
    ];
    db.replace_segments(&r.id, &segs).await.unwrap();
    let words = vec![
        TranscriptWord {
            start_ms: 0,
            end_ms: 1000,
            text: "hello there".into(),
            leading_space: true,
            speaker: Some("1".into()),
            confidence: None,
        },
        TranscriptWord {
            start_ms: 1000,
            end_ms: 2000,
            text: "hi yourself".into(),
            leading_space: true,
            speaker: Some("2".into()),
            confidence: None,
        },
        TranscriptWord {
            start_ms: 2000,
            end_ms: 3000,
            text: "how are you".into(),
            leading_space: true,
            speaker: Some("1".into()),
            confidence: None,
        },
    ];
    db.replace_words(&r.id, &words).await.unwrap();
    db.update_transcript(
        &r.id,
        "[Speaker 1]: hello there\n\n[Speaker 2]: hi yourself\n\n[Speaker 1]: how are you",
        "orig",
        "tiny",
    )
    .await
    .unwrap();
    r
}

fn labels(segs: &[TranscriptSegment]) -> Vec<Option<&str>> {
    segs.iter().map(|s| s.speaker.as_deref()).collect()
}

#[tokio::test]
async fn reassign_segment_moves_to_existing_and_brand_new_label() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;

    // Move the middle segment (idx 1) from speaker 2 → speaker 1.
    db.reassign_segment(&r.id, 1, 1).await.unwrap();
    let segs = db.segments_for(&r.id).await.unwrap();
    assert_eq!(labels(&segs), vec![Some("1"), Some("1"), Some("1")]);
    // All three turns now coalesce under one speaker in the prose markers.
    assert_eq!(
        transcript_text(&db, &r.id).await.as_deref(),
        Some("[Speaker 1]: hello there hi yourself how are you"),
        "consecutive same-label segments coalesce into one turn"
    );
    // The word layer for that segment's span followed.
    let words = db.words_for(&r.id).await.unwrap();
    assert_eq!(words[1].speaker.as_deref(), Some("1"));

    // Reassign the same segment to a brand-new label 3 — it simply starts.
    db.reassign_segment(&r.id, 1, 3).await.unwrap();
    let segs = db.segments_for(&r.id).await.unwrap();
    assert_eq!(labels(&segs), vec![Some("1"), Some("3"), Some("1")]);
    assert_eq!(
        transcript_text(&db, &r.id).await.as_deref(),
        Some("[Speaker 1]: hello there\n\n[Speaker 3]: hi yourself\n\n[Speaker 1]: how are you"),
    );
}

#[tokio::test]
async fn reassign_segment_unknown_idx_errors_with_no_mutation() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;
    let before = db.segments_for(&r.id).await.unwrap();
    let before_text = transcript_text(&db, &r.id).await;

    let err = db.reassign_segment(&r.id, 99, 2).await.unwrap_err();
    assert!(matches!(err, crate::error::Error::NotFound { .. }));
    assert_eq!(
        db.segments_for(&r.id).await.unwrap(),
        before,
        "no segment write"
    );
    assert_eq!(
        transcript_text(&db, &r.id).await,
        before_text,
        "no text write"
    );

    // A label < 1 is rejected before any write too.
    assert!(db.reassign_segment(&r.id, 0, 0).await.is_err());
    assert_eq!(db.segments_for(&r.id).await.unwrap(), before);
}

#[tokio::test]
async fn merge_speakers_repoints_segments_and_keeps_into_name() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;
    db.set_speaker_name(&r.id, 1, "Ada").await.unwrap();
    db.set_speaker_name(&r.id, 2, "Bob").await.unwrap();

    // Merge speaker 2 into speaker 1: every 2-segment becomes 1.
    db.merge_speakers(&r.id, 2, 1).await.unwrap();
    let segs = db.segments_for(&r.id).await.unwrap();
    assert_eq!(labels(&segs), vec![Some("1"), Some("1"), Some("1")]);
    // `into` (1) keeps its name; `from` (2) name row is gone.
    let names = db.speaker_names_for(&r.id).await.unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].speaker_label, 1);
    assert_eq!(names[0].name, "Ada", "into keeps its own name");
    // Prose markers reflect the merge.
    assert_eq!(
        transcript_text(&db, &r.id).await.as_deref(),
        Some("[Speaker 1]: hello there hi yourself how are you"),
    );
    // Words followed too.
    let words = db.words_for(&r.id).await.unwrap();
    assert!(words.iter().all(|w| w.speaker.as_deref() == Some("1")));
}

#[tokio::test]
async fn merge_speakers_into_adopts_froms_name_when_into_unnamed() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;
    // Only `from` (2) is named; `into` (1) is not.
    db.set_speaker_name(&r.id, 2, "Bob").await.unwrap();

    db.merge_speakers(&r.id, 2, 1).await.unwrap();
    let names = db.speaker_names_for(&r.id).await.unwrap();
    assert_eq!(
        names,
        vec![SpeakerName {
            speaker_label: 1,
            name: "Bob".into()
        }],
        "an unnamed into adopts from's name; the from row is removed"
    );
}

#[tokio::test]
async fn merge_speakers_drops_froms_voiceprint_and_recomputes_library() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;
    // Both labels captured and enrolled under different named voices.
    db.save_speaker_voiceprint(r.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(r.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(r.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    let bob = db
        .enroll_speaker(r.id.as_str(), 2, "Bob")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(named_voice_samples(&db, &bob).await, 1);

    // Merge 2 → 1: from's (2) capture is dropped; Bob's library entry
    // recomputes to zero linked samples.
    db.merge_speakers(&r.id, 2, 1).await.unwrap();
    assert!(
        db.speaker_voiceprint(r.id.as_str(), 2)
            .await
            .unwrap()
            .is_none(),
        "from's capture row is deleted"
    );
    assert!(
        db.speaker_voiceprint(r.id.as_str(), 1)
            .await
            .unwrap()
            .is_some(),
        "into's capture is untouched"
    );
    assert_eq!(
        named_voice_samples(&db, &bob).await,
        0,
        "Bob no longer counts the dropped capture"
    );
    assert_eq!(named_voice_samples(&db, &ada).await, 1, "Ada untouched");
}

#[tokio::test]
async fn merge_speakers_unknown_from_errors_with_no_mutation() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;
    let before = db.segments_for(&r.id).await.unwrap();

    // No segment carries label 5.
    let err = db.merge_speakers(&r.id, 5, 1).await.unwrap_err();
    assert!(matches!(err, crate::error::Error::NotFound { .. }));
    assert_eq!(db.segments_for(&r.id).await.unwrap(), before);

    // Self-merge and bad labels are rejected.
    assert!(db.merge_speakers(&r.id, 1, 1).await.is_err());
    assert!(db.merge_speakers(&r.id, 0, 1).await.is_err());
    assert_eq!(db.segments_for(&r.id).await.unwrap(), before);
}

#[tokio::test]
async fn split_speaker_moves_listed_segments_only() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;
    // Speaker 1 owns segments 0 and 2. Split segment 2 off onto a fresh 3.
    db.split_speaker(&r.id, 1, &[2], 3).await.unwrap();
    let segs = db.segments_for(&r.id).await.unwrap();
    assert_eq!(labels(&segs), vec![Some("1"), Some("2"), Some("3")]);
    assert_eq!(
        transcript_text(&db, &r.id).await.as_deref(),
        Some("[Speaker 1]: hello there\n\n[Speaker 2]: hi yourself\n\n[Speaker 3]: how are you"),
    );
    // The new label has no name and no voiceprint until enrolled.
    assert!(db
        .speaker_names_for(&r.id)
        .await
        .unwrap()
        .iter()
        .all(|n| n.speaker_label != 3));
    assert!(db
        .speaker_voiceprint(r.id.as_str(), 3)
        .await
        .unwrap()
        .is_none());
    // The word in segment 2's span followed.
    let words = db.words_for(&r.id).await.unwrap();
    assert_eq!(words[2].speaker.as_deref(), Some("3"));
}

#[tokio::test]
async fn split_speaker_rejects_idx_not_owned_by_label_with_no_mutation() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = seed_diarized(&db).await;
    let before = db.segments_for(&r.id).await.unwrap();

    // Segment 1 belongs to speaker 2, not 1 — the whole op must abort.
    let err = db.split_speaker(&r.id, 1, &[0, 1], 3).await.unwrap_err();
    assert!(matches!(err, crate::error::Error::Internal(_)));
    assert_eq!(
        db.segments_for(&r.id).await.unwrap(),
        before,
        "one bad idx rolls back the whole split (segment 0 not moved)"
    );

    // Unknown idx, empty list, self-target, bad labels all error cleanly.
    assert!(matches!(
        db.split_speaker(&r.id, 1, &[99], 3).await.unwrap_err(),
        crate::error::Error::NotFound { .. }
    ));
    assert!(db.split_speaker(&r.id, 1, &[], 3).await.is_err());
    assert!(db.split_speaker(&r.id, 1, &[0], 1).await.is_err());
    assert_eq!(db.segments_for(&r.id).await.unwrap(), before);
}

#[tokio::test]
async fn speaker_edit_leaves_plain_transcript_markers_untouched() {
    // A non-diarized transcript has no `[Speaker N]` markers to keep
    // consistent — the rebuild must leave the prose text alone (segments
    // still update, driving the timeline views).
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();
    db.replace_segments(&r.id, &[seg(0, 1000, "just one voice", "1")])
        .await
        .unwrap();
    db.update_transcript(&r.id, "just one voice", "orig", "tiny")
        .await
        .unwrap();

    db.reassign_segment(&r.id, 0, 2).await.unwrap();
    assert_eq!(
        transcript_text(&db, &r.id).await.as_deref(),
        Some("just one voice"),
        "plain prose is never marker-rewritten"
    );
    assert_eq!(
        db.segments_for(&r.id).await.unwrap()[0].speaker.as_deref(),
        Some("2"),
        "the segment label still updates"
    );
}

/// One recording's sample count for a named voice, read straight from the
/// library row (the cached count `recompute_named_centroid` maintains).
async fn named_voice_samples(db: &Catalog, id: &str) -> i64 {
    sqlx::query_scalar("SELECT samples FROM named_voiceprints WHERE id = ?")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn recognize_speakers_one_to_one_never_doubles_a_name() {
    // Audit H2: two captured speakers in one recording, both nearest the same
    // library voice, must not both be suggested that name — at most one gets it,
    // the other is left unknown.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // Enroll one library voice "Ada" from a source recording.
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();

    // A second recording with two captured speakers, both closest to Ada
    // (one a near-perfect match, the other a clear runner-up).
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.95, 0.31, 0.0], 0)
        .await
        .unwrap();

    let sugg = db
        .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    let to_ada: Vec<_> = sugg.iter().filter(|s| s.named_voice_id == ada).collect();
    assert_eq!(
        to_ada.len(),
        1,
        "a name can be suggested to at most one speaker per recording"
    );
    assert_eq!(
        to_ada[0].speaker_label, 1,
        "the closest speaker wins the name"
    );
    assert!(
        sugg.len() <= 1,
        "only one library voice exists, so at most one suggestion"
    );
}

#[tokio::test]
async fn recognize_speakers_assigns_distinct_voices_one_each() {
    // Two library voices, two speakers each clearly nearest a different one:
    // both get their own name, no crossover.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    let bob = db
        .enroll_speaker(src.id.as_str(), 2, "Bob")
        .await
        .unwrap()
        .unwrap();

    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
        .await
        .unwrap();

    let sugg = db
        .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert_eq!(sugg.len(), 2, "both speakers recognized");
    assert_eq!(sugg[0].speaker_label, 1, "sorted by speaker_label");
    assert_eq!(sugg[0].named_voice_id, ada);
    assert_eq!(sugg[1].named_voice_id, bob);
}

#[tokio::test]
async fn recognize_speakers_skips_ambiguous_speaker() {
    // A speaker nearly equidistant from two library voices (within MARGIN)
    // is too ambiguous to name — no suggestion is emitted for it.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 2, &[0.0, 1.0], 0)
        .await
        .unwrap();
    db.enroll_speaker(src.id.as_str(), 1, "Ada").await.unwrap();
    db.enroll_speaker(src.id.as_str(), 2, "Bob").await.unwrap();

    // Probe at 45°: cosine ~0.707 to both voices — tie inside MARGIN.
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 1.0], 0)
        .await
        .unwrap();
    let sugg = db
        .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert!(sugg.is_empty(), "an ambiguous speaker is left unknown");
}

#[tokio::test]
async fn recognize_speakers_off_mode_unchanged_from_raw_assign() {
    // V2 default-off contract at the catalog layer: routing through the new
    // mode argument with ScoreNorm::Off produces the same assignment as the
    // raw assign_speakers it delegates to.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    let bob = db
        .enroll_speaker(src.id.as_str(), 2, "Bob")
        .await
        .unwrap()
        .unwrap();

    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[0.98, 0.02, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.02, 0.98, 0.0], 0)
        .await
        .unwrap();

    let off = db
        .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert_eq!(off.len(), 2);
    assert_eq!(off[0].speaker_label, 1);
    assert_eq!(off[0].named_voice_id, ada);
    assert_eq!(off[1].named_voice_id, bob);
    // Off scores are still raw cosines (≈1), not z-scores.
    assert!(off[0].score > 0.9 && off[1].score > 0.9, "{off:?}");
}

#[tokio::test]
async fn recognize_speakers_snorm_routes_and_assigns() {
    // V2 on-path smoke test: with S-norm the scores are z-scores (can exceed
    // 1 / be negative), so the threshold is a z-bar. The two clearly-distinct
    // speakers still each get their own name. Cohort = the 3 library voices.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    // Three enrolled voices spread out so each probe has a real cohort.
    for (label, v) in [
        (1i64, [1.0f32, 0.0, 0.0]),
        (2, [0.0, 1.0, 0.0]),
        (3, [0.0, 0.0, 1.0]),
    ] {
        db.save_speaker_voiceprint(src.id.as_str(), label, &v, 0)
            .await
            .unwrap();
    }
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    let bob = db
        .enroll_speaker(src.id.as_str(), 2, "Bob")
        .await
        .unwrap()
        .unwrap();
    db.enroll_speaker(src.id.as_str(), 3, "Cleo").await.unwrap();

    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[0.99, 0.01, 0.0], 0) // ~Ada
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.01, 0.99, 0.0], 0) // ~Bob
        .await
        .unwrap();

    // z-bar ~1.0: a genuine match stands well above its cohort mean.
    let sugg = db
        .recognize_speakers_for(rec.id.as_str(), 1.0, crate::voiceprint::ScoreNorm::SNorm)
        .await
        .unwrap();
    assert_eq!(sugg.len(), 2, "both distinct speakers recognized: {sugg:?}");
    assert_eq!(sugg[0].speaker_label, 1);
    assert_eq!(sugg[0].named_voice_id, ada);
    assert_eq!(sugg[1].named_voice_id, bob);
}

#[tokio::test]
async fn enroll_speaker_clears_a_prior_dismissal() {
    // Audit M9: a speaker dismissed before the right voice existed must be
    // recognizable once it's named — naming clears the dismissal row.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.dismiss_speaker_suggestion(rec.id.as_str(), 1)
        .await
        .unwrap();

    // Naming the speaker is an explicit ID — the dismissal must be gone.
    db.enroll_speaker(rec.id.as_str(), 1, "Ada").await.unwrap();
    let still_dismissed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dismissed_speaker_suggestions \
             WHERE recording_id = ? AND speaker_label = ?",
    )
    .bind(rec.id.as_str())
    .bind(1i64)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(still_dismissed, 0, "enrolling clears the dismissal");
}

#[tokio::test]
async fn recompute_named_centroid_drops_an_outlier_capture() {
    // Audit M7: with >= 4 captures, a clear wrong-speaker outlier is pruned
    // before the mean, so the cached sample count drops and the centroid
    // stays close to the genuine cluster.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    // Three tightly-clustered captures plus one clear (opposite-direction)
    // outlier — a wrong-speaker capture mistakenly named into this voice.
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.99, 0.10, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 3, &[0.98, 0.0, 0.10], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 4, &[-1.0, 0.0, 0.0], 0) // outlier
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(rec.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    for label in [2i64, 3, 4] {
        db.enroll_speaker(rec.id.as_str(), label, "Ada")
            .await
            .unwrap();
    }

    // Four captures linked, but the outlier is pruned → 3 effective samples.
    assert_eq!(
        named_voice_samples(&db, &ada).await,
        3,
        "the orthogonal outlier is dropped from the template"
    );
    // The surviving centroid still points at the cluster (cosine ~1 to it).
    let probe = vec![1.0f32, 0.0, 0.0];
    let (_, score) = db.recognize_voice(&probe, 0.5).await.unwrap().unwrap();
    assert!(
        score > 0.95,
        "centroid stays on the genuine cluster: {score}"
    );
}

#[tokio::test]
async fn recompute_named_centroid_keeps_all_below_four_samples() {
    // Below 4 captures every sample counts — even a far one is kept (too few
    // to tell signal from noise).
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.0, 1.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(rec.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    db.enroll_speaker(rec.id.as_str(), 2, "Ada").await.unwrap();
    assert_eq!(
        named_voice_samples(&db, &ada).await,
        2,
        "under the threshold no pruning happens"
    );
}

/// The named voice's cached centroid, read straight from the library row.
async fn named_voice_centroid(db: &Catalog, id: &str) -> Vec<f32> {
    let json: String = sqlx::query_scalar("SELECT centroid FROM named_voiceprints WHERE id = ?")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    serde_json::from_str(&json).unwrap()
}

#[tokio::test]
async fn recompute_named_centroid_legacy_zero_durations_match_plain_mean() {
    // Backward-compat contract: a library whose captures all have
    // duration_ms=0 (built before V4) must recompute to the same centroid the
    // unweighted mean produces — that is, mean_centroid of those vectors.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    let vecs = [
        vec![1.0f32, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
        vec![0.7, 0.7, 0.0],
    ];
    for (i, v) in vecs.iter().enumerate() {
        db.save_speaker_voiceprint(rec.id.as_str(), (i + 1) as i64, v, 0)
            .await
            .unwrap();
    }
    let ada = db
        .enroll_speaker(rec.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    db.enroll_speaker(rec.id.as_str(), 2, "Ada").await.unwrap();
    db.enroll_speaker(rec.id.as_str(), 3, "Ada").await.unwrap();

    // Below the 4-sample pruning threshold, so all three survive → the cached
    // centroid is exactly the unweighted mean of the three vectors.
    let expected = crate::voiceprint::mean_centroid(&vecs).unwrap();
    let got = named_voice_centroid(&db, &ada).await;
    assert_eq!(got.len(), expected.len());
    for (g, e) in got.iter().zip(expected.iter()) {
        assert!(
            (g - e).abs() < 1e-6,
            "legacy recompute drifted: {got:?} vs {expected:?}"
        );
    }
}

#[tokio::test]
async fn recompute_named_centroid_weights_toward_the_longer_capture() {
    // Two orthogonal captures of very different speaking duration. The plain
    // (unweighted) mean would sit at 45° — equidistant. Duration-weighting
    // must pull the cached centroid toward the much longer capture, so it
    // scores clearly higher to the long sample's direction than the short.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    let long_dir = [1.0f32, 0.0]; // spoke for minutes
    let short_dir = [0.0f32, 1.0]; // spoke for a moment
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &long_dir, 600_000)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &short_dir, 2_000)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(rec.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    db.enroll_speaker(rec.id.as_str(), 2, "Ada").await.unwrap();

    let centroid = named_voice_centroid(&db, &ada).await;
    let to_long = crate::voiceprint::cosine_similarity(&centroid, &long_dir);
    let to_short = crate::voiceprint::cosine_similarity(&centroid, &short_dir);
    assert!(
        to_long > to_short,
        "centroid must lean toward the longer capture: long {to_long} vs short {to_short}"
    );
    // And strictly past the unweighted-mean midpoint (cos 0.707 to each).
    let midpoint = std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        to_long > midpoint + 1e-3,
        "weighting must move the centroid past the equal-weight midpoint: {to_long}"
    );
}

#[tokio::test]
async fn recognize_voice_skips_dimension_mismatched_library() {
    // Audit L: a library centroid of a different dimension than the probe is
    // skipped (it came from another embedding model) — not silently scored 0.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    // Library voice has a 3-dim centroid.
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.enroll_speaker(rec.id.as_str(), 1, "Ada").await.unwrap();

    // Probe a 2-dim vector: dimension mismatch → skipped → no match.
    let got = db.recognize_voice(&[1.0, 0.0], 0.5).await.unwrap();
    assert!(
        got.is_none(),
        "a cross-model library entry is skipped, not matched at 0.0"
    );
}

#[tokio::test]
async fn clear_all_recordings_recomputes_emptied_named_voices() {
    // Audit M1: wiping every recording cascades the voiceprints away, so the
    // named voice must drop to zero samples (not keep a stale count).
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(rec.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(named_voice_samples(&db, &ada).await, 1);

    db.clear_all_recordings().await.unwrap();
    assert_eq!(
        named_voice_samples(&db, &ada).await,
        0,
        "the library entry's count must follow its lost captures"
    );
}

#[tokio::test]
async fn retention_hard_delete_recomputes_named_voices() {
    // Audit M1: a hard-delete retention sweep cascades the deleted recording's
    // voiceprints away, so its named voice's cached count must be recomputed.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    // Old recording (eligible) enrolled into "Ada".
    let old = {
        let mut r = embedded_recording(None);
        r.started_at = Local::now() - chrono::Duration::days(90);
        r
    };
    db.insert(&old).await.unwrap();
    db.save_speaker_voiceprint(old.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(old.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(named_voice_samples(&db, &ada).await, 1);

    let cfg = crate::config::RetentionConfig {
        max_age_days: Some(30),
        max_count: None,
        delete_audio: false,
    };
    db.apply_retention(&cfg).await.unwrap();
    assert!(db.get(&old.id).await.unwrap().is_none(), "old row swept");
    assert_eq!(
        named_voice_samples(&db, &ada).await,
        0,
        "the swept recording's named voice must lose its sample"
    );
}

// ---- Name propagation (V5) ----------------------------------------------

/// The custom display name for a (recording, label), or None.
async fn display_name(db: &Catalog, rid: &RecordingId, label: i64) -> Option<String> {
    db.speaker_names_for(rid)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.speaker_label == label)
        .map(|s| s.name)
}

/// Enroll "Ada" in a source recording, and create two more recordings that
/// each have one unnamed speaker matching Ada and (in `rec_named`) one
/// already-named "Bob". Returns `(db, ada_id, rec_match, rec_named, far)`.
async fn propagation_fixture() -> (Catalog, String, RecordingId, RecordingId, RecordingId) {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // Source: enroll Ada at [1,0,0].
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();

    // rec_match: one unnamed speaker that matches Ada.
    let rec_match = embedded_recording(None);
    db.insert(&rec_match).await.unwrap();
    db.save_speaker_voiceprint(rec_match.id.as_str(), 1, &[0.99, 0.01, 0.0], 0)
        .await
        .unwrap();

    // rec_named: an unnamed Ada-match at label 1 plus an already-named "Bob"
    // (also near Ada geometrically — its name must not be overwritten).
    let rec_named = embedded_recording(None);
    db.insert(&rec_named).await.unwrap();
    db.save_speaker_voiceprint(rec_named.id.as_str(), 1, &[0.98, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.save_speaker_voiceprint(rec_named.id.as_str(), 2, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    db.set_speaker_name(&rec_named.id, 2, "Bob").await.unwrap();

    // far: a dissimilar voice that must never be a candidate.
    let far = embedded_recording(None);
    db.insert(&far).await.unwrap();
    db.save_speaker_voiceprint(far.id.as_str(), 1, &[0.0, 0.0, 1.0], 0)
        .await
        .unwrap();

    (db, ada, rec_match.id, rec_named.id, far.id)
}

#[tokio::test]
async fn propagation_candidates_respect_threshold_and_unnamed_only() {
    let (db, ada, rec_match, rec_named, far) = propagation_fixture().await;
    let cands = db
        .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();

    // The two unnamed Ada-matches are candidates; the dissimilar "far" voice
    // is below threshold, and the already-named "Bob" (label 2 in rec_named)
    // is excluded because only `named_voice_id IS NULL` rows are scanned.
    let pairs: std::collections::HashSet<(String, i64)> = cands
        .iter()
        .map(|c| (c.recording_id.as_str().to_string(), c.speaker_label))
        .collect();
    assert!(pairs.contains(&(rec_match.as_str().to_string(), 1)));
    assert!(pairs.contains(&(rec_named.as_str().to_string(), 1)));
    assert!(
        !pairs.contains(&(rec_named.as_str().to_string(), 2)),
        "an already-named speaker is never a candidate"
    );
    assert!(
        !pairs.iter().any(|(rid, _)| rid == far.as_str()),
        "a dissimilar voice (below threshold) is not a candidate"
    );
    // The source recording (already enrolled under Ada) is excluded.
    assert_eq!(cands.len(), 2);
    // Ordered by score, highest first.
    assert!(cands[0].score >= cands[1].score);
}

#[tokio::test]
async fn propagation_admits_a_second_unnamed_speaker_in_an_enrolled_recording() {
    // The over-exclude guard: a single recording can hold speaker 1 enrolled as
    // Ada plus a second, still-unnamed speaker of the same voice. A
    // recording-wide exclusion would drop the whole recording (so speaker 2 is
    // never offered); per-speaker `IS NULL` scoping must still surface speaker
    // 2.
    let (db, ada, _rec_match, _rec_named, _far) = propagation_fixture().await;

    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    // Speaker 1: enrolled into the same Ada voice (find_or_create dedups by
    // name), so this recording has a speaker already under `ada`.
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let same = db
        .enroll_speaker(rec.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(same, ada, "enrolling 'Ada' again must reuse the voice");
    // Speaker 2: an unnamed Ada-match that should still be a candidate.
    db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.99, 0.01, 0.0], 0)
        .await
        .unwrap();

    let cands = db
        .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    let pairs: std::collections::HashSet<(String, i64)> = cands
        .iter()
        .map(|c| (c.recording_id.as_str().to_string(), c.speaker_label))
        .collect();
    assert!(
        pairs.contains(&(rec.id.as_str().to_string(), 2)),
        "the second, still-unnamed same-voice speaker must be a candidate"
    );
    assert!(
        !pairs.contains(&(rec.id.as_str().to_string(), 1)),
        "the already-enrolled speaker 1 is excluded by IS NULL"
    );
}

#[tokio::test]
async fn propagation_off_policy_backfills_nothing() {
    // The OFF policy is purely a routing decision in the IPC layer, but the
    // core invariant it relies on: candidates exist, yet nothing is applied
    // unless apply_propagation is called. Prove naming alone never touches
    // another recording (the Ask default proof below covers the live path).
    let (db, ada, rec_match, _rec_named, _far) = propagation_fixture().await;
    // Simulate OFF: gather candidates (or not) but never apply.
    let _ = db
        .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert!(
        display_name(&db, &rec_match, 1).await.is_none(),
        "OFF: no other recording is named"
    );
}

#[tokio::test]
async fn propagation_ask_returns_candidates_but_applies_nothing() {
    // The default-policy guarantee: under Ask, gathering candidates must not
    // modify any other recording.
    let (db, ada, rec_match, rec_named, _far) = propagation_fixture().await;

    // Snapshot every other recording's name state before.
    let before_match = display_name(&db, &rec_match, 1).await;
    let before_named1 = display_name(&db, &rec_named, 1).await;
    let before_named2 = display_name(&db, &rec_named, 2).await;

    let cands = db
        .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert_eq!(cands.len(), 2, "Ask surfaces candidates");

    // Nothing changed: no display names written, no extra enrollments.
    assert_eq!(display_name(&db, &rec_match, 1).await, before_match);
    assert_eq!(display_name(&db, &rec_named, 1).await, before_named1);
    assert_eq!(display_name(&db, &rec_named, 2).await, before_named2);
    assert!(
        db.named_voice_for(rec_match.as_str(), 1)
            .await
            .unwrap()
            .is_none(),
        "Ask enrolls nothing in the candidate recordings"
    );
    // Ada's sample count is unchanged (only the source sample).
    assert_eq!(named_voice_samples(&db, &ada).await, 1);
}

#[tokio::test]
async fn propagation_auto_backfills_unnamed_keeps_named_and_is_idempotent() {
    let (db, ada, rec_match, rec_named, far) = propagation_fixture().await;
    let cands = db
        .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    let targets: Vec<(RecordingId, i64)> = cands
        .iter()
        .map(|c| (c.recording_id.clone(), c.speaker_label))
        .collect();

    let applied = db.apply_propagation(&ada, &targets).await.unwrap();
    assert_eq!(applied.len(), 2, "both unnamed Ada-matches back-filled");

    // The unnamed speakers now read "Ada" and are enrolled under it.
    assert_eq!(
        display_name(&db, &rec_match, 1).await.as_deref(),
        Some("Ada")
    );
    assert_eq!(
        display_name(&db, &rec_named, 1).await.as_deref(),
        Some("Ada")
    );
    assert_eq!(
        db.named_voice_for(rec_match.as_str(), 1)
            .await
            .unwrap()
            .as_deref(),
        Some(ada.as_str())
    );
    // The already-named "Bob" is untouched.
    assert_eq!(
        display_name(&db, &rec_named, 2).await.as_deref(),
        Some("Bob")
    );
    // The dissimilar "far" voice is never named.
    assert!(display_name(&db, &far, 1).await.is_none());

    // Idempotent: re-running with the same targets back-fills nothing new.
    let again = db.apply_propagation(&ada, &targets).await.unwrap();
    assert!(
        again.is_empty(),
        "re-running propagation does no duplicate work"
    );
    // And re-scanning yields no candidates (they're all named now).
    let after = db
        .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert!(after.is_empty(), "no unnamed matches remain");
}

#[tokio::test]
async fn apply_propagation_never_overwrites_an_existing_name() {
    // Even if a caller hands apply_propagation an already-named target, the
    // name is preserved (defense in depth on top of the candidate filter).
    let (db, ada, _rec_match, rec_named, _far) = propagation_fixture().await;
    let applied = db
        .apply_propagation(&ada, &[(rec_named.clone(), 2)]) // label 2 == "Bob"
        .await
        .unwrap();
    assert!(applied.is_empty(), "an already-named speaker is skipped");
    assert_eq!(
        display_name(&db, &rec_named, 2).await.as_deref(),
        Some("Bob")
    );
}

#[tokio::test]
async fn apply_propagation_skips_target_without_voiceprint() {
    // A target that has a name written but no captured voiceprint must not be
    // left half-applied (display-named but unenrolled): apply rolls the name
    // back so the state is clean.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();

    // A recording with no voiceprint for label 1.
    let novp = embedded_recording(None);
    db.insert(&novp).await.unwrap();

    let applied = db
        .apply_propagation(&ada, &[(novp.id.clone(), 1)])
        .await
        .unwrap();
    assert!(applied.is_empty(), "no voiceprint → nothing enrolled");
    assert!(
        display_name(&db, &novp.id, 1).await.is_none(),
        "the name is rolled back so the target isn't half-applied"
    );
}

#[tokio::test]
async fn propagation_candidates_respect_threshold_strictly() {
    // A voice that's similar but below the bar is not a candidate; raising the
    // bar drops a marginal match.
    let (db, ada, rec_match, _rec_named, _far) = propagation_fixture().await;
    // rec_match's speaker is cos≈0.9999 to Ada — clears 0.5 but let's add a
    // marginal one at ~0.6 and bar it out at 0.8.
    let marg = embedded_recording(None);
    db.insert(&marg).await.unwrap();
    // cos([0.6,0.8,0],[1,0,0]) = 0.6.
    db.save_speaker_voiceprint(marg.id.as_str(), 1, &[0.6, 0.8, 0.0], 0)
        .await
        .unwrap();

    let strict = db
        .propagation_candidates(&ada, 0.8, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    let ids: Vec<&str> = strict.iter().map(|c| c.recording_id.as_str()).collect();
    assert!(
        ids.contains(&rec_match.as_str()),
        "strong match survives 0.8"
    );
    assert!(
        !ids.contains(&marg.id.as_str()),
        "0.6 match is below 0.8 bar"
    );
}

// ---- Reversible forget (soft-delete) (V5) -------------------------------

#[tokio::test]
async fn forget_soft_deletes_then_undo_restores() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 5000)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(db.list_named_voices().await.unwrap().len(), 1);

    // Forget hides it from listing and recognition.
    assert!(
        db.forget_named_voice(&ada).await.unwrap(),
        "live voice forgotten"
    );
    assert!(
        db.list_named_voices().await.unwrap().is_empty(),
        "forgotten voice is hidden from list-named-voices"
    );
    // A deleted voice doesn't match in recognition.
    let sugg = db
        .recognize_speakers_for(src.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert!(sugg.is_empty(), "a forgotten voice never matches");
    // Its capture is unlinked.
    assert!(db
        .named_voice_for(src.id.as_str(), 1)
        .await
        .unwrap()
        .is_none());

    // Idempotent: forgetting again is a no-op.
    assert!(
        !db.forget_named_voice(&ada).await.unwrap(),
        "already forgotten"
    );

    // Undo restores it: visible again, capture re-linked, centroid recomputed.
    assert!(
        db.undo_forget(&ada).await.unwrap(),
        "forgotten voice restored"
    );
    let voices = db.list_named_voices().await.unwrap();
    assert_eq!(voices.len(), 1, "restored voice is listed again");
    assert_eq!(voices[0].id, ada);
    assert_eq!(
        db.named_voice_for(src.id.as_str(), 1)
            .await
            .unwrap()
            .as_deref(),
        Some(ada.as_str()),
        "the capture is re-linked on undo"
    );
    assert_eq!(
        named_voice_samples(&db, &ada).await,
        1,
        "centroid recomputed"
    );

    // Idempotent: undo on a live voice is a no-op.
    assert!(!db.undo_forget(&ada).await.unwrap(), "already live");
}

#[tokio::test]
async fn undo_forget_does_not_clobber_a_later_reenrollment() {
    // A capture re-named onto a different voice after the forget keeps its
    // newer assignment when the original is undone.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();

    db.forget_named_voice(&ada).await.unwrap();
    // Re-name that speaker to Bob while Ada is forgotten.
    db.set_speaker_name(&src.id, 1, "Bob").await.unwrap();
    let bob = db
        .enroll_speaker(src.id.as_str(), 1, "Bob")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(bob, ada);

    // Undo Ada — must not steal the capture back from Bob.
    assert!(db.undo_forget(&ada).await.unwrap());
    assert_eq!(
        db.named_voice_for(src.id.as_str(), 1)
            .await
            .unwrap()
            .as_deref(),
        Some(bob.as_str()),
        "a re-enrolled capture keeps its newer voice"
    );
    assert_eq!(
        named_voice_samples(&db, &ada).await,
        0,
        "Ada has no captures back"
    );
}

#[tokio::test]
async fn reusing_a_forgotten_name_makes_a_fresh_voice() {
    // Naming a new speaker with a forgotten voice's name creates a new live
    // voice, not a revival of the tombstone.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada1 = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    db.forget_named_voice(&ada1).await.unwrap();

    let rec = embedded_recording(None);
    db.insert(&rec).await.unwrap();
    db.save_speaker_voiceprint(rec.id.as_str(), 1, &[0.0, 1.0, 0.0], 0)
        .await
        .unwrap();
    let ada2 = db
        .enroll_speaker(rec.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(ada1, ada2, "a forgotten name yields a fresh voice id");
    let voices = db.list_named_voices().await.unwrap();
    assert_eq!(voices.len(), 1, "only the new live Ada is listed");
    assert_eq!(voices[0].id, ada2);
}

#[tokio::test]
async fn name_propagation_config_defaults_to_ask() {
    // The default-Ask contract at the config layer.
    let diar = crate::config::DiarizationConfig::default();
    assert_eq!(diar.name_propagation, crate::config::NamePropagation::Ask);
    // Round-trips through serde as snake_case.
    let json = serde_json::to_string(&crate::config::NamePropagation::Auto).unwrap();
    assert_eq!(json, "\"auto\"");
    let off: crate::config::NamePropagation = serde_json::from_str("\"off\"").unwrap();
    assert_eq!(off, crate::config::NamePropagation::Off);
    // A config missing the field deserializes to Ask (serde default).
    let de: crate::config::NamePropagation = serde_json::from_str("null").unwrap_or_default();
    assert_eq!(de, crate::config::NamePropagation::Ask);
}

#[tokio::test]
async fn soft_delete_migration_applies_and_deleted_at_filters() {
    // The migration chain applies cleanly on a fresh DB (Catalog::open runs
    // it), the new column exists, and it filters listing. A direct UPDATE
    // proves the column is queryable and the read path honors it.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let src = embedded_recording(None);
    db.insert(&src).await.unwrap();
    db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
        .await
        .unwrap();
    let ada = db
        .enroll_speaker(src.id.as_str(), 1, "Ada")
        .await
        .unwrap()
        .unwrap();
    // Stamp deleted_at directly (the column exists post-migration).
    sqlx::query("UPDATE named_voiceprints SET deleted_at = datetime('now') WHERE id = ?")
        .bind(&ada)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        db.list_named_voices().await.unwrap().is_empty(),
        "deleted_at IS NOT NULL is filtered from listing"
    );
}

#[tokio::test]
async fn mean_confidence_round_trips_and_clears() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // Fresh insert: no aggregate yet (NULL), the graceful default.
    assert_eq!(db.get(&r.id).await.unwrap().unwrap().mean_confidence, None);

    // Store a mean, then read it back through the DTO.
    db.update_confidence(&r.id, Some(0.42)).await.unwrap();
    let mc = db.get(&r.id).await.unwrap().unwrap().mean_confidence;
    assert!(mc.is_some_and(|m| (m - 0.42).abs() < 1e-6), "got {mc:?}");

    // A retranscribe that drops to a no-confidence provider clears it back to NULL.
    db.update_confidence(&r.id, None).await.unwrap();
    assert_eq!(db.get(&r.id).await.unwrap().unwrap().mean_confidence, None);
}

#[tokio::test]
async fn low_confidence_filter_excludes_null_and_high() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // Three recordings: one low (0.4), one high (0.9), one with no aggregate
    // (NULL — an older row or a cloud transcript).
    let low = embedded_recording(None);
    db.insert(&low).await.unwrap();
    db.update_confidence(&low.id, Some(0.4)).await.unwrap();

    let high = embedded_recording(None);
    db.insert(&high).await.unwrap();
    db.update_confidence(&high.id, Some(0.9)).await.unwrap();

    let unknown = embedded_recording(None);
    db.insert(&unknown).await.unwrap();
    // Leave mean_confidence NULL.

    // Filter below 0.6: only the low one — NULL and high are excluded, so older
    // rows / cloud transcripts are never wrongly flagged.
    let filter = ListFilter {
        low_confidence_below: Some(0.6),
        ..Default::default()
    };
    let hits = db.list(&filter).await.unwrap();
    assert_eq!(hits.len(), 1, "only the below-threshold recording matches");
    assert_eq!(hits[0].id.as_str(), low.id.as_str());

    // No filter: all three come back.
    assert_eq!(db.list(&ListFilter::default()).await.unwrap().len(), 3);

    // Exactly at the threshold is NOT low (strict `<`).
    db.update_confidence(&low.id, Some(0.6)).await.unwrap();
    let hits = db.list(&filter).await.unwrap();
    assert!(hits.is_empty(), "mean == threshold is not below it");
}

#[tokio::test]
async fn insert_restored_round_trips_mean_confidence() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mut r = embedded_recording(None);
    r.mean_confidence = Some(0.73);
    db.insert_restored(&r).await.unwrap();
    let back = db.get(&r.id).await.unwrap().unwrap().mean_confidence;
    assert!(
        back.is_some_and(|m| (m - 0.73).abs() < 1e-6),
        "got {back:?}"
    );
}

#[tokio::test]
async fn detected_language_round_trips_and_clears() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = embedded_recording(None);
    db.insert(&r).await.unwrap();

    // Fresh insert: no detected language yet (NULL), the graceful default.
    assert_eq!(
        db.get(&r.id).await.unwrap().unwrap().detected_language,
        None
    );

    // Store a code, then read it back through the DTO.
    db.set_detected_language(&r.id, Some("es")).await.unwrap();
    assert_eq!(
        db.get(&r.id)
            .await
            .unwrap()
            .unwrap()
            .detected_language
            .as_deref(),
        Some("es")
    );

    // A retranscribe that drops to a detection-less provider clears it to NULL.
    db.set_detected_language(&r.id, None).await.unwrap();
    assert_eq!(
        db.get(&r.id).await.unwrap().unwrap().detected_language,
        None
    );
}

#[tokio::test]
async fn insert_restored_round_trips_detected_language() {
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mut r = embedded_recording(None);
    r.detected_language = Some("fr".into());
    db.insert_restored(&r).await.unwrap();
    assert_eq!(
        db.get(&r.id)
            .await
            .unwrap()
            .unwrap()
            .detected_language
            .as_deref(),
        Some("fr")
    );
}

// ── Ask my archive: retrieve_context (local-RAG retrieval + citations) ───────
//
// These exercise the new `retrieve_context` helper that grounds an Ask answer:
// same hybrid retrieval as the search bar, plus best-chunk recovery so a `[n]`
// citation can point at the exact passage. The ONNX model isn't bundled in
// tests, so the embedding space is simulated with synthetic vectors (the same
// technique the hybrid_search tests use), and stored chunk vectors are aligned
// by ordinal to `chunk_transcript(transcript)` so a citation index is checkable.

/// Build a transcript long enough that `chunk_transcript` splits it into several
/// chunks, with each chunk carrying a distinctive sentence at a known ordinal so
/// the recovered citation text is verifiable. Returns the transcript.
fn multi_chunk_transcript() -> String {
    // Each "block" is ~40 words; CHUNK_TARGET_WORDS is 80, so two blocks fill a
    // chunk and the transcript reliably splits into multiple chunks.
    let filler = "this sentence carries several ordinary words that pad the block out toward the chunk target so the splitter keeps moving forward through the note. ";
    let mut t = String::new();
    for marker in ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"] {
        // A distinctive marker sentence, then filler, so each region of the
        // transcript is identifiable in the recovered chunk text.
        t.push_str(&format!("the distinctive {marker} topic begins here. "));
        t.push_str(filler);
        t.push_str(filler);
    }
    t
}

#[tokio::test]
async fn retrieve_context_recovers_the_matching_chunk_and_its_text() {
    // Citation correctness: store one vector per chunk, make the query identical
    // to a chosen chunk's vector, and assert retrieve_context recovers that
    // chunk's index and its exact `chunk_transcript` text.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let transcript = multi_chunk_transcript();
    let chunks = crate::chunk::chunk_transcript(&transcript);
    assert!(
        chunks.len() >= 3,
        "the fixture must split into several chunks, got {}",
        chunks.len()
    );

    let mut rec = embedded_recording(None);
    rec.transcript = Some(transcript.clone());
    db.insert(&rec).await.unwrap();

    // One distinct synthetic vector per chunk: the i-th chunk's vector is the
    // i-th basis-ish direction, so the argmax is unambiguous.
    let dim = chunks.len();
    let mut vectors: Vec<Vec<f32>> = Vec::new();
    for i in 0..dim {
        let mut v = vec![0.0_f32; dim];
        v[i] = 1.0;
        vectors.push(v);
    }
    db.upsert_chunk_embeddings(&rec.id, &vectors).await.unwrap();

    // Query identical to chunk index 2 (the obvious match).
    let target_idx = 2usize;
    let mut query = vec![0.0_f32; dim];
    query[target_idx] = 1.0;

    let hits = db
        .retrieve_context("the distinctive charlie topic", &query, 8, 0.12, None)
        .await
        .unwrap();
    assert!(!hits.is_empty(), "the recording must be retrieved");
    let top = &hits[0];
    assert_eq!(top.recording_id.as_str(), rec.id.as_str());
    assert_eq!(
        top.chunk_index, target_idx as i64,
        "the recovered chunk index must be the argmax chunk"
    );
    assert_eq!(
        top.text, chunks[target_idx],
        "the recovered text must be exactly chunk_transcript(transcript)[idx]"
    );
    assert!(top.relevance > 0.5, "an exact chunk match reads as strong");
    assert!(!top.is_lexical, "a vector winner isn't a lexical-only hit");
}

#[tokio::test]
async fn retrieve_context_is_deterministic_and_meeting_deduped() {
    // Distinct cosines ⇒ deterministic RRF order; two tracks of one meeting
    // collapse to a single RetrievedChunk.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    let mut a = embedded_recording(None);
    a.transcript = Some("the project kickoff covered the budget and the timeline in detail".into());
    let mut b = embedded_recording(None);
    b.transcript = Some("notes about the office coffee machine being broken again".into());
    db.insert(&a).await.unwrap();
    db.insert(&b).await.unwrap();
    // a is clearly closer to the query axis than b (distinct cosines), but b's
    // cosine (0.5) must still clear the calibrated floor for `min_relevance = 0.12`
    // below — `calibrate_cosine(0.5) ≈ 0.64` — so this exercises two SURVIVING,
    // deterministically-ordered results. (b's old 0.2 cosine calibrated to ~0.09,
    // just under the 0.12 floor, so it was dropped and only one result came back.)
    db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db.upsert_chunk_embeddings(&b.id, &[vec![0.5, 0.87, 0.0]])
        .await
        .unwrap();

    let q = [1.0_f32, 0.0, 0.0];
    let first = db
        .retrieve_context("kickoff budget", &q, 8, 0.12, None)
        .await
        .unwrap();
    let second = db
        .retrieve_context("kickoff budget", &q, 8, 0.12, None)
        .await
        .unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(
        first
            .iter()
            .map(|c| c.recording_id.as_str().to_string())
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|c| c.recording_id.as_str().to_string())
            .collect::<Vec<_>>(),
        "distinct cosines give a stable order across runs"
    );
    assert_eq!(
        first[0].recording_id.as_str(),
        a.id.as_str(),
        "closer recording ranks first"
    );

    // Meeting dedupe: two tracks, one meeting_id → one result.
    let db2 = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mut mic = embedded_recording(Some("m1"));
    mic.transcript = Some("the mic track of the planning meeting".into());
    let mut sys = embedded_recording(Some("m1"));
    sys.transcript = Some("the system track of the planning meeting".into());
    db2.insert(&mic).await.unwrap();
    db2.insert(&sys).await.unwrap();
    db2.upsert_chunk_embeddings(&mic.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    db2.upsert_chunk_embeddings(&sys.id, &[vec![0.9, 0.1, 0.0]])
        .await
        .unwrap();
    let hits = db2
        .retrieve_context("planning meeting", &q, 8, 0.12, None)
        .await
        .unwrap();
    assert_eq!(
        hits.len(),
        1,
        "a meeting's two tracks collapse to one chunk, got {hits:?}"
    );
}

#[tokio::test]
async fn retrieve_context_clamps_index_when_transcript_shortened() {
    // Stored vectors for 4 chunks, then the transcript is edited down to ~1
    // chunk. The argmax may land past the live transcript's chunk count, so the
    // index must clamp and the text must stay non-empty.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let long = multi_chunk_transcript();
    let long_chunks = crate::chunk::chunk_transcript(&long);
    assert!(long_chunks.len() >= 4);

    let mut rec = embedded_recording(None);
    rec.transcript = Some(long.clone());
    db.insert(&rec).await.unwrap();
    // Store one vector per original chunk; the LAST one is the query winner.
    let dim = long_chunks.len();
    let mut vectors: Vec<Vec<f32>> = Vec::new();
    for i in 0..dim {
        let mut v = vec![0.0_f32; dim];
        v[i] = 1.0;
        vectors.push(v);
    }
    db.upsert_chunk_embeddings(&rec.id, &vectors).await.unwrap();

    // Edit the transcript down to a single short chunk (the stored vectors now
    // outnumber the live chunks).
    db.update_user_transcript(&rec.id, "now it is just a short one-liner.")
        .await
        .unwrap();

    // Query the LAST stored chunk's vector — its ordinal is past the new chunk
    // count, so the index must clamp to the last live chunk.
    let mut query = vec![0.0_f32; dim];
    query[dim - 1] = 1.0;
    let hits = db
        .retrieve_context("one-liner", &query, 8, 0.12, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    let live_chunks = crate::chunk::chunk_transcript("now it is just a short one-liner.");
    assert!(
        hits[0].chunk_index >= 0 && (hits[0].chunk_index as usize) < live_chunks.len().max(1),
        "the chunk index must clamp into the shortened transcript, got {}",
        hits[0].chunk_index
    );
    assert!(!hits[0].text.trim().is_empty(), "text must never be empty");
}

#[tokio::test]
async fn retrieve_context_lexical_only_hit_uses_prefix_and_floor() {
    // A recording matched only by FTS (its vector is orthogonal to the query and
    // there's no per-chunk vector worth citing): chunk_index = -1, is_lexical,
    // a non-empty transcript-prefix snippet, and the lexical relevance floor.
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let mut rec = embedded_recording(None);
    rec.transcript = Some("the Kubernetes rollout runbook is attached to the ticket".into());
    db.insert(&rec).await.unwrap();
    // No chunk vectors at all (legacy/lexical-only): nothing to argmax over.

    let hits = db
        .retrieve_context("Kubernetes", &[1.0, 0.0, 0.0], 8, 0.12, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "the exact-term lexical hit must surface");
    let top = &hits[0];
    assert_eq!(top.recording_id.as_str(), rec.id.as_str());
    assert_eq!(top.chunk_index, -1, "no per-chunk vector ⇒ index -1");
    assert!(top.is_lexical, "an FTS-only hit is flagged lexical");
    assert!(
        !top.text.trim().is_empty(),
        "the snippet must be a non-empty prefix"
    );
    assert!(
        top.text.starts_with("the Kubernetes rollout"),
        "the snippet is the transcript prefix, got {:?}",
        top.text
    );
    assert!(
        (top.relevance - 0.30).abs() < 1e-6,
        "a lexical-only hit is floored to 0.30, got {}",
        top.relevance
    );
}

#[tokio::test]
async fn retrieve_context_drops_weak_semantic_and_scopes_by_filter() {
    // Floor: a semantic-only hit below min_relevance is dropped. Filter: a scope
    // restricts results to in-scope keys (parity with the hybrid_search S3 test).
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

    // In-scope, strong match. `Catalog::insert` only writes the columns a fresh
    // recording starts with — `favorite` is filled later via `set_favorite`, never
    // by `insert` — so the flag must be set through that setter after insert (just
    // setting `r.favorite` on the struct passed to `insert` is silently dropped).
    let mut keep = embedded_recording(None);
    keep.transcript = Some("the migration plan for the data warehouse rollout".into());
    let mut weak = embedded_recording(None);
    weak.transcript = Some("totally unrelated grocery list for the weekend".into());
    db.insert(&keep).await.unwrap();
    db.insert(&weak).await.unwrap();
    db.set_favorite(&keep.id, true).await.unwrap(); // we'll scope by favorite
    db.set_favorite(&weak.id, true).await.unwrap();
    db.upsert_chunk_embeddings(&keep.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();
    // `weak` is near-orthogonal: calibrates well below the 0.5 floor we use here.
    db.upsert_chunk_embeddings(&weak.id, &[vec![0.18, 0.0, 0.98]])
        .await
        .unwrap();

    // High floor (0.5) drops the weak semantic-only hit.
    let q = [1.0_f32, 0.0, 0.0];
    let hits = db
        .retrieve_context("migration plan", &q, 8, 0.5, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "the weak semantic-only hit is floored out");
    assert_eq!(hits[0].recording_id.as_str(), keep.id.as_str());

    // A non-favorite, in-axis recording exists but is out of scope.
    let mut out_of_scope = embedded_recording(None);
    out_of_scope.transcript = Some("another migration plan, but not favorited".into());
    out_of_scope.favorite = false;
    db.insert(&out_of_scope).await.unwrap();
    db.upsert_chunk_embeddings(&out_of_scope.id, &[vec![1.0, 0.0, 0.0]])
        .await
        .unwrap();

    let scope = ListFilter {
        favorite: Some(true),
        ..Default::default()
    };
    let scoped = db
        .retrieve_context("migration plan", &q, 8, 0.12, Some(&scope))
        .await
        .unwrap();
    assert!(
        scoped
            .iter()
            .all(|c| c.recording_id.as_str() != out_of_scope.id.as_str()),
        "an out-of-scope recording must be excluded by the filter"
    );
    assert!(
        scoped
            .iter()
            .any(|c| c.recording_id.as_str() == keep.id.as_str()),
        "the in-scope strong match survives the filter"
    );
}

/// Minimal `Done` recording for the entity tests — only the columns
/// `set_entities` / `list_entities` care about matter; the rest are inert.
fn entity_test_recording() -> Recording {
    Recording {
        id: RecordingId::new(),
        started_at: Local::now(),
        duration_ms: 1000,
        audio_path: "x.wav".into(),
        transcript: Some("hello".into()),
        model: Some("tiny".into()),
        status: RecordingStatus::Done,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    }
}

#[tokio::test]
async fn entities_round_trip_and_populate_on_get_and_list() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = entity_test_recording();
    db.insert(&r).await.expect("insert");

    let entities = vec![
        Entity {
            kind: "person".into(),
            value: "Ada Lovelace".into(),
        },
        Entity {
            kind: "org".into(),
            value: "Analytical Engine Co".into(),
        },
        Entity {
            kind: "topic".into(),
            value: "compiler design".into(),
        },
    ];
    db.set_entities(&r.id, &entities)
        .await
        .expect("set entities");
    db.set_entities_model(&r.id, "phi3:mini")
        .await
        .expect("set entities model");

    // list_entities returns them kind-then-value sorted.
    let listed = db.list_entities(&r.id).await.expect("list entities");
    assert_eq!(listed.len(), 3);
    assert_eq!(
        listed[0],
        Entity {
            kind: "org".into(),
            value: "Analytical Engine Co".into()
        }
    );
    assert_eq!(
        listed[1],
        Entity {
            kind: "person".into(),
            value: "Ada Lovelace".into()
        }
    );
    assert_eq!(
        listed[2],
        Entity {
            kind: "topic".into(),
            value: "compiler design".into()
        }
    );

    // get() populates Recording::entities + entities_model.
    let fetched = db.get(&r.id).await.expect("get").expect("some");
    assert_eq!(fetched.entities.len(), 3);
    assert_eq!(fetched.entities_model.as_deref(), Some("phi3:mini"));

    // list() populates them per-row too.
    let rows = db
        .list(&ListFilter {
            limit: Some(10),
            ..Default::default()
        })
        .await
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entities.len(), 3);
}

#[tokio::test]
async fn set_entities_replaces_wholesale_and_empty_clears() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = entity_test_recording();
    db.insert(&r).await.expect("insert");

    db.set_entities(
        &r.id,
        &[Entity {
            kind: "person".into(),
            value: "First".into(),
        }],
    )
    .await
    .expect("set 1");
    // A second set replaces (DELETE-then-insert), it does not accumulate.
    db.set_entities(
        &r.id,
        &[Entity {
            kind: "topic".into(),
            value: "Second".into(),
        }],
    )
    .await
    .expect("set 2");
    let listed = db.list_entities(&r.id).await.expect("list");
    assert_eq!(
        listed,
        vec![Entity {
            kind: "topic".into(),
            value: "Second".into()
        }]
    );

    // An empty slice clears the set.
    db.set_entities(&r.id, &[]).await.expect("clear");
    assert!(db.list_entities(&r.id).await.expect("list").is_empty());
}

#[tokio::test]
async fn list_all_entities_dedupes_and_filters_by_kind() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r1 = entity_test_recording();
    let r2 = entity_test_recording();
    db.insert(&r1).await.expect("insert r1");
    db.insert(&r2).await.expect("insert r2");

    // The same (kind, value) appears in both recordings — list_all_entities
    // de-dupes it to one row.
    let shared = Entity {
        kind: "topic".into(),
        value: "shared topic".into(),
    };
    db.set_entities(
        &r1.id,
        &[
            shared.clone(),
            Entity {
                kind: "person".into(),
                value: "Alice".into(),
            },
        ],
    )
    .await
    .expect("set r1");
    db.set_entities(
        &r2.id,
        &[
            shared.clone(),
            Entity {
                kind: "person".into(),
                value: "Bob".into(),
            },
        ],
    )
    .await
    .expect("set r2");

    let all = db.list_all_entities().await.expect("list all");
    // shared topic (once) + Alice + Bob = 3 distinct.
    assert_eq!(all.len(), 3);
    assert_eq!(all.iter().filter(|e| e.value == "shared topic").count(), 1);

    // entities_by_kind slices to one kind.
    let people = db.entities_by_kind("person").await.expect("by kind");
    assert_eq!(people.len(), 2);
    assert!(people.iter().all(|e| e.kind == "person"));
}

#[tokio::test]
async fn deleting_a_recording_cascades_its_entities() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = entity_test_recording();
    db.insert(&r).await.expect("insert");
    db.set_entities(
        &r.id,
        &[Entity {
            kind: "term".into(),
            value: "FK cascade".into(),
        }],
    )
    .await
    .expect("set");
    db.delete(&r.id).await.expect("delete");
    // The FK ON DELETE CASCADE took the entity rows with the recording.
    assert!(db.list_all_entities().await.expect("list all").is_empty());
}

// ── Auto-chapters ────────────────────────────────────────────────────────────

#[tokio::test]
async fn chapters_replace_round_trip_and_cascade() {
    use crate::types::Chapter;
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = entity_test_recording();
    db.insert(&r).await.unwrap();

    // No chapters yet is a normal (empty) state, not an error.
    assert!(db.chapters_for(&r.id).await.unwrap().is_empty());

    let first = vec![
        Chapter {
            start_ms: 0,
            end_ms: 5000,
            title: "Intro".into(),
            summary: Some("Kick-off and agenda".into()),
        },
        Chapter {
            start_ms: 5000,
            end_ms: 12000,
            title: "Design".into(),
            summary: None,
        },
    ];
    db.replace_chapters(&r.id, &first).await.unwrap();
    assert_eq!(db.chapters_for(&r.id).await.unwrap(), first);

    // A re-run replaces wholesale — fewer rows must not leave stale tail chapters.
    let second = vec![Chapter {
        start_ms: 0,
        end_ms: 3000,
        title: "Only one now".into(),
        summary: None,
    }];
    db.replace_chapters(&r.id, &second).await.unwrap();
    assert_eq!(db.chapters_for(&r.id).await.unwrap(), second);

    // An empty slice clears them.
    db.replace_chapters(&r.id, &[]).await.unwrap();
    assert!(db.chapters_for(&r.id).await.unwrap().is_empty());

    db.replace_chapters(&r.id, &first).await.unwrap();
    db.delete(&r.id).await.unwrap();
    assert!(
        db.chapters_for(&r.id).await.unwrap().is_empty(),
        "chapters must be cascade-deleted with their recording"
    );
}

#[tokio::test]
async fn set_chapters_model_lands_on_get() {
    use crate::types::Chapter;
    let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
    let r = entity_test_recording();
    db.insert(&r).await.unwrap();

    // Older row carries no chapter model.
    assert_eq!(db.get(&r.id).await.unwrap().unwrap().chapters_model, None);

    db.set_chapters_model(&r.id, "phi3:mini").await.unwrap();
    db.replace_chapters(
        &r.id,
        &[Chapter {
            start_ms: 0,
            end_ms: 1000,
            title: "A".into(),
            summary: None,
        }],
    )
    .await
    .unwrap();

    let fetched = db.get(&r.id).await.unwrap().unwrap();
    assert_eq!(fetched.chapters_model.as_deref(), Some("phi3:mini"));
    // Chapters are NOT carried on the Recording DTO — they are fetched lazily.
    assert_eq!(db.chapters_for(&r.id).await.unwrap().len(), 1);
}

// ── Tasks (the per-recording child table + the mutable done flag) ────────────
//
// Mirrors the entity tests, plus the two task-only behaviors: the done-flag is
// preserved across re-extraction (the #1 risk in the brief), and `set_task_done`
// flips one row.

/// A task with no due hint, the common shape.
fn task(text: &str) -> Task {
    Task {
        id: 0,
        text: text.into(),
        due_hint: None,
        done: false,
    }
}

#[tokio::test]
async fn tasks_round_trip_and_populate_on_get_and_list() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = entity_test_recording();
    db.insert(&r).await.expect("insert");

    let tasks = vec![
        Task {
            id: 0,
            text: "Send the roadmap".into(),
            due_hint: Some("by Friday".into()),
            done: false,
        },
        task("Book the meeting room"),
    ];
    db.set_tasks(&r.id, &tasks).await.expect("set tasks");
    db.set_tasks_model(&r.id, "phi3:mini")
        .await
        .expect("set tasks model");

    // list_tasks returns them open-first then by id, carrying the row id + due hint.
    let listed = db.list_tasks(&r.id).await.expect("list tasks");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|t| t.id > 0), "row ids assigned");
    assert_eq!(listed[0].text, "Send the roadmap");
    assert_eq!(listed[0].due_hint.as_deref(), Some("by Friday"));
    assert!(!listed[0].done);

    // get() populates Recording::tasks + tasks_model.
    let fetched = db.get(&r.id).await.expect("get").expect("some");
    assert_eq!(fetched.tasks.len(), 2);
    assert_eq!(fetched.tasks_model.as_deref(), Some("phi3:mini"));

    // list() populates them per-row too.
    let rows = db
        .list(&ListFilter {
            limit: Some(10),
            ..Default::default()
        })
        .await
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tasks.len(), 2);
}

#[tokio::test]
async fn set_tasks_preserves_done_flag_across_reextraction() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = entity_test_recording();
    db.insert(&r).await.expect("insert");

    db.set_tasks(&r.id, &[task("Send the roadmap"), task("Book the room")])
        .await
        .expect("set");
    // The user checks off the first task.
    let listed = db.list_tasks(&r.id).await.expect("list");
    let roadmap_id = listed
        .iter()
        .find(|t| t.text == "Send the roadmap")
        .expect("roadmap task")
        .id;
    db.set_task_done(&r.id, roadmap_id, true)
        .await
        .expect("set done");

    // Re-extraction returns the SAME texts (all freshly done = false). The done
    // flag on the surviving text must be preserved — not silently un-checked.
    db.set_tasks(&r.id, &[task("Send the roadmap"), task("Book the room")])
        .await
        .expect("re-extract");
    let after = db.list_tasks(&r.id).await.expect("list after");
    let roadmap = after
        .iter()
        .find(|t| t.text == "Send the roadmap")
        .expect("roadmap survived");
    assert!(roadmap.done, "done flag survived re-extraction");
    // open-first ordering puts the still-open "Book the room" before the done one.
    assert_eq!(after[0].text, "Book the room");
    assert!(!after[0].done);
}

#[tokio::test]
async fn set_task_done_flips_one_row_and_reports_missing() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = entity_test_recording();
    db.insert(&r).await.expect("insert");
    db.set_tasks(&r.id, &[task("A"), task("B")])
        .await
        .expect("set");
    let listed = db.list_tasks(&r.id).await.expect("list");
    let a_id = listed.iter().find(|t| t.text == "A").unwrap().id;

    // Flipping one row affects exactly one and leaves the other alone.
    let affected = db.set_task_done(&r.id, a_id, true).await.expect("done");
    assert_eq!(affected, 1);
    let after = db.list_tasks(&r.id).await.expect("list");
    assert!(after.iter().find(|t| t.text == "A").unwrap().done);
    assert!(!after.iter().find(|t| t.text == "B").unwrap().done);

    // An unknown task id matches no row (0 affected) — the handler maps that to
    // not_found.
    let none = db
        .set_task_done(&r.id, 999_999, true)
        .await
        .expect("missing");
    assert_eq!(none, 0);
}

#[tokio::test]
async fn set_task_done_is_scoped_to_its_recording() {
    // A task can only be toggled through the recording it belongs to: naming a
    // different recording matches no row, so a client can't flip another
    // recording's task (nor mis-fire TasksUpdated for the wrong recording).
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r1 = entity_test_recording();
    let r2 = entity_test_recording();
    db.insert(&r1).await.expect("insert r1");
    db.insert(&r2).await.expect("insert r2");
    db.set_tasks(&r1.id, &[task("r1 task")])
        .await
        .expect("set r1");
    let t_id = db.list_tasks(&r1.id).await.expect("list r1")[0].id;

    // Wrong recording → nothing matched, the task stays open.
    let affected = db.set_task_done(&r2.id, t_id, true).await.expect("scoped");
    assert_eq!(affected, 0, "cross-recording toggle matches nothing");
    assert!(!db.list_tasks(&r1.id).await.expect("relist")[0].done);

    // Right recording → flips exactly that row.
    let affected = db.set_task_done(&r1.id, t_id, true).await.expect("owned");
    assert_eq!(affected, 1);
    assert!(db.list_tasks(&r1.id).await.expect("relist2")[0].done);
}

#[tokio::test]
async fn empty_set_tasks_clears_and_list_all_filters_open() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r1 = entity_test_recording();
    let r2 = entity_test_recording();
    db.insert(&r1).await.expect("insert r1");
    db.insert(&r2).await.expect("insert r2");

    db.set_tasks(&r1.id, &[task("r1 open"), task("r1 done")])
        .await
        .expect("set r1");
    db.set_tasks(&r2.id, &[task("r2 open")])
        .await
        .expect("set r2");
    // Mark one of r1's tasks done.
    let r1_tasks = db.list_tasks(&r1.id).await.expect("list r1");
    let done_id = r1_tasks.iter().find(|t| t.text == "r1 done").unwrap().id;
    db.set_task_done(&r1.id, done_id, true).await.expect("done");

    // list_all_tasks(false) returns every task with its recording ref.
    let all = db.list_all_tasks(false).await.expect("all");
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|t| !t.recording_id.is_empty()));
    // list_all_tasks(true) drops the done one.
    let open = db.list_all_tasks(true).await.expect("open");
    assert_eq!(open.len(), 2);
    assert!(open.iter().all(|t| !t.done));

    // An empty slice clears a recording's tasks.
    db.set_tasks(&r1.id, &[]).await.expect("clear");
    assert!(db.list_tasks(&r1.id).await.expect("list").is_empty());
}

#[tokio::test]
async fn deleting_a_recording_cascades_its_tasks() {
    let db = Catalog::open(Path::new("sqlite::memory:"))
        .await
        .expect("open db");
    let r = entity_test_recording();
    db.insert(&r).await.expect("insert");
    db.set_tasks(&r.id, &[task("cascade me")])
        .await
        .expect("set");
    db.delete(&r.id).await.expect("delete");
    // The FK ON DELETE CASCADE took the task rows with the recording.
    assert!(db.list_all_tasks(false).await.expect("all").is_empty());
}

// ── ANN (approximate nearest-neighbour) index ───────────────────────────────
//
// These tests are gated behind the `ann-usearch` feature so the default CI lane
// stays C++-free; they are pure DB + in-process index tests with no real OS I/O,
// so they're safe under the unattended-test keystroke-injection rule. They use a
// file-based catalog (a TempDir) because the ANN sidecar needs an on-disk home —
// an in-memory `sqlite::memory:` catalog has no sidecar and the ANN stays
// disabled there by design.
#[cfg(feature = "ann-usearch")]
mod ann_tests {
    use super::*;
    use crate::config::AnnConfig;

    /// A tiny deterministic LCG so the synthetic corpus is reproducible without
    /// pulling an rng crate into dev-deps. Park–Miller minimal standard.
    struct Lcg(u64);
    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            // Top 24 bits → [0,1).
            ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
        }
    }

    /// One L2-normalized random vector of dimension `dim`.
    fn random_unit(rng: &mut Lcg, dim: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
        crate::embed::l2_normalize(&mut v);
        v
    }

    /// Open a file-based catalog with ANN enabled, under a TempDir.
    async fn open_ann_catalog(dir: &std::path::Path, cfg: AnnConfig) -> Catalog {
        let db = Catalog::open(&dir.join("catalog.db")).await.unwrap();
        db.set_ann_config(cfg);
        db
    }

    fn enabled_cfg() -> AnnConfig {
        AnnConfig {
            enabled: true,
            ..AnnConfig::default()
        }
    }

    /// Build (insert) → save → load → search → remove round-trip on the raw
    /// index type, independent of the catalog wiring.
    #[test]
    fn ann_index_build_save_load_search_remove_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let sidecar = dir.path().join("rt.ann");
        let cfg = enabled_cfg();
        let dim = 16;
        let mut rng = Lcg(42);
        let pairs: Vec<(u64, Vec<f32>)> = (0..50u64)
            .map(|k| (k + 1, random_unit(&mut rng, dim)))
            .collect();

        let index =
            crate::catalog::ann::AnnIndex::build_from_pairs(sidecar.clone(), dim, &pairs, &cfg)
                .unwrap();
        assert_eq!(index.len(), pairs.len());

        // The nearest neighbour of a stored vector is itself.
        let (key0, vec0) = &pairs[0];
        let hits = index.search(vec0, 5).unwrap();
        assert_eq!(
            hits[0].0, *key0,
            "a vector's own key is its nearest neighbour"
        );

        index.save().unwrap();
        assert!(sidecar.exists(), "save wrote the sidecar");

        // Load into a fresh index and confirm identical top hit.
        let loaded =
            crate::catalog::ann::AnnIndex::load_verified(sidecar.clone(), dim, pairs.len(), &cfg)
                .unwrap();
        assert_eq!(loaded.len(), pairs.len());
        let hits2 = loaded.search(vec0, 5).unwrap();
        assert_eq!(hits2[0].0, *key0, "loaded index returns the same top hit");

        // Remove drops the vector.
        loaded.remove(*key0).unwrap();
        assert_eq!(loaded.len(), pairs.len() - 1);
    }

    /// The correctness gate: ANN top-10 agrees with brute force on a seeded
    /// corpus at recall@10 >= 0.95, with bit-identical scores for the overlap.
    #[tokio::test]
    async fn ann_recall_at_10_matches_brute_force() {
        let dir = tempfile::TempDir::new().unwrap();
        let dim = 32;
        let n = 400usize;
        let mut rng = Lcg(7);

        // Brute-force reference catalog (ANN off) and the ANN catalog, populated
        // identically so the only difference is the retrieval path. Each lives in
        // its own subdir so their catalog.db / catalog.ann files don't collide.
        let brute_dir = dir.path().join("brute");
        let ann_dir = dir.path().join("ann");
        std::fs::create_dir_all(&brute_dir).unwrap();
        std::fs::create_dir_all(&ann_dir).unwrap();
        let brute = Catalog::open(&brute_dir.join("catalog.db")).await.unwrap();
        let ann = open_ann_catalog(&ann_dir, enabled_cfg()).await;

        let mut corpus: Vec<(RecordingId, Vec<f32>)> = Vec::with_capacity(n);
        for _ in 0..n {
            let r = embedded_recording(None);
            brute.insert(&r).await.unwrap();
            ann.insert(&r).await.unwrap();
            let v = random_unit(&mut rng, dim);
            brute
                .upsert_chunk_embeddings(&r.id, std::slice::from_ref(&v))
                .await
                .unwrap();
            ann.upsert_chunk_embeddings(&r.id, std::slice::from_ref(&v))
                .await
                .unwrap();
            corpus.push((r.id, v));
        }

        // Build the ANN index from SQLite (the daemon's background-build path).
        ann.rebuild_ann_index().await.unwrap();
        let health = ann.ann_health().await;
        assert!(health.index_loaded, "index should be warm after rebuild");
        assert_eq!(health.index_vectors, n, "every chunk vector is indexed");

        // Run a battery of queries; compare the top-10 recording sets.
        let queries = 40;
        let mut total_overlap = 0usize;
        let mut total_expected = 0usize;
        for _ in 0..queries {
            let q = random_unit(&mut rng, dim);
            let brute_rank = brute.vector_ranking(&q).await.unwrap();
            let ann_rank = ann.vector_ranking(&q).await.unwrap();

            let brute_top: Vec<String> = brute_rank
                .iter()
                .take(10)
                .map(|(_, id, _)| id.as_str().to_string())
                .collect();
            let ann_top: std::collections::HashSet<String> = ann_rank
                .iter()
                .take(10)
                .map(|(_, id, _)| id.as_str().to_string())
                .collect();

            // Scores must be bit-identical for any recording both paths returned
            // (the ANN re-score uses the same cosine_similarity).
            let brute_score: std::collections::HashMap<&str, f32> = brute_rank
                .iter()
                .map(|(_, id, s)| (id.as_str(), *s))
                .collect();
            for (_, id, s) in &ann_rank {
                if let Some(bs) = brute_score.get(id.as_str()) {
                    assert_eq!(*bs, *s, "ANN re-score must equal brute-force score");
                }
            }

            for id in &brute_top {
                total_expected += 1;
                if ann_top.contains(id) {
                    total_overlap += 1;
                }
            }
        }
        let recall = total_overlap as f64 / total_expected as f64;
        assert!(
            recall >= 0.95,
            "recall@10 = {recall:.3} must be >= 0.95 (overlap {total_overlap}/{total_expected})"
        );
    }

    /// Lifecycle: embed → search (hit) → delete → search (gone); re-embed →
    /// old chunk gone, new chunk found. Mirrors the cache lifecycle tests.
    #[tokio::test]
    async fn ann_lifecycle_insert_delete_reembed() {
        let dir = tempfile::TempDir::new().unwrap();
        let dim = 8;
        let db = open_ann_catalog(dir.path(), enabled_cfg()).await;

        let a = embedded_recording(None);
        let b = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.insert(&b).await.unwrap();
        let mut va = vec![0.0f32; dim];
        va[0] = 1.0;
        let mut vb = vec![0.0f32; dim];
        vb[1] = 1.0;
        db.upsert_chunk_embeddings(&a.id, &[va.clone()])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&b.id, &[vb.clone()])
            .await
            .unwrap();
        db.rebuild_ann_index().await.unwrap();

        // Query on a's axis: a wins.
        let hit = db.vector_ranking(&va).await.unwrap();
        assert_eq!(hit[0].1.as_str(), a.id.as_str());

        // Delete a → its node leaves the index; b is now the only candidate.
        db.delete(&a.id).await.unwrap();
        let after = db.vector_ranking(&va).await.unwrap();
        assert!(
            !after.iter().any(|(_, id, _)| id.as_str() == a.id.as_str()),
            "deleted recording must not return from the ANN path"
        );
        assert_eq!(db.ann_health().await.index_vectors, 1, "a's node removed");

        // Re-embed b onto a's old axis: it now wins an a-axis query.
        db.upsert_chunk_embeddings(&b.id, &[va.clone()])
            .await
            .unwrap();
        let reembed = db.vector_ranking(&va).await.unwrap();
        assert_eq!(reembed[0].1.as_str(), b.id.as_str());
    }

    /// Fallback: a corrupt/dimension-mismatched sidecar must not panic and must
    /// fall through to brute force (same results).
    #[tokio::test]
    async fn ann_dim_mismatch_falls_back_to_brute_force() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = open_ann_catalog(dir.path(), enabled_cfg()).await;
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        // 4-dim corpus + index.
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.rebuild_ann_index().await.unwrap();

        // Query with a DIFFERENT dimension (3 vs the index's 4): the ANN path
        // declines (dim mismatch) and brute force scores nothing (it also skips
        // the dim-mismatched stored vector), so this is just a no-panic check.
        let res = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert!(
            res.is_empty(),
            "dim-mismatched query yields nothing, no panic"
        );

        // A matching-dim query still works via the ANN path.
        let ok = db.vector_ranking(&[1.0, 0.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(ok[0].1.as_str(), a.id.as_str());
    }

    /// With ANN enabled but no index built yet (cold), search must fall back to
    /// brute force and return correct results.
    #[tokio::test]
    async fn ann_cold_index_uses_brute_force() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = open_ann_catalog(dir.path(), enabled_cfg()).await;
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        // Deliberately do NOT rebuild: the index is cold.
        assert!(!db.ann_health().await.index_loaded);
        let res = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(res[0].1.as_str(), a.id.as_str(), "cold index → brute force");
    }

    /// Regression: an ANN search that returns hits but resolves ZERO `ann_keys`
    /// rows (every top-K node is stranded — its key row is gone) must fall back to
    /// brute force, not return an empty candidate set that makes `vector_ranking`
    /// skip every chunk. Strand the keys (the index still holds the vectors), then
    /// assert the search still returns the brute-force result.
    #[tokio::test]
    async fn ann_empty_key_resolution_falls_back_to_brute_force() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = open_ann_catalog(dir.path(), enabled_cfg()).await;

        let a = embedded_recording(None);
        let b = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.insert(&b).await.unwrap();
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&b.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();
        db.rebuild_ann_index().await.unwrap();

        // Sanity: with keys intact, the ANN path resolves and a-axis query wins a.
        let warm = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(warm[0].1.as_str(), a.id.as_str());
        assert!(db.ann_health().await.index_loaded, "index is warm");

        // Strand every node: the graph still returns hits, but their keys no
        // longer resolve to any recording. Delete the ann_keys rows directly (the
        // in-memory index is untouched, so search still produces top-K keys).
        sqlx::query("DELETE FROM ann_keys")
            .execute(&db.pool)
            .await
            .unwrap();
        // The index still holds the vectors (resolution, not the graph, is empty).
        assert_eq!(
            db.ann_health().await.index_vectors,
            2,
            "the in-memory graph is untouched by the ann_keys delete"
        );

        // The ANN candidate resolution is now empty for any query → fall back to
        // brute force, which scans the (still-present) embedding corpus and finds
        // the right recording. Before the fix this returned an empty candidate set
        // and `vector_ranking` produced no chunk hits.
        let after = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(
            after[0].1.as_str(),
            a.id.as_str(),
            "empty key resolution must fall back to brute force, not drop results"
        );
        let after_b = db.vector_ranking(&[0.0, 1.0, 0.0]).await.unwrap();
        assert_eq!(after_b[0].1.as_str(), b.id.as_str());
    }

    /// Regression: a recording added (via the incremental `sync_recording_to_ann`
    /// path inside `upsert_chunk_embeddings`) while a `rebuild_ann_index` is in its
    /// read-snapshot → build → swap window must not be silently missing from the
    /// rebuilt index. The rebuild re-checks the embedding generation under the
    /// swap lock and replays if an add raced it. Drive an add concurrently with the
    /// rebuild over several iterations and assert the warm index always covers
    /// every chunk in SQLite (no add lost until restart).
    #[tokio::test]
    async fn ann_rebuild_does_not_drop_concurrent_add() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = std::sync::Arc::new(open_ann_catalog(dir.path(), enabled_cfg()).await);

        // Seed a small corpus and build once so the index is warm.
        for _ in 0..5 {
            let r = embedded_recording(None);
            db.insert(&r).await.unwrap();
            db.upsert_chunk_embeddings(&r.id, &[vec![1.0, 0.0, 0.0]])
                .await
                .unwrap();
        }
        db.rebuild_ann_index().await.unwrap();

        // Repeatedly race a fresh add against a full rebuild. Whatever the
        // interleaving, the post-condition must hold: the warm index covers every
        // chunk row in SQLite.
        for _ in 0..6 {
            let r = embedded_recording(None);
            db.insert(&r).await.unwrap();
            let rid = r.id.clone();

            let rebuild_db = db.clone();
            let add_db = db.clone();
            let (rebuild_res, add_res) = tokio::join!(
                async move { rebuild_db.rebuild_ann_index().await },
                async move {
                    add_db
                        .upsert_chunk_embeddings(&rid, &[vec![1.0, 0.0, 0.0]])
                        .await
                }
            );
            rebuild_res.unwrap();
            add_res.unwrap();

            // One more deterministic rebuild settles any in-flight replay so the
            // count assertion isn't itself racing a build.
            db.rebuild_ann_index().await.unwrap();

            let health = db.ann_health().await;
            assert_eq!(
                health.index_vectors, health.sqlite_vectors,
                "every chunk in SQLite must be in the warm index (no add lost)"
            );
        }
    }
}
