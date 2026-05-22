use chrono::{Local, TimeZone};
use phoneme_core::{Catalog, ListFilter, Recording, RecordingId, RecordingStatus};
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
        .update_transcript(&rec.id, "hello world", "gemma-4-E4B")
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
        .update_transcript(&rec.id, "remind me to email Sarah about the contract", "m")
        .await
        .unwrap();
    let hits = catalog.search("sarah").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, rec.id);
    let miss = catalog.search("nonexistent").await.unwrap();
    assert!(miss.is_empty());
}

#[tokio::test]
async fn delete_removes_recording_and_fts_row() {
    let (_dir, catalog) = fresh_catalog().await;
    let rec = sample_recording(RecordingId::new());
    catalog.insert(&rec).await.unwrap();
    catalog
        .update_transcript(&rec.id, "deletable", "m")
        .await
        .unwrap();
    catalog.delete(&rec.id).await.unwrap();
    assert!(catalog.get(&rec.id).await.unwrap().is_none());
    assert!(catalog.search("deletable").await.unwrap().is_empty());
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
