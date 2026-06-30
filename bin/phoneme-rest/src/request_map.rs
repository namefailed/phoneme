//! Pure route → [`Request`] mapping.
//!
//! Every REST endpoint is a thin translation of its path/query parameters into
//! exactly one [`phoneme_ipc::Request`], which the handler then forwards to the
//! daemon verbatim. Keeping that translation in free functions here (no axum
//! types, no live connection) means the dispatch logic — the part most likely
//! to drift from the schema — is unit-tested in isolation: a test asserts the
//! built `Request`, not a round-trip through a mock daemon.
//!
//! The query-parameter shapes used by the list/search handlers live here too as
//! `Deserialize` structs, so axum's `Query` extractor produces them directly.

use phoneme_core::{ListFilter, ListKind, RecordMode, RecordingId, RecordingStatus};
use phoneme_ipc::Request;
use serde::Deserialize;

/// Query parameters for `GET /api/recordings`. Mirrors the most-used facets of
/// the IPC [`ListFilter`] so REST clients can filter as richly as the GUI.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListQuery {
    /// Maximum rows to return (maps to [`ListFilter::limit`]).
    pub limit: Option<u32>,
    /// Rows to skip before returning results (maps to [`ListFilter::offset`]).
    pub offset: Option<u32>,
    /// Recording-type filter: `single` or `meeting` (maps to
    /// [`ListFilter::kind`]). Any other value is ignored (treated as "all").
    pub kind: Option<String>,
    /// Full-text search over transcripts (maps to [`ListFilter::search`]).
    pub text: Option<String>,
    /// Restrict to recordings carrying this tag id (maps to [`ListFilter::tag_id`]).
    pub tag_id: Option<i64>,
    /// Status filter, e.g. `done` / `transcribe_failed` (maps to
    /// [`ListFilter::status`]); an unrecognized value is ignored.
    pub status: Option<String>,
    /// `true` = favorites only, `false` = non-favorites (maps to [`ListFilter::favorite`]).
    pub favorite: Option<bool>,
    /// `true` = pinned only, `false` = non-pinned (maps to [`ListFilter::pinned`]).
    pub pinned: Option<bool>,
    /// `true` = dictation (in-place) only (maps to [`ListFilter::in_place`]).
    pub in_place: Option<bool>,
    /// `true` = tagged only, `false` = untagged only (maps to [`ListFilter::tagged`]).
    pub tagged: Option<bool>,
    /// RFC-3339 lower bound on `started_at` (maps to [`ListFilter::since`]).
    pub since: Option<String>,
    /// RFC-3339 upper bound on `started_at` (maps to [`ListFilter::until`]).
    pub until: Option<String>,
    /// Sort newest-first (`true`, the default) or oldest-first (`false`).
    pub sort_desc: Option<bool>,
}

/// Query parameters for the timeline endpoints (`/segments`, `/words`). Selects
/// which transcript timeline to read: raw (default) or `cleaned` (re-aligned
/// after post-processing).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VariantQuery {
    /// `cleaned` = the post-cleanup re-aligned timeline; absent / anything else
    /// = the raw machine transcript.
    pub variant: Option<String>,
}

/// Query parameters for `GET /api/search`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchQuery {
    /// The natural-language query (`q`).
    #[serde(default)]
    pub q: String,
    /// Maximum number of results. Defaults to [`DEFAULT_SEARCH_LIMIT`].
    pub limit: Option<usize>,
}

/// Fallback result count for `GET /api/search` when `limit` is omitted.
pub const DEFAULT_SEARCH_LIMIT: usize = 20;

/// Parse the `kind` query value into a [`ListKind`]. Unknown / absent values
/// yield `None` ("all kinds"), matching the daemon's filter semantics.
fn parse_kind(kind: Option<&str>) -> Option<ListKind> {
    match kind.map(str::trim) {
        Some("single") => Some(ListKind::Single),
        Some("meeting") => Some(ListKind::Meeting),
        _ => None,
    }
}

/// Parse an RFC-3339 timestamp into the daemon's `Local` clock, or `None`.
fn parse_dt(s: Option<&str>) -> Option<chrono::DateTime<chrono::Local>> {
    s.and_then(|raw| {
        chrono::DateTime::parse_from_rfc3339(raw.trim())
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Local))
    })
}

