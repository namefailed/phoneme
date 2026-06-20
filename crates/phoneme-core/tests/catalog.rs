use chrono::{Duration, Local, TimeZone};
use phoneme_core::{
    config::RetentionConfig, Catalog, ListFilter, Recording, RecordingId, RecordingStatus,
};
use tempfile::TempDir;

fn sample_recording(id: RecordingId) -> Recording {
    Recording {
        id,
        started_at: Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap(),
        duration_ms: 8470,
        audio_path: "C:/tmp/x.wav".into(),
        in_place: false,
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
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        tags: vec![],
        speaker_names: vec![],
    }
}

async fn fresh_catalog() -> (TempDir, Catalog) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("catalog.db");
    let catalog = Catalog::open(&path).await.expect("opens");
    (dir, catalog)
}

#[tokio::test]
async fn opens_creates_schema_when_missing() {
    let (_dir, _catalog) = fresh_catalog().await;
    // Reaching this point means migrations ran without error.
}

#[tokio::test]
async fn insert_then_get_returns_same_recording() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    let got = catalog.get(&rec.id).await.unwrap().expect("found");
    assert_eq!(got.id, rec.id);
    assert_eq!(got.audio_path, rec.audio_path);
    assert_eq!(got.status, RecordingStatus::Recording);
}

#[tokio::test]
async fn get_missing_returns_none() {
    let (_dir, catalog) = fresh_catalog().await;
    let id = RecordingId::new();
    let got = catalog.get(&id).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn update_status_advances_through_states() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_status(&rec.id, RecordingStatus::Transcribing)
        .await
        .unwrap();
    let got = catalog.get(&rec.id).await.unwrap().unwrap();
    assert_eq!(got.status, RecordingStatus::Transcribing);
}

#[tokio::test]
async fn update_transcript_persists_text() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_transcript(&rec.id, "hello world", "hello world", "gemma-4-E4B")
        .await
        .unwrap();
    let got = catalog.get(&rec.id).await.unwrap().unwrap();
    assert_eq!(got.transcript.as_deref(), Some("hello world"));
    assert_eq!(got.model.as_deref(), Some("gemma-4-E4B"));
}

#[tokio::test]
async fn update_summary_persists_text_and_model() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    // Defaults to absent until generated.
    let before = catalog.get(&rec.id).await.unwrap().unwrap();
    assert_eq!(before.summary, None);
    assert_eq!(before.summary_model, None);

    catalog
        .update_summary(&rec.id, "- key point\n- action item", Some("gemma3:4b"))
        .await
        .unwrap();
    let got = catalog.get(&rec.id).await.unwrap().unwrap();
    assert_eq!(got.summary.as_deref(), Some("- key point\n- action item"));
    assert_eq!(got.summary_model.as_deref(), Some("gemma3:4b"));
    // A summary must not disturb the stored transcript.
    assert_eq!(got.transcript, None);
}

#[tokio::test]
async fn list_returns_inserted_recordings_descending() {
    let (_dir, catalog) = fresh_catalog().await;
    let a = sample_recording(RecordingId::from_datetime(
        Local.with_ymd_and_hms(2026, 5, 19, 9, 0, 0).unwrap(),
    ));
    let b = sample_recording(RecordingId::from_datetime(
        Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap(),
    ));
    catalog.insert(&a).await.unwrap();
    catalog.insert(&b).await.unwrap();
    let list = catalog.list(&ListFilter::default()).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, b.id);
    assert_eq!(list[1].id, a.id);
}

