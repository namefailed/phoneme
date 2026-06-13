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
    Request::GetSegments { id }
}

/// `GET /api/search` → [`Request::SemanticSearch`].
pub fn search(q: &SearchQuery) -> Request {
    Request::SemanticSearch {
        query: q.q.clone(),
        limit: q.limit.unwrap_or(DEFAULT_SEARCH_LIMIT),
    }
}

/// `POST /api/record/start` → [`Request::RecordStart`].
///
/// The REST surface always starts a `hold`-mode recording (stop is an explicit
/// `POST /api/record/stop`); dictation/in-place is not exposed over HTTP.
pub fn record_start() -> Request {
    Request::RecordStart {
        mode: RecordMode::Hold,
        in_place: false,
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
            Request::GetSegments { id: got } => assert_eq!(got, id),
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
            Request::SemanticSearch { query, limit } => {
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
            Request::RecordStart { mode, in_place } => {
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
}