/// `GET /api/recordings` → [`Request::ListRecordings`]. Threads the full facet
/// set so REST clients can filter by text / tag / status / favorite / pinned /
/// date range, not just limit/offset/kind.
pub fn list_recordings(q: &ListQuery) -> Request {
    Request::ListRecordings {
        filter: ListFilter {
            limit: q.limit,
            offset: q.offset,
            kind: parse_kind(q.kind.as_deref()),
            search: q.text.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
            tag_id: q.tag_id,
            status: q.status.as_deref().and_then(RecordingStatus::from_str_opt),
            favorite: q.favorite,
            pinned: q.pinned,
            in_place: q.in_place,
            tagged: q.tagged,
            since: parse_dt(q.since.as_deref()),
            until: parse_dt(q.until.as_deref()),
            sort_desc: q.sort_desc,
            ..ListFilter::default()
        },
    }
}

/// Validate a `:id` path segment into a [`RecordingId`].
///
/// Returns `None` for anything that isn't the canonical 18-char id shape, so
/// the caller can answer `400 Bad Request` instead of forwarding a malformed id
/// that the daemon would reject (and that would panic the fixed-offset id
/// accessors elsewhere).
pub fn parse_id(raw: &str) -> Option<RecordingId> {
    RecordingId::parse(raw)
}

/// `GET /api/recordings/:id` → [`Request::GetRecording`].
pub fn get_recording(id: RecordingId) -> Request {
    Request::GetRecording { id }
}

/// Normalize the `variant` query value: `cleaned` selects the re-aligned
/// post-cleanup timeline; anything else (incl. absent) reads the raw transcript.
fn parse_variant(v: Option<&str>) -> Option<String> {
    match v.map(str::trim) {
        Some("cleaned") => Some("cleaned".to_string()),
        _ => None,
    }
}

/// `GET /api/recordings/:id/segments` → [`Request::GetSegments`]. The optional
/// `?variant=cleaned` selects the re-aligned post-cleanup timeline.
pub fn get_segments(id: RecordingId, variant: Option<&str>) -> Request {
    Request::GetSegments {
        id,
        variant: parse_variant(variant),
    }
}

/// `GET /api/recordings/:id/words` → [`Request::GetWords`]. The optional
/// `?variant=cleaned` selects the re-aligned post-cleanup timeline.
pub fn get_words(id: RecordingId, variant: Option<&str>) -> Request {
    Request::GetWords {
        id,
        variant: parse_variant(variant),
    }
}

/// `GET /api/recordings/:id/chapters` → [`Request::GetChapters`].
pub fn get_chapters(id: RecordingId) -> Request {
    Request::GetChapters { id }
}

/// `GET /api/recordings/:id/versions` → [`Request::ListTranscriptVersions`]. The
/// compounding-transcript chain (raw ASR → each step → live) for side-by-side
/// compare; an HTTP alternative to the pipe-only access a client would otherwise
/// need (cross-platform, no named-pipe path).
pub fn transcript_versions(id: RecordingId) -> Request {
    Request::ListTranscriptVersions { id }
}

/// JSON body for `POST /api/recordings/:id/clip`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClipBody {
    /// Start of the range, in milliseconds from the recording's start.
    pub start_ms: i64,
    /// End of the range, in milliseconds (exclusive; clamped to the duration).
    pub end_ms: i64,
    /// Absolute output path for the new WAV; absent/empty = next to the source
    /// with a `_clip_<start>-<end>` suffix.
    #[serde(default)]
    pub out_path: Option<String>,
}

/// `POST /api/recordings/:id/clip` → [`Request::ExportClip`].
pub fn export_clip(id: RecordingId, body: &ClipBody) -> Request {
    Request::ExportClip {
        id,
        start_ms: body.start_ms,
        end_ms: body.end_ms,
        out_path: body.out_path.clone(),
    }
}

/// `GET /api/search` → [`Request::SemanticSearch`].
pub fn search(q: &SearchQuery) -> Request {
    Request::SemanticSearch {
        query: q.q.clone(),
        limit: q.limit.unwrap_or(DEFAULT_SEARCH_LIMIT),
        // The REST search endpoint stays unscoped for now; the S3 filter is a
        // pipe-level addition the daemon honors when present.
        filter: None,
    }
}