#[tokio::test]
async fn list_respects_limit() {
    let (_dir, catalog) = fresh_catalog().await;
    for h in 0..5 {
        let rec = sample_recording(RecordingId::from_datetime(
            Local.with_ymd_and_hms(2026, 5, 19, h, 0, 0).unwrap(),
        ));
        catalog.insert(&rec).await.unwrap();
    }
    let list = catalog
        .list(&ListFilter {
            limit: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(list.len(), 2);
}

#[tokio::test]
async fn list_offset_paginates_without_overlap() {
    let (_dir, catalog) = fresh_catalog().await;
    // Insert 5 recordings at hours 0..5; default order is newest-first, so the
    // list is [h4, h3, h2, h1, h0].
    for h in 0..5 {
        let rec = sample_recording(RecordingId::from_datetime(
            Local.with_ymd_and_hms(2026, 5, 19, h, 0, 0).unwrap(),
        ));
        catalog.insert(&rec).await.unwrap();
    }
    let page = |limit, offset| {
        let catalog = &catalog;
        async move {
            catalog
                .list(&ListFilter {
                    limit: Some(limit),
                    offset: Some(offset),
                    ..Default::default()
                })
                .await
                .unwrap()
        }
    };

    let p1 = page(2, 0).await;
    let p2 = page(2, 2).await;
    let p3 = page(2, 4).await;
    assert_eq!(p1.len(), 2, "first page");
    assert_eq!(p2.len(), 2, "second page");
    assert_eq!(p3.len(), 1, "last page has the remainder");

    // Pages must be contiguous and non-overlapping (newest → oldest).
    let ids: Vec<_> = p1
        .iter()
        .chain(&p2)
        .chain(&p3)
        .map(|r| r.id.clone())
        .collect();
    let full = catalog.list(&ListFilter::default()).await.unwrap();
    let full_ids: Vec<_> = full.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids, full_ids, "paged ids reconstruct the full ordered list");

    // OFFSET without LIMIT skips the first N and returns the rest.
    let skip_first = catalog
        .list(&ListFilter {
            offset: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(skip_first.len(), 4);
    assert_eq!(skip_first[0].id, full[1].id);
}

#[tokio::test]
async fn list_filters_by_status() {
    let (_dir, catalog) = fresh_catalog().await;
    let r1 = sample_recording(RecordingId::new());
    let r2 = sample_recording(RecordingId::new());
    catalog.insert(&r1).await.unwrap();
    catalog.insert(&r2).await.unwrap();
    catalog
        .update_status(&r2.id, RecordingStatus::Done)
        .await
        .unwrap();
    let list = catalog
        .list(&ListFilter {
            status: Some(RecordingStatus::Done),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, r2.id);
}

#[tokio::test]
async fn search_finds_by_transcript_text() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_transcript(
            &rec.id,
            "remind me to email Sarah about the contract",
            "remind me to email Sarah about the contract",
            "m",
        )
        .await
        .unwrap();
    let hits = catalog
        .list(&ListFilter {
            search: Some("sarah".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, rec.id);
    let miss = catalog
        .list(&ListFilter {
            search: Some("nonexistent".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(miss.is_empty());
}

#[tokio::test]
async fn delete_removes_recording_and_fts_row() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_transcript(&rec.id, "deletable", "deletable", "m")
        .await
        .unwrap();
    catalog.delete(&rec.id).await.unwrap();
    assert!(catalog.get(&rec.id).await.unwrap().is_none());
    let search_res = catalog
        .list(&ListFilter {
            search: Some("deletable".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(search_res.is_empty());
}

#[tokio::test]
async fn update_hook_result_persists_exit_code() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_hook_result(&rec.id, "powershell -file foo.ps1", 0, 142)
        .await
        .unwrap();
    let got = catalog.get(&rec.id).await.unwrap().unwrap();
    assert_eq!(
        got.hook_command.as_deref(),
        Some("powershell -file foo.ps1")
    );
    assert_eq!(got.hook_exit_code, Some(0));
    assert_eq!(got.hook_duration_ms, Some(142));
}

#[tokio::test]
async fn tags_round_trip() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("work", Some("#f38ba8")).await.unwrap();
    assert_eq!(tag.name, "work");

    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog.attach_tag(&rec.id, tag.id).await.unwrap();

    let attached = catalog.tags_for(&rec.id).await.unwrap();
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0].name, "work");

    catalog.detach_tag(&rec.id, tag.id).await.unwrap();
    let after = catalog.tags_for(&rec.id).await.unwrap();
    assert!(after.is_empty());
}

// ── Sort direction ────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sort_ascending_returns_oldest_first() {
    let (_dir, catalog) = fresh_catalog().await;
    let a = sample_recording(RecordingId::from_datetime(
        Local.with_ymd_and_hms(2026, 5, 19, 9, 0, 0).unwrap(),
    ));
    let b = sample_recording(RecordingId::from_datetime(
        Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap(),
    ));
    catalog.insert(&a).await.unwrap();
    catalog.insert(&b).await.unwrap();

    let asc = catalog
        .list(&ListFilter {
            sort_desc: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(asc.len(), 2);
    assert_eq!(asc[0].id, a.id, "oldest should be first in ascending order");
    assert_eq!(asc[1].id, b.id);
}

#[tokio::test]
async fn list_sort_desc_none_defaults_to_newest_first() {
    let (_dir, catalog) = fresh_catalog().await;
    let old = sample_recording(RecordingId::from_datetime(
        Local.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ));
    let new = sample_recording(RecordingId::from_datetime(
        Local.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
    ));
    catalog.insert(&old).await.unwrap();
    catalog.insert(&new).await.unwrap();

    let list = catalog
        .list(&ListFilter {
            sort_desc: None,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(list[0].id, new.id, "None should default to newest-first");
}

// ── Tag management ────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_tag_persists_new_name_and_color() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("old name", Some("#ff0000")).await.unwrap();
    let updated = catalog
        .update_tag(tag.id, "new name", Some("#00ff00"))
        .await
        .unwrap();
    assert_eq!(updated.name, "new name");
    assert_eq!(updated.color.as_deref(), Some("#00ff00"));
}

#[tokio::test]
async fn update_tag_can_clear_color() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("colored", Some("#ff0000")).await.unwrap();
    let updated = catalog.update_tag(tag.id, "colored", None).await.unwrap();
    assert!(updated.color.is_none());
}

#[tokio::test]
async fn list_all_tags_includes_unattached_tags() {
    let (_dir, catalog) = fresh_catalog().await;
    let t1 = catalog.add_tag("attached", None).await.unwrap();
    let _t2 = catalog.add_tag("orphan", None).await.unwrap();

    // Attach t1 to a recording; leave t2 unattached.
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog.attach_tag(&rec.id, t1.id).await.unwrap();

    let all = catalog.list_all_tags().await.unwrap();
    let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"attached"), "attached tag must appear");
    assert!(
        names.contains(&"orphan"),
        "orphan tag must appear in list_all_tags"
    );

    // Contrast: list_tags (filter-dropdown variant) must exclude the orphan.
    let attached_only = catalog.list_tags().await.unwrap();
    let attached_names: Vec<&str> = attached_only.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !attached_names.contains(&"orphan"),
        "list_tags must exclude unattached tags"
    );
}

// ── Retention policy ──────────────────────────────────────────────────────────

/// Build a done recording whose started_at is `days_ago` days in the past.
async fn insert_done_recording_aged(catalog: &Catalog, days_ago: i64) -> Recording {
    let started_at = Local::now() - Duration::try_days(days_ago).unwrap();
    let id = RecordingId::from_datetime(started_at);
    let mut rec = sample_recording(id);
    rec.started_at = started_at;
    rec.audio_path = format!("/tmp/{}.wav", rec.id);
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_status(&rec.id, RecordingStatus::Done)
        .await
        .unwrap();
    rec
}

#[tokio::test]
async fn analyze_upcoming_retention_counts_only_terminal_rows_entering_window() {
    // Pre-deletion warning math (was untested). For max_age_days=30, hours_ahead=72
    // the "about to age out" window is started_at in [now-30d, now-27d): recordings
    // 28–30 days old will cross the 30-day deletion threshold within the next 72h.
    let (_dir, catalog) = fresh_catalog().await;

    insert_done_recording_aged(&catalog, 29).await; // in window, terminal -> COUNTS
    insert_done_recording_aged(&catalog, 31).await; // already past max_age -> not "upcoming"
    insert_done_recording_aged(&catalog, 10).await; // far too new -> not counted

    // In-progress recording inside the window must NOT count — only terminal
    // statuses (done / transcribe_failed / hook_failed) are eligible for deletion.
    let started = Local::now() - Duration::try_days(28).unwrap();
    let mut in_progress = sample_recording(RecordingId::from_datetime(started));
    in_progress.started_at = started;
    in_progress.audio_path = format!("/tmp/{}.wav", in_progress.id);
    catalog.insert(&in_progress).await.unwrap(); // stays RecordingStatus::Recording

    let cfg = RetentionConfig {
        max_age_days: Some(30),
        max_count: None,
        delete_audio: false,
    };
    let count = catalog.analyze_upcoming_retention(&cfg, 72).await.unwrap();
    assert_eq!(
        count, 1,
        "only the terminal recording entering the 72h window should be counted"
    );

    // No age policy => nothing is ever 'upcoming' (early return at max_age_days=None).
    let no_age = RetentionConfig {
        max_age_days: None,
        max_count: Some(5),
        delete_audio: false,
    };
    assert_eq!(
        catalog
            .analyze_upcoming_retention(&no_age, 72)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn apply_retention_age_deletes_old_removes_catalog_row() {
    let (_dir, catalog) = fresh_catalog().await;
    let old = insert_done_recording_aged(&catalog, 31).await;
    let recent = insert_done_recording_aged(&catalog, 1).await;

    let deleted_paths = catalog
        .apply_retention(&RetentionConfig {
            max_age_days: Some(30),
            max_count: None,
            delete_audio: false,
        })
        .await
        .unwrap();

    assert_eq!(
        deleted_paths.len(),
        1,
        "only the 31-day-old recording should be deleted"
    );
    assert_eq!(deleted_paths[0], old.audio_path);

    // Old recording gone from catalog; recent one survives.
    assert!(catalog.get(&old.id).await.unwrap().is_none());
    assert!(catalog.get(&recent.id).await.unwrap().is_some());
}

#[tokio::test]
async fn apply_retention_count_keeps_most_recent_n() {
    let (_dir, catalog) = fresh_catalog().await;
    // Insert 5 recordings newest→oldest.
    let recs: Vec<_> = (0..5)
        .map(|i| {
            let catalog = &catalog;
            async move { insert_done_recording_aged(catalog, i).await }
        })
        .collect();
    // Drive all futures sequentially (order matters for started_at).
    let mut inserted = Vec::new();
    for f in recs {
        inserted.push(f.await);
    }

    let deleted_paths = catalog
        .apply_retention(&RetentionConfig {
            max_age_days: None,
            max_count: Some(3),
            delete_audio: false,
        })
        .await
        .unwrap();

    assert_eq!(
        deleted_paths.len(),
        2,
        "2 of 5 should be pruned to keep newest 3"
    );

    // The 3 newest (days 0, 1, 2) must still be present.
    for rec in &inserted[..3] {
        assert!(
            catalog.get(&rec.id).await.unwrap().is_some(),
            "recent recording {} should survive",
            rec.id
        );
    }
}

#[tokio::test]
async fn apply_retention_ignores_in_progress_recordings() {
    let (_dir, catalog) = fresh_catalog().await;
    // A recording that is still transcribing — must never be deleted.
    let in_progress_id = RecordingId::from_datetime(Local::now() - Duration::try_days(60).unwrap());
    let mut in_progress = sample_recording(in_progress_id);
    in_progress.started_at = Local::now() - Duration::try_days(60).unwrap();
    in_progress.audio_path = "/tmp/active.wav".into();
    catalog.insert(&in_progress).await.unwrap();
    catalog
        .update_status(&in_progress.id, RecordingStatus::Transcribing)
        .await
        .unwrap();

    let deleted = catalog
        .apply_retention(&RetentionConfig {
            max_age_days: Some(1),
            max_count: None,
            delete_audio: false,
        })
        .await
        .unwrap();

    assert!(
        deleted.is_empty(),
        "in-progress recording should never be deleted by retention"
    );
    assert!(catalog.get(&in_progress.id).await.unwrap().is_some());
}

#[tokio::test]
async fn apply_retention_noop_when_both_limits_are_none() {
    let (_dir, catalog) = fresh_catalog().await;
    insert_done_recording_aged(&catalog, 999).await;

    let deleted = catalog
        .apply_retention(&RetentionConfig {
            max_age_days: None,
            max_count: None,
            delete_audio: false,
        })
        .await
        .unwrap();

    assert!(
        deleted.is_empty(),
        "no-policy retention must never delete anything"
    );
}

// ── Tag cascade ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn deleting_recording_cascades_to_recording_tags() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("foo", None).await.unwrap();
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog.attach_tag(&rec.id, tag.id).await.unwrap();
    catalog.delete(&rec.id).await.unwrap();
    let still_tagged = catalog.tags_for(&rec.id).await.unwrap();
    assert!(still_tagged.is_empty());
}

#[tokio::test]
async fn deleting_tag_cascades_to_recording_tags() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog
        .add_tag("soon-deleted", Some("#ff0000"))
        .await
        .unwrap();
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog.attach_tag(&rec.id, tag.id).await.unwrap();

    // Confirm the tag is attached.
    assert_eq!(catalog.tags_for(&rec.id).await.unwrap().len(), 1);

    // Delete the tag itself — the junction row must vanish too.
    catalog.delete_tag(tag.id).await.unwrap();

    let after = catalog.tags_for(&rec.id).await.unwrap();
    assert!(
        after.is_empty(),
        "delete_tag should cascade to recording_tags"
    );

    // Tag must be gone from list_all_tags as well.
    let all = catalog.list_all_tags().await.unwrap();
    assert!(!all.iter().any(|t| t.id == tag.id));
}

// ── Tag CRUD edge cases ───────────────────────────────────────────────────────

#[tokio::test]
async fn add_tag_without_color_stores_null() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("no-color", None).await.unwrap();
    assert_eq!(tag.name, "no-color");
    assert!(
        tag.color.is_none(),
        "color should be None when not supplied"
    );

    let all = catalog.list_all_tags().await.unwrap();
    let found = all
        .iter()
        .find(|t| t.id == tag.id)
        .expect("tag must appear in list_all_tags");
    assert!(found.color.is_none());
}

#[tokio::test]
async fn attach_tag_is_idempotent() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("idempotent", None).await.unwrap();
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();

    catalog.attach_tag(&rec.id, tag.id).await.unwrap();
    // Attaching a second time must not error (INSERT OR IGNORE) and must not create a duplicate.
    catalog.attach_tag(&rec.id, tag.id).await.unwrap();

    let tags = catalog.tags_for(&rec.id).await.unwrap();
    assert_eq!(
        tags.len(),
        1,
        "duplicate attach should not create a second row"
    );
}

#[tokio::test]
async fn tags_for_is_scoped_to_the_queried_recording() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag_a = catalog.add_tag("alpha", None).await.unwrap();
    let tag_b = catalog.add_tag("beta", None).await.unwrap();
    let r1 = sample_recording(RecordingId::new());
    let r2 = sample_recording(RecordingId::new());
    catalog.insert(&r1).await.unwrap();
    catalog.insert(&r2).await.unwrap();
    catalog.attach_tag(&r1.id, tag_a.id).await.unwrap();
    catalog.attach_tag(&r2.id, tag_b.id).await.unwrap();

    let tags1 = catalog.tags_for(&r1.id).await.unwrap();
    assert_eq!(tags1.len(), 1);
    assert_eq!(tags1[0].id, tag_a.id, "r1 should only carry tag_a");

    let tags2 = catalog.tags_for(&r2.id).await.unwrap();
    assert_eq!(tags2.len(), 1);
    assert_eq!(tags2[0].id, tag_b.id, "r2 should only carry tag_b");
}

// ── Tag filter in list() ──────────────────────────────────────────────────────

#[tokio::test]
async fn list_filters_by_tag_id() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("rust", None).await.unwrap();
    let r1 = sample_recording(RecordingId::new());
    let r2 = sample_recording(RecordingId::new());
    catalog.insert(&r1).await.unwrap();
    catalog.insert(&r2).await.unwrap();
    catalog.attach_tag(&r1.id, tag.id).await.unwrap();
    // r2 is intentionally untagged.

    let results = catalog
        .list(&ListFilter {
            tag_id: Some(tag.id),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(
        results.len(),
        1,
        "only the tagged recording should be returned"
    );
    assert_eq!(results[0].id, r1.id);
}

#[tokio::test]
async fn list_with_tag_filter_returns_empty_for_unused_tag() {
    let (_dir, catalog) = fresh_catalog().await;
    let tag = catalog.add_tag("orphan", None).await.unwrap();
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    // Tag exists but is not attached to any recording.

    let results = catalog
        .list(&ListFilter {
            tag_id: Some(tag.id),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(
        results.is_empty(),
        "unattached tag should match no recordings"
    );
}

// ── Transcript history (original_transcript) ──────────────────────────────────

/// Machine transcription stores the output in both columns so the original
/// is preserved for "View original" even before any user edits.
#[tokio::test]
async fn machine_transcription_sets_original_transcript() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_transcript(&rec.id, "hello world", "hello world", "m")
        .await
        .unwrap();

    let original = catalog.get_original_transcript(&rec.id).await.unwrap();
    assert_eq!(
        original.as_deref(),
        Some("hello world"),
        "original_transcript must be set by machine transcription"
    );
}

/// A user edit must update only the live `transcript` column, leaving
/// `original_transcript` untouched so the user can still revert.
#[tokio::test]
async fn user_edit_preserves_original_transcript() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();

    // Machine transcription sets both.
    catalog
        .update_transcript(&rec.id, "raw whisper output", "raw whisper output", "m")
        .await
        .unwrap();

    // User edits the live transcript.
    catalog
        .update_user_transcript(&rec.id, "edited by user")
        .await
        .unwrap();

    let got = catalog.get(&rec.id).await.unwrap().unwrap();
    assert_eq!(
        got.transcript.as_deref(),
        Some("edited by user"),
        "live transcript must reflect the user edit"
    );
    assert!(
        got.user_edited,
        "user_edited flag must be set after a hand edit"
    );
    assert_eq!(
        got.model.as_deref(),
        Some("m"),
        "model must keep the transcription model, not be clobbered by the edit"
    );

    let original = catalog.get_original_transcript(&rec.id).await.unwrap();
    assert_eq!(
        original.as_deref(),
        Some("raw whisper output"),
        "original_transcript must not be touched by user edits"
    );
}

/// The "unedited" (clean) transcript snapshots the pipeline output and must
/// survive user edits, so "View unedited transcript" shows transcribed+cleaned
/// text even after the user changes the live transcript.
#[tokio::test]
async fn user_edit_preserves_clean_transcript() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();

    // Pipeline: raw machine output → original_transcript, cleaned → transcript +
    // clean_transcript.
    catalog
        .update_transcript(
            &rec.id,
            "We talked about the thing.",
            "um so like the thing",
            "m",
        )
        .await
        .unwrap();

    // User edits the live transcript.
    catalog
        .update_user_transcript(&rec.id, "We discussed the proposal.")
        .await
        .unwrap();

    // Raw (pre-cleanup) version untouched.
    assert_eq!(
        catalog
            .get_original_transcript(&rec.id)
            .await
            .unwrap()
            .as_deref(),
        Some("um so like the thing"),
    );
    // Unedited (cleaned, pre-edit) version untouched.
    assert_eq!(
        catalog
            .get_clean_transcript(&rec.id)
            .await
            .unwrap()
            .as_deref(),
        Some("We talked about the thing."),
        "clean_transcript must survive user edits",
    );
    // Live transcript reflects the user edit.
    assert_eq!(
        catalog
            .get(&rec.id)
            .await
            .unwrap()
            .unwrap()
            .transcript
            .as_deref(),
        Some("We discussed the proposal."),
    );
}

/// When LLM post-processing is active the pipeline stores the LLM-cleaned
/// text as `transcript` but the raw Whisper output as `original_transcript`.
/// This test simulates that by passing different values to `update_transcript`.
#[tokio::test]
async fn llm_post_processing_does_not_overwrite_original_transcript() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();

    let raw = "um so like yeah we uh talked about the thing";
    let llm_cleaned = "We talked about the thing.";

    // Pipeline passes (llm_output, raw_whisper) to update_transcript.
    catalog
        .update_transcript(&rec.id, llm_cleaned, raw, "whisper-base")
        .await
        .unwrap();

    let got = catalog.get(&rec.id).await.unwrap().unwrap();
    assert_eq!(
        got.transcript.as_deref(),
        Some(llm_cleaned),
        "live transcript must be the LLM-cleaned version"
    );

    let original = catalog.get_original_transcript(&rec.id).await.unwrap();
    assert_eq!(
        original.as_deref(),
        Some(raw),
        "original_transcript must be the raw Whisper output, not the LLM output"
    );
}

