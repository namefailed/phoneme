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

use phoneme_core::{ListFilter, ListKind, RecordMode, RecordingId};
use phoneme_ipc::Request;
use serde::Deserialize;

/// Query parameters for `GET /api/recordings`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListQuery {
    /// Maximum rows to return (maps to [`ListFilter::limit`]).
    pub limit: Option<u32>,
    /// Rows to skip before returning results (maps to [`ListFilter::offset`]).
    pub offset: Option<u32>,
    /// Recording-type filter: `single` or `meeting` (maps to
    /// [`ListFilter::kind`]). Any other value is ignored (treated as "all").
    pub kind: Option<String>,
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

/// `GET /api/recordings` → [`Request::ListRecordings`].
pub fn list_recordings(q: &ListQuery) -> Request {
    Request::ListRecordings {
        filter: ListFilter {
            limit: q.limit,
            offset: q.offset,
            kind: parse_kind(q.kind.as_deref()),
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

/// `GET /api/recordings/:id/segments` → [`Request::GetSegments`].
pub fn get_segments(id: RecordingId) -> Request {
    Request::GetSegments { id, variant: None }
}

/// `GET /api/recordings/:id/words` → [`Request::GetWords`].
pub fn get_words(id: RecordingId) -> Request {
    Request::GetWords { id, variant: None }
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
        match get_segments(id.clone()) {
            Request::GetSegments { id: got, .. } => assert_eq!(got, id),
            other => panic!("expected GetSegments, got {other:?}"),
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
        match get_words(id.clone()) {
            Request::GetWords { id: got, .. } => assert_eq!(got, id),
            other => panic!("expected GetWords, got {other:?}"),
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
            } => {
                assert_eq!(got, id);
                assert!(model.is_none() && prompt.is_none());
            }
            other => panic!("expected RerunSummary, got {other:?}"),
        }
    }

    #[test]
    fn meeting_start_stop_map_to_their_variants() {
        assert_eq!(meeting_start(), Request::StartMeeting);
        assert_eq!(meeting_stop(), Request::StopMeeting);
    }
}