/// Query parameters for `GET /api/recordings/:id/similar` ("more like this").
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SimilarQuery {
    /// Maximum number of results. Defaults to [`DEFAULT_SEARCH_LIMIT`].
    pub limit: Option<usize>,
}

/// `GET /api/recordings/:id/similar` → [`Request::MoreLikeThis`].
pub fn more_like_this(id: RecordingId, q: &SimilarQuery) -> Request {
    Request::MoreLikeThis {
        id,
        limit: q.limit.unwrap_or(DEFAULT_SEARCH_LIMIT),
    }
}

/// `GET /api/tags` → [`Request::ListTags`] (tags attached to ≥1 recording).
pub fn list_tags() -> Request {
    Request::ListTags
}

/// `GET /api/recordings/:id/tags` → [`Request::TagsFor`].
pub fn tags_for(id: RecordingId) -> Request {
    Request::TagsFor { recording_id: id }
}

/// `GET /api/queue` → [`Request::ListQueue`].
pub fn list_queue() -> Request {
    Request::ListQueue
}

/// JSON body for `POST /api/recordings/:id/title`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TitleBody {
    /// The user's title, or `null`/absent to clear it back to auto-generation.
    #[serde(default)]
    pub title: Option<String>,
}

/// `POST /api/recordings/:id/title` → [`Request::SetRecordingTitle`].
pub fn set_title(id: RecordingId, body: &TitleBody) -> Request {
    Request::SetRecordingTitle {
        id,
        title: body.title.clone(),
    }
}

/// JSON body for `POST /api/recordings/:id/favorite`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FavoriteBody {
    /// `true` = starred. Deliberately lenient (`#[serde(default)]`): an omitted
    /// field decodes to `false` rather than erroring. Making it required was tried
    /// and reverted — a missing field then produced an axum `JsonRejection` (422,
    /// plain-text) that bypasses the uniform `{"error":…}` envelope every other
    /// endpoint returns, a worse inconsistency than the minor "`{}` unstars". The
    /// frontend always sends the field, so the lenient default is never hit there.
    #[serde(default)]
    pub favorite: bool,
}

/// `POST /api/recordings/:id/favorite` → [`Request::SetFavorite`].
pub fn set_favorite(id: RecordingId, body: &FavoriteBody) -> Request {
    Request::SetFavorite {
        id,
        favorite: body.favorite,
    }
}

/// JSON body for `POST /api/recordings/:id/pinned`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PinnedBody {
    /// `true` = pinned. Lenient (`#[serde(default)]`) for the same reason as
    /// [`FavoriteBody::favorite`]: an omitted field decodes to `false` rather
    /// than erroring out of the uniform `{"error":…}` envelope.
    #[serde(default)]
    pub pinned: bool,
}

/// `POST /api/recordings/:id/pinned` → [`Request::SetPinned`].
pub fn set_pinned(id: RecordingId, body: &PinnedBody) -> Request {
    Request::SetPinned {
        id,
        pinned: body.pinned,
    }
}

/// JSON body for `POST /api/recordings/:id/tags`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AttachTagBody {
    /// The tag's id to attach.
    pub tag_id: i64,
}

/// `POST /api/recordings/:id/tags` → [`Request::AttachTag`].
pub fn attach_tag(id: RecordingId, body: &AttachTagBody) -> Request {
    Request::AttachTag {
        recording_id: id,
        tag_id: body.tag_id,
    }
}

/// `DELETE /api/recordings/:id/tags/:tag_id` → [`Request::DetachTag`].
pub fn detach_tag(id: RecordingId, tag_id: i64) -> Request {
    Request::DetachTag {
        recording_id: id,
        tag_id,
    }
}

/// `POST /api/recordings/:id/cleanup` → [`Request::RerunCleanup`].
///
/// Re-runs only the LLM cleanup step against the stored original transcript,
/// using the configured `[llm_post_process]` connection. The REST surface does
/// not expose the per-run provider/model/prompt overrides — they all stay
/// `None` (the configured values), keeping the endpoint a plain "re-clean this".
pub fn rerun_cleanup(id: RecordingId) -> Request {
    Request::RerunCleanup {
        id,
        model: None,
        provider: None,
        prompt: None,
        api_url: None,
        api_key: None,
    }
}