// ── Session grouping (Meeting Mode, v1.6) ─────────────────────────────────────

/// Build a recording belonging to a meeting session/track at a given time.
fn meeting_track(started: chrono::DateTime<Local>, meeting_id: &str, track: &str) -> Recording {
    let mut rec = sample_recording(RecordingId::from_datetime(started));
    rec.started_at = started;
    rec.meeting_id = Some(meeting_id.to_string());
    rec.track = Some(track.to_string());
    rec.audio_path = format!("/tmp/{}-{track}.wav", meeting_id);
    rec
}

#[tokio::test]
async fn list_by_meeting_returns_both_tracks_ordered_by_track() {
    let (_dir, catalog) = fresh_catalog().await;
    let start = Local.with_ymd_and_hms(2026, 5, 19, 14, 0, 0).unwrap();

    // Insert "system" first to prove ordering is by `track`, not insert order.
    let system = meeting_track(start, "sess-1", "system");
    let mic = meeting_track(start, "sess-1", "mic");
    catalog.insert(&system).await.unwrap();
    catalog.insert(&mic).await.unwrap();

    // A standalone recording with no session must not leak into the session.
    let solo = sample_recording(RecordingId::new());
    catalog.insert(&solo).await.unwrap();

    let rows = catalog.list_by_meeting("sess-1").await.unwrap();
    assert_eq!(rows.len(), 2, "exactly the two meeting tracks come back");
    // "mic" < "system" lexicographically, so mic is first.
    assert_eq!(rows[0].track.as_deref(), Some("mic"));
    assert_eq!(rows[1].track.as_deref(), Some("system"));
    assert!(
        rows.iter()
            .all(|r| r.meeting_id.as_deref() == Some("sess-1")),
        "every row must belong to the queried session"
    );
}

