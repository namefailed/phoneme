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

    let original = catalog
        .get_original_transcript(&rec.id)
        .await
        .unwrap();
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
    assert_eq!(
        got.model.as_deref(),
        Some("user-edit"),
        "model must be set to 'user-edit'"
    );

    let original = catalog
        .get_original_transcript(&rec.id)
        .await
        .unwrap();
    assert_eq!(
        original.as_deref(),
        Some("raw whisper output"),
        "original_transcript must not be touched by user edits"
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