/// `POST /api/recordings/:id/summary` → [`Request::RerunSummary`].
///
/// Generates (or regenerates) an LLM summary of the recording's current
/// transcript using the configured summary model/prompt; the per-run overrides
/// are not exposed over REST (both `None`).
pub fn rerun_summary(id: RecordingId) -> Request {
    Request::RerunSummary {
        id,
        model: None,
        prompt: None,
        provider: None,
        api_url: None,
        api_key: None,
    }
}

/// `POST /api/meeting/start` → [`Request::StartMeeting`].
pub fn meeting_start() -> Request {
    Request::StartMeeting
}

/// `POST /api/meeting/stop` → [`Request::StopMeeting`].
pub fn meeting_stop() -> Request {
    Request::StopMeeting
}

/// `POST /api/record/start` → [`Request::RecordStart`].
///
/// The REST surface always starts a `hold`-mode recording (stop is an explicit
/// `POST /api/record/stop`); dictation/in-place is not exposed over HTTP.
pub fn record_start() -> Request {
    Request::RecordStart {
        mode: RecordMode::Hold,
        in_place: false,
        recipe_id: None,
        whisper_model: None,
        source: None,
    }
}

/// `POST /api/record/stop` → [`Request::RecordStop`].
pub fn record_stop() -> Request {
    Request::RecordStop
}

/// JSON body for `POST /api/import`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImportBody {
    /// Absolute path to a local audio file (`.wav/.mp3/.m4a/.flac`). The daemon
    /// resolves it on its side, so a relative path from another process won't
    /// survive — send an absolute path. URL import (yt-dlp) is CLI-only.
    pub path: String,
    /// Optional one-time Playbook recipe override for this import (id or, the
    /// daemon's contract, an exact recipe id). Absent/empty ⇒ the `default`
    /// pipeline. A `scope = meeting` recipe is rejected by the daemon.
    #[serde(default)]
    pub recipe_id: Option<String>,
    /// Optional external-reference key for idempotent import: if a recording
    /// already carries it, the daemon returns that one (`{"id":…,"reused":true}`)
    /// instead of importing a duplicate. Absent ⇒ always a fresh import.
    #[serde(default)]
    pub ext_ref: Option<String>,
}

/// `POST /api/import` → [`Request::ImportRecording`].
pub fn import_recording(body: &ImportBody) -> Request {
    Request::ImportRecording {
        path: body.path.clone(),
        recipe_id: body.recipe_id.clone(),
        ext_ref: body.ext_ref.clone(),
    }
}