#[tokio::test]
async fn list_by_meeting_unknown_session_returns_empty() {
    let (_dir, catalog) = fresh_catalog().await;
    // A standalone (NULL session) recording exists, but no meeting.
    let solo = sample_recording(RecordingId::new());
    catalog.insert(&solo).await.unwrap();

    let rows = catalog.list_by_meeting("no-such-session").await.unwrap();
    assert!(
        rows.is_empty(),
        "querying a session id with no rows yields an empty vec, not an error"
    );
}

#[tokio::test]
async fn list_by_meeting_isolates_distinct_sessions() {
    let (_dir, catalog) = fresh_catalog().await;
    let start = Local.with_ymd_and_hms(2026, 5, 19, 14, 0, 0).unwrap();
    for track in ["mic", "system"] {
        catalog
            .insert(&meeting_track(start, "sess-A", track))
            .await
            .unwrap();
        catalog
            .insert(&meeting_track(start, "sess-B", track))
            .await
            .unwrap();
    }

    let a = catalog.list_by_meeting("sess-A").await.unwrap();
    let b = catalog.list_by_meeting("sess-B").await.unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(b.len(), 2);
    assert!(a.iter().all(|r| r.meeting_id.as_deref() == Some("sess-A")));
    assert!(b.iter().all(|r| r.meeting_id.as_deref() == Some("sess-B")));
}

#[tokio::test]
async fn semantic_search_thresholds_dim_checks_and_dedupes_meetings() {
    let (_dir, catalog) = fresh_catalog().await;

    // Insert a recording (optionally part of a meeting) plus its embedding.
    async fn add(catalog: &Catalog, id: &str, meeting: Option<&str>, vec: &[f32]) -> RecordingId {
        let rid = RecordingId::parse(id).unwrap();
        let mut rec = sample_recording(rid.clone());
        rec.meeting_id = meeting.map(|m| m.to_string());
        catalog.insert(&rec).await.unwrap();
        catalog.upsert_embedding(&rid, vec).await.unwrap();
        rid
    }

    // Query along the x-axis. `cosine_similarity` is a bare dot product (inputs
    // are assumed L2-normalized in production), which is fine for these fixtures.
    let q = [1.0f32, 0.0, 0.0];
    let _standalone = add(&catalog, "20260519T100000001", None, &[0.99, 0.1, 0.0]).await; // ~0.99
                                                                                          // Meeting: two tracks share a meeting_id; only the best should survive.
    let _mic = add(
        &catalog,
        "20260519T100000002",
        Some("meeting-x"),
        &[0.90, 0.2, 0.0],
    )
    .await; // 0.90
    let sys = add(
        &catalog,
        "20260519T100000003",
        Some("meeting-x"),
        &[0.95, 0.05, 0.0],
    )
    .await; // 0.95
            // Orthogonal → below threshold, must be dropped.
    let _ortho = add(&catalog, "20260519T100000004", None, &[0.0, 0.0, 1.0]).await; // 0.0
                                                                                    // Wrong dimension → skipped, never panics.
    let _baddim = add(&catalog, "20260519T100000005", None, &[1.0, 0.0]).await;

    let results = catalog.semantic_search(&q, 10, 0.2).await.unwrap();

    // Orthogonal (below floor) + wrong-dim excluded; meeting collapses to one.
    assert_eq!(results.len(), 2, "results: {results:?}");

    let ids: Vec<String> = results
        .iter()
        .map(|(id, _)| id.as_str().to_string())
        .collect();
    let meeting_hits = ids
        .iter()
        .filter(|i| i.as_str() == "20260519T100000002" || i.as_str() == "20260519T100000003")
        .count();
    assert_eq!(
        meeting_hits, 1,
        "meeting tracks must dedupe to a single result"
    );
    assert!(
        ids.contains(&sys.as_str().to_string()),
        "the better-scoring meeting track should win"
    );
    assert!(
        results[0].1 >= results[1].1,
        "results must be sorted descending"
    );
}