/// `GET /api/status` → [`Request::DaemonStatus`].
pub fn daemon_status() -> Request {
    Request::DaemonStatus
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_recordings_maps_limit_offset_kind() {
        let q = ListQuery {
            limit: Some(10),
            offset: Some(20),
            kind: Some("meeting".into()),
            ..ListQuery::default()
        };
        match list_recordings(&q) {
            Request::ListRecordings { filter } => {
                assert_eq!(filter.limit, Some(10));
                assert_eq!(filter.offset, Some(20));
                assert_eq!(filter.kind, Some(ListKind::Meeting));
            }
            other => panic!("expected ListRecordings, got {other:?}"),
        }
    }

    #[test]
    fn list_recordings_threads_the_full_facet_set() {
        let q = ListQuery {
            text: Some("budget".into()),
            tag_id: Some(7),
            status: Some("done".into()),
            favorite: Some(true),
            pinned: Some(false),
            in_place: Some(true),
            tagged: Some(true),
            since: Some("2026-01-01T00:00:00Z".into()),
            until: Some("2026-02-01T00:00:00Z".into()),
            sort_desc: Some(false),
            ..ListQuery::default()
        };
        // The two facets that go through real parsing must land on the *exact*
        // value, not merely Some(_): a wrong status variant or a shifted instant
        // would otherwise slip through.
        let expected_since = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Local);
        let expected_until = chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Local);
        match list_recordings(&q) {
            Request::ListRecordings { filter } => {
                assert_eq!(filter.search.as_deref(), Some("budget"));
                assert_eq!(filter.tag_id, Some(7));
                assert_eq!(filter.favorite, Some(true));
                assert_eq!(filter.pinned, Some(false));
                assert_eq!(filter.in_place, Some(true));
                assert_eq!(filter.tagged, Some(true));
                assert_eq!(filter.sort_desc, Some(false));
                // `status="done"` → RecordingStatus::Done (not just "some status").
                assert_eq!(filter.status, Some(RecordingStatus::Done));
                // `since`/`until` map to their own exact instants, and the two are
                // distinct — so swapping or dropping either mapping fails here.
                assert_eq!(filter.since, Some(expected_since));
                assert_eq!(filter.until, Some(expected_until));
                assert_ne!(filter.since, filter.until);
            }
            other => panic!("expected ListRecordings, got {other:?}"),
        }
    }

    #[test]
    fn list_recordings_drops_malformed_date_bounds() {
        // A non-RFC-3339 since/until must be ignored (parse_dt → None), never
        // error or forwarded as a bogus bound.
        let q = ListQuery {
            since: Some("not-a-date".into()),
            until: Some("2026-13-99T99:99:99Z".into()),
            ..ListQuery::default()
        };
        match list_recordings(&q) {
            Request::ListRecordings { filter } => {
                assert_eq!(filter.since, None);
                assert_eq!(filter.until, None);
            }
            other => panic!("expected ListRecordings, got {other:?}"),
        }
    }

    #[test]
    fn list_recordings_defaults_when_empty() {
        let q = ListQuery::default();
        match list_recordings(&q) {
            Request::ListRecordings { filter } => {
                assert_eq!(filter.limit, None);
                assert_eq!(filter.offset, None);
                assert_eq!(filter.kind, None);
            }
            other => panic!("expected ListRecordings, got {other:?}"),
        }
    }

    #[test]
    fn parse_kind_handles_single_meeting_and_unknown() {
        assert_eq!(parse_kind(Some("single")), Some(ListKind::Single));
        assert_eq!(parse_kind(Some("meeting")), Some(ListKind::Meeting));
        assert_eq!(parse_kind(Some("everything")), None);
        assert_eq!(parse_kind(None), None);
        // Whitespace is trimmed.
        assert_eq!(parse_kind(Some(" single ")), Some(ListKind::Single));
    }

    #[test]
    fn parse_id_accepts_canonical_and_rejects_garbage() {
        assert!(parse_id("20260519T143500042").is_some());
        assert!(parse_id("not-an-id").is_none());
        assert!(parse_id("20260519X143500042").is_none());
    }

    #[test]
    fn get_recording_and_segments_carry_the_id() {
        let id = parse_id("20260519T143500042").unwrap();
        match get_recording(id.clone()) {
            Request::GetRecording { id: got } => assert_eq!(got, id),
            other => panic!("expected GetRecording, got {other:?}"),
        }
        match get_segments(id.clone(), None) {
            Request::GetSegments {
                id: got,
                variant: None,
            } => assert_eq!(got, id),
            other => panic!("expected GetSegments, got {other:?}"),
        }
        // `?variant=cleaned` selects the re-aligned timeline.
        match get_segments(id.clone(), Some("cleaned")) {
            Request::GetSegments {
                variant: Some(v), ..
            } => assert_eq!(v, "cleaned"),
            other => panic!("expected cleaned GetSegments, got {other:?}"),
        }
        // Whitespace around `cleaned` is trimmed back to the canonical value.
        match get_segments(id.clone(), Some(" cleaned ")) {
            Request::GetSegments {
                variant: Some(v), ..
            } => assert_eq!(v, "cleaned"),
            other => panic!("expected trimmed-cleaned GetSegments, got {other:?}"),
        }
        // Any other variant string collapses to None (raw transcript), so a
        // bogus value never reaches the daemon as an invalid variant.
        match get_segments(id.clone(), Some("bogus")) {
            Request::GetSegments {
                variant: None,
                id: got,
            } => assert_eq!(got, id),
            other => panic!("expected raw (None) GetSegments, got {other:?}"),
        }
        match get_chapters(id.clone()) {
            Request::GetChapters { id: got } => assert_eq!(got, id),
            other => panic!("expected GetChapters, got {other:?}"),
        }
    }

    #[test]
    fn search_uses_default_limit_when_absent() {
        let q = SearchQuery {
            q: "hello".into(),
            limit: None,
        };
        match search(&q) {
            Request::SemanticSearch { query, limit, .. } => {
                assert_eq!(query, "hello");
                assert_eq!(limit, DEFAULT_SEARCH_LIMIT);
            }
            other => panic!("expected SemanticSearch, got {other:?}"),
        }
    }

    #[test]
    fn search_honors_explicit_limit() {
        let q = SearchQuery {
            q: "hello".into(),
            limit: Some(3),
        };
        match search(&q) {
            Request::SemanticSearch { limit, .. } => assert_eq!(limit, 3),
            other => panic!("expected SemanticSearch, got {other:?}"),
        }
    }

    #[test]
    fn record_start_is_hold_mode_not_in_place() {
        match record_start() {
            Request::RecordStart { mode, in_place, .. } => {
                assert_eq!(mode, RecordMode::Hold);
                assert!(!in_place);
            }
            other => panic!("expected RecordStart, got {other:?}"),
        }
    }

    #[test]
    fn record_stop_and_status_map_to_their_variants() {
        assert_eq!(record_stop(), Request::RecordStop);
        assert_eq!(daemon_status(), Request::DaemonStatus);
    }

    #[test]
    fn get_words_carries_the_id() {
        let id = parse_id("20260519T143500042").unwrap();
        match get_words(id.clone(), None) {
            Request::GetWords {
                id: got,
                variant: None,
            } => assert_eq!(got, id),
            other => panic!("expected GetWords, got {other:?}"),
        }
        match get_words(id.clone(), Some("cleaned")) {
            Request::GetWords {
                variant: Some(v), ..
            } => assert_eq!(v, "cleaned"),
            other => panic!("expected cleaned GetWords, got {other:?}"),
        }
    }

    #[test]
    fn more_like_this_carries_id_and_limit() {
        let id = parse_id("20260519T143500042").unwrap();
        // Default limit when absent.
        match more_like_this(id.clone(), &SimilarQuery::default()) {
            Request::MoreLikeThis { id: got, limit } => {
                assert_eq!(got, id);
                assert_eq!(limit, DEFAULT_SEARCH_LIMIT);
            }
            other => panic!("expected MoreLikeThis, got {other:?}"),
        }
        // Explicit limit honored.
        match more_like_this(id.clone(), &SimilarQuery { limit: Some(7) }) {
            Request::MoreLikeThis { limit, .. } => assert_eq!(limit, 7),
            other => panic!("expected MoreLikeThis, got {other:?}"),
        }
    }

    #[test]
    fn tags_endpoints_map_to_their_variants() {
        assert_eq!(list_tags(), Request::ListTags);
        assert_eq!(list_queue(), Request::ListQueue);

        let id = parse_id("20260519T143500042").unwrap();
        match tags_for(id.clone()) {
            Request::TagsFor { recording_id } => assert_eq!(recording_id, id),
            other => panic!("expected TagsFor, got {other:?}"),
        }
        match attach_tag(id.clone(), &AttachTagBody { tag_id: 42 }) {
            Request::AttachTag {
                recording_id,
                tag_id,
            } => {
                assert_eq!(recording_id, id);
                assert_eq!(tag_id, 42);
            }
            other => panic!("expected AttachTag, got {other:?}"),
        }
        match detach_tag(id.clone(), 42) {
            Request::DetachTag {
                recording_id,
                tag_id,
            } => {
                assert_eq!(recording_id, id);
                assert_eq!(tag_id, 42);
            }
            other => panic!("expected DetachTag, got {other:?}"),
        }
    }

    #[test]
    fn set_title_passes_some_and_none() {
        let id = parse_id("20260519T143500042").unwrap();
        match set_title(
            id.clone(),
            &TitleBody {
                title: Some("Quarterly review".into()),
            },
        ) {
            Request::SetRecordingTitle { id: got, title } => {
                assert_eq!(got, id);
                assert_eq!(title.as_deref(), Some("Quarterly review"));
            }
            other => panic!("expected SetRecordingTitle, got {other:?}"),
        }
        // Absent title clears it (back to auto).
        match set_title(id.clone(), &TitleBody::default()) {
            Request::SetRecordingTitle { title, .. } => assert_eq!(title, None),
            other => panic!("expected SetRecordingTitle, got {other:?}"),
        }
    }

    #[test]
    fn set_favorite_carries_the_flag() {
        let id = parse_id("20260519T143500042").unwrap();
        match set_favorite(id.clone(), &FavoriteBody { favorite: true }) {
            Request::SetFavorite { id: got, favorite } => {
                assert_eq!(got, id);
                assert!(favorite);
            }
            other => panic!("expected SetFavorite, got {other:?}"),
        }
        // Default body is unstarred.
        match set_favorite(id.clone(), &FavoriteBody::default()) {
            Request::SetFavorite { favorite, .. } => assert!(!favorite),
            other => panic!("expected SetFavorite, got {other:?}"),
        }
    }

    #[test]
    fn set_pinned_carries_the_flag() {
        let id = parse_id("20260519T143500042").unwrap();
        match set_pinned(id.clone(), &PinnedBody { pinned: true }) {
            Request::SetPinned { id: got, pinned } => {
                assert_eq!(got, id);
                assert!(pinned);
            }
            other => panic!("expected SetPinned, got {other:?}"),
        }
        // Default body is unpinned.
        match set_pinned(id.clone(), &PinnedBody::default()) {
            Request::SetPinned { pinned, .. } => assert!(!pinned),
            other => panic!("expected SetPinned, got {other:?}"),
        }
    }

    #[test]
    fn rerun_cleanup_and_summary_leave_overrides_unset() {
        let id = parse_id("20260519T143500042").unwrap();
        match rerun_cleanup(id.clone()) {
            Request::RerunCleanup {
                id: got,
                model,
                provider,
                prompt,
                api_url,
                api_key,
            } => {
                assert_eq!(got, id);
                assert!(
                    model.is_none()
                        && provider.is_none()
                        && prompt.is_none()
                        && api_url.is_none()
                        && api_key.is_none(),
                    "REST cleanup must not carry per-run overrides"
                );
            }
            other => panic!("expected RerunCleanup, got {other:?}"),
        }
        match rerun_summary(id.clone()) {
            Request::RerunSummary {
                id: got,
                model,
                prompt,
                provider,
                api_url,
                api_key,
            } => {
                assert_eq!(got, id);
                assert!(model.is_none() && prompt.is_none());
                // REST doesn't expose the per-run connection overrides.
                assert!(provider.is_none() && api_url.is_none() && api_key.is_none());
            }
            other => panic!("expected RerunSummary, got {other:?}"),
        }
    }

    #[test]
    fn meeting_start_stop_map_to_their_variants() {
        assert_eq!(meeting_start(), Request::StartMeeting);
        assert_eq!(meeting_stop(), Request::StopMeeting);
    }

    #[test]
    fn import_carries_path_recipe_and_ext_ref() {
        match import_recording(&ImportBody {
            path: "C:/audio/talk.m4a".into(),
            recipe_id: Some("lecture".into()),
            ext_ref: Some("video-1".into()),
        }) {
            Request::ImportRecording {
                path,
                recipe_id,
                ext_ref,
            } => {
                assert_eq!(path, "C:/audio/talk.m4a");
                assert_eq!(recipe_id.as_deref(), Some("lecture"));
                assert_eq!(ext_ref.as_deref(), Some("video-1"));
            }
            other => panic!("expected ImportRecording, got {other:?}"),
        }
        // No recipe / no key ⇒ default pipeline, fresh import.
        match import_recording(&ImportBody {
            path: "/tmp/a.wav".into(),
            recipe_id: None,
            ext_ref: None,
        }) {
            Request::ImportRecording {
                recipe_id, ext_ref, ..
            } => {
                assert!(recipe_id.is_none());
                assert!(ext_ref.is_none());
            }
            other => panic!("expected ImportRecording, got {other:?}"),
        }
    }
}