#[tokio::test]
async fn saved_searches_upsert_list_and_delete() {
    let (_dir, catalog) = fresh_catalog().await;

    // Empty to start.
    assert!(catalog.list_saved_searches().await.unwrap().is_empty());

    // Insert two; both come back.
    catalog
        .upsert_saved_search("ss_a", "Meetings", r#"{"kind":"meeting"}"#)
        .await
        .unwrap();
    catalog
        .upsert_saved_search("ss_b", "Failed", r#"{"status":"failed"}"#)
        .await
        .unwrap();
    let list = catalog.list_saved_searches().await.unwrap();
    assert_eq!(list.len(), 2);

    // Upsert by the same id updates in place (no duplicate row).
    catalog
        .upsert_saved_search("ss_a", "Meetings (renamed)", r#"{"kind":"meeting","sort_desc":false}"#)
        .await
        .unwrap();
    let list = catalog.list_saved_searches().await.unwrap();
    assert_eq!(list.len(), 2, "upsert by id must not create a duplicate");
    let a = list.iter().find(|s| s.id == "ss_a").expect("ss_a present");
    assert_eq!(a.name, "Meetings (renamed)");
    assert_eq!(a.filter_json, r#"{"kind":"meeting","sort_desc":false}"#);

    // Delete one; the other remains, and a second delete of the same id is a no-op.
    assert!(catalog.delete_saved_search("ss_a").await.unwrap());
    assert!(!catalog.delete_saved_search("ss_a").await.unwrap());
    let list = catalog.list_saved_searches().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "ss_b");
}

#[tokio::test]
async fn voiceprints_enroll_recognize_merge_and_forget() {
    let (_dir, catalog) = fresh_catalog().await;
    // Voiceprints have an ON DELETE CASCADE FK to recordings, so each capture's
    // recording must exist (it always does in production — captured at transcribe).
    let r1 = RecordingId::new();
    let r2 = RecordingId::new();
    let r3 = RecordingId::new();
    for r in [&r1, &r2, &r3] {
        catalog.insert(&sample_recording((*r).clone())).await.unwrap();
    }

    // Empty library + an un-enrolled capture recognizes nothing.
    assert!(catalog.list_named_voices().await.unwrap().is_empty());
    catalog
        .save_speaker_voiceprint(r1.as_str(), 1, &[1.0, 0.0, 0.0])
        .await
        .unwrap();
    assert!(catalog
        .recognize_voice(&[1.0, 0.0, 0.0], 0.8)
        .await
        .unwrap()
        .is_none());

    // Enrolling that capture under a name makes a future close centroid match it.
    let id = catalog
        .enroll_speaker(r1.as_str(), 1, "Alice")
        .await
        .unwrap()
        .expect("enrolled");
    let (voice, score) = catalog
        .recognize_voice(&[0.98, 0.02, 0.0], 0.8)
        .await
        .unwrap()
        .expect("recognized");
    assert_eq!(voice.name, "Alice");
    assert_eq!(voice.id, id);
    assert!(score > 0.9);
    // An orthogonal voice is not Alice.
    assert!(catalog
        .recognize_voice(&[0.0, 0.0, 1.0], 0.8)
        .await
        .unwrap()
        .is_none());

    // A second sample for Alice (different recording) updates the running mean
    // and sample count; naming by the same name reuses the entry.
    catalog
        .save_speaker_voiceprint(r2.as_str(), 1, &[0.0, 1.0, 0.0])
        .await
        .unwrap();
    let id2 = catalog
        .enroll_speaker(r2.as_str(), 1, "alice") // case-insensitive → same voice
        .await
        .unwrap()
        .expect("enrolled");
    assert_eq!(id2, id);
    let voices = catalog.list_named_voices().await.unwrap();
    assert_eq!(voices.len(), 1);
    assert_eq!(voices[0].samples, 2);

    // A separate voice, then merge it into Alice: captures re-point, entry drops.
    catalog
        .save_speaker_voiceprint(r3.as_str(), 1, &[0.0, 0.0, 1.0])
        .await
        .unwrap();
    let other = catalog
        .enroll_speaker(r3.as_str(), 1, "Bob")
        .await
        .unwrap()
        .expect("enrolled");
    assert_eq!(catalog.list_named_voices().await.unwrap().len(), 2);
    assert!(catalog.merge_named_voices(&other, &id).await.unwrap());
    let voices = catalog.list_named_voices().await.unwrap();
    assert_eq!(voices.len(), 1);
    assert_eq!(voices[0].samples, 3);

    // Forgetting Alice empties the library but keeps the raw captures.
    assert!(catalog.forget_named_voice(&id).await.unwrap());
    assert!(catalog.list_named_voices().await.unwrap().is_empty());
    assert!(catalog
        .recognize_voice(&[0.98, 0.02, 0.0], 0.8)
        .await
        .unwrap()
        .is_none());
    assert!(catalog
        .speaker_voiceprint(r1.as_str(), 1)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn recognize_speakers_for_skips_named_and_dismissed() {
    let (_dir, catalog) = fresh_catalog().await;
    let r1 = RecordingId::new();
    let r2 = RecordingId::new();
    // Both must exist: set_speaker_name + the voiceprint cascade FK to recordings.
    catalog.insert(&sample_recording(r1.clone())).await.unwrap();
    catalog.insert(&sample_recording(r2.clone())).await.unwrap();

    // Enroll Alice from one recording.
    catalog
        .save_speaker_voiceprint(r1.as_str(), 1, &[1.0, 0.0, 0.0])
        .await
        .unwrap();
    catalog.enroll_speaker(r1.as_str(), 1, "Alice").await.unwrap();

    // A new recording: speaker 1 sounds like Alice, speaker 2 is someone else.
    catalog
        .save_speaker_voiceprint(r2.as_str(), 1, &[0.97, 0.03, 0.0])
        .await
        .unwrap();
    catalog
        .save_speaker_voiceprint(r2.as_str(), 2, &[0.0, 0.0, 1.0])
        .await
        .unwrap();
    let sugg = catalog
        .recognize_speakers_for(r2.as_str(), 0.5, phoneme_core::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert_eq!(sugg.len(), 1, "only speaker 1 matches Alice");
    assert_eq!(sugg[0].speaker_label, 1);
    assert_eq!(sugg[0].name, "Alice");
    assert!(sugg[0].score > 0.9);

    // Naming speaker 1 stops suggesting over it.
    catalog.set_speaker_name(&r2, 1, "Alice").await.unwrap();
    assert!(catalog
        .recognize_speakers_for(r2.as_str(), 0.5, phoneme_core::voiceprint::ScoreNorm::Off)
        .await
        .unwrap()
        .is_empty());

    // A third speaker also matching Alice IS suggested — until dismissed.
    catalog
        .save_speaker_voiceprint(r2.as_str(), 3, &[0.96, 0.04, 0.0])
        .await
        .unwrap();
    let sugg = catalog
        .recognize_speakers_for(r2.as_str(), 0.5, phoneme_core::voiceprint::ScoreNorm::Off)
        .await
        .unwrap();
    assert_eq!(sugg.len(), 1);
    assert_eq!(sugg[0].speaker_label, 3);
    catalog.dismiss_speaker_suggestion(r2.as_str(), 3).await.unwrap();
    assert!(catalog
        .recognize_speakers_for(r2.as_str(), 0.5, phoneme_core::voiceprint::ScoreNorm::Off)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn deleting_a_recording_cascades_voiceprints_and_recomputes_named_voices() {
    let (_dir, catalog) = fresh_catalog().await;
    let r1 = RecordingId::new();
    let r2 = RecordingId::new();
    catalog.insert(&sample_recording(r1.clone())).await.unwrap();
    catalog.insert(&sample_recording(r2.clone())).await.unwrap();

    // "Alice" enrolled from two recordings → 2 samples.
    catalog.save_speaker_voiceprint(r1.as_str(), 1, &[1.0, 0.0]).await.unwrap();
    catalog.save_speaker_voiceprint(r2.as_str(), 1, &[0.0, 1.0]).await.unwrap();
    catalog.enroll_speaker(r1.as_str(), 1, "Alice").await.unwrap();
    catalog.enroll_speaker(r2.as_str(), 1, "Alice").await.unwrap();
    catalog.dismiss_speaker_suggestion(r1.as_str(), 2).await.unwrap();
    assert_eq!(catalog.list_named_voices().await.unwrap()[0].samples, 2);

    // Delete r1 → its voiceprint + dismissal cascade away; Alice recomputed to 1.
    catalog.delete(&r1).await.unwrap();
    assert!(
        catalog.speaker_voiceprint(r1.as_str(), 1).await.unwrap().is_none(),
        "the deleted recording's voiceprint must cascade away"
    );
    let voices = catalog.list_named_voices().await.unwrap();
    assert_eq!(voices.len(), 1);
    assert_eq!(voices[0].samples, 1, "Alice recomputed after losing r1's sample");
}

#[tokio::test]
async fn renaming_a_speaker_recomputes_the_previously_linked_voice() {
    let (_dir, catalog) = fresh_catalog().await;
    let r = RecordingId::new();
    catalog.insert(&sample_recording(r.clone())).await.unwrap();
    catalog.save_speaker_voiceprint(r.as_str(), 1, &[1.0, 0.0]).await.unwrap();

    // Enroll as Alice, then re-name (the corrected-a-wrong-suggestion case) to Bob.
    catalog.enroll_speaker(r.as_str(), 1, "Alice").await.unwrap();
    catalog.enroll_speaker(r.as_str(), 1, "Bob").await.unwrap();

    let voices = catalog.list_named_voices().await.unwrap();
    let alice = voices.iter().find(|v| v.name == "Alice").expect("Alice present");
    let bob = voices.iter().find(|v| v.name == "Bob").expect("Bob present");
    assert_eq!(alice.samples, 0, "Alice recomputed to 0 after the sample moved to Bob");
    assert_eq!(bob.samples, 1);
}
