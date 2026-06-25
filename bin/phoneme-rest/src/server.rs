//! Router construction and the shared handler state.
//!
//! [`router`] wires every endpoint to its handler; [`AppState`] carries the one
//! thing handlers need — the daemon pipe name to forward over. The router is
//! built independently of the listener so tests can exercise it with
//! `tower::ServiceExt::oneshot` (no socket needed) and the disabled-guard /
//! bind logic stays in `main`.

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;

use crate::{handlers, sse};

/// Whether a `Host` authority (e.g. `127.0.0.1:7777`, `localhost`, `[::1]:7777`)
/// names the loopback interface. The port is ignored; only the host matters.
fn host_is_loopback(host: &str) -> bool {
    let h = host.trim();
    // IPv6 literals are bracketed (`[::1]:port`); everything else splits on the
    // last colon to drop an optional port without mangling an IPv6 address.
    let name = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        h.rsplit_once(':').map(|(host, _port)| host).unwrap_or(h)
    };
    matches!(name, "127.0.0.1" | "::1" | "localhost")
}

/// Whether an `Origin` header value points at the loopback interface. A
/// sandboxed/opaque origin (`null`) is treated as foreign.
fn origin_is_loopback(origin: &str) -> bool {
    origin
        .split_once("://")
        .map(|(_scheme, rest)| host_is_loopback(rest.split('/').next().unwrap_or(rest)))
        .unwrap_or(false)
}

/// Reject browser cross-origin and DNS-rebinding attacks against the loopback
/// API.
///
/// The server binds to loopback, but a browser on the same machine can still
/// reach it: a malicious page can POST to it (CSRF), open an `EventSource` to
/// exhaust SSE slots, or rebind a hostname it controls to `127.0.0.1` and read
/// responses (DNS rebinding). Every such request carries a foreign `Host`
/// (rebinding) or `Origin` (cross-site fetch/EventSource) header that a browser
/// can't forge, so:
/// - a request whose `Host` is present and not loopback is refused (rebinding);
/// - a request whose `Origin` is present and not loopback is refused (CSRF or
///   cross-origin SSE). The check covers every method, not just `POST`, so a
///   cross-origin `GET /api/events` EventSource can't exhaust SSE slots.
///
/// Non-browser local clients (curl, the CLI) omit both headers and are
/// unaffected. A same-origin browser dashboard either omits `Origin` on a
/// same-origin GET or sends a loopback Origin, so neither path is blocked.
async fn loopback_guard(req: Request, next: Next) -> Response {
    if let Some(host) = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        if !host_is_loopback(host) {
            return (StatusCode::FORBIDDEN, "host not allowed").into_response();
        }
    }
    if let Some(origin) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        if !origin_is_loopback(origin) {
            return (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response();
        }
    }
    next.run(req).await
}

/// State shared by every handler: the daemon pipe to forward IPC over.
///
/// Cheap to clone (just a `String`), which axum does per request. It holds the
/// pipe name rather than a live connection because the bridge connects
/// per-request (see [`crate::daemon`]).
#[derive(Clone, Debug)]
pub struct AppState {
    /// The configured `daemon.pipe_name` to dial for each forwarded request.
    pub pipe_name: String,
}

/// Build the full REST + SSE router over the given [`AppState`].
///
/// Endpoints (all under `/api`, all loopback-only — see [`crate`] docs):
///
/// | Method | Path                          | Daemon `Request`  |
/// |--------|-------------------------------|-------------------|
/// | GET    | `/api/health`                 | `DaemonStatus`    |
/// | GET    | `/api/status`                 | `DaemonStatus`    |
/// | GET    | `/api/recordings`             | `ListRecordings`  |
/// | GET    | `/api/recordings/{id}`        | `GetRecording`    |
/// | GET    | `/api/recordings/{id}/segments` | `GetSegments`   |
/// | GET    | `/api/recordings/{id}/words`  | `GetWords`        |
/// | GET    | `/api/recordings/{id}/chapters` | `GetChapters`   |
/// | GET    | `/api/recordings/{id}/versions` | `ListTranscriptVersions` |
/// | POST   | `/api/recordings/{id}/clip`   | `ExportClip`      |
/// | GET    | `/api/recordings/{id}/similar` | `MoreLikeThis`   |
/// | GET    | `/api/recordings/{id}/tags`   | `TagsFor`         |
/// | POST   | `/api/recordings/{id}/tags`   | `AttachTag`       |
/// | DELETE | `/api/recordings/{id}/tags/{tag_id}` | `DetachTag` |
/// | POST   | `/api/recordings/{id}/title`  | `SetRecordingTitle` |
/// | POST   | `/api/recordings/{id}/favorite` | `SetFavorite`   |
/// | POST   | `/api/recordings/{id}/pinned` | `SetPinned`       |
/// | POST   | `/api/recordings/{id}/cleanup` | `RerunCleanup`   |
/// | POST   | `/api/recordings/{id}/summary` | `RerunSummary`   |
/// | GET    | `/api/tags`                   | `ListTags`        |
/// | GET    | `/api/queue`                  | `ListQueue`       |
/// | GET    | `/api/search`                 | `SemanticSearch`  |
/// | GET    | `/api/recipes`                | (reads config)    |
/// | POST   | `/api/import`                 | `ImportRecording` |
/// | POST   | `/api/record/start`           | `RecordStart`     |
/// | POST   | `/api/record/stop`            | `RecordStop`      |
/// | POST   | `/api/meeting/start`          | `StartMeeting`    |
/// | POST   | `/api/meeting/stop`           | `StopMeeting`     |
/// | GET    | `/api/events`                 | `SubscribeEvents` (SSE) |
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(handlers::health))
        .route("/api/status", get(handlers::status))
        .route("/api/recordings", get(handlers::list_recordings))
        .route("/api/recordings/{id}", get(handlers::get_recording))
        .route("/api/recordings/{id}/segments", get(handlers::get_segments))
        .route("/api/recordings/{id}/words", get(handlers::get_words))
        .route("/api/recordings/{id}/chapters", get(handlers::get_chapters))
        .route("/api/recordings/{id}/versions", get(handlers::get_versions))
        .route("/api/recordings/{id}/clip", post(handlers::export_clip))
        .route(
            "/api/recordings/{id}/similar",
            get(handlers::more_like_this),
        )
        .route(
            "/api/recordings/{id}/tags",
            get(handlers::tags_for).post(handlers::attach_tag),
        )
        .route(
            "/api/recordings/{id}/tags/{tag_id}",
            delete(handlers::detach_tag),
        )
        .route("/api/recordings/{id}/title", post(handlers::set_title))
        .route(
            "/api/recordings/{id}/favorite",
            post(handlers::set_favorite),
        )
        .route("/api/recordings/{id}/pinned", post(handlers::set_pinned))
        .route(
            "/api/recordings/{id}/cleanup",
            post(handlers::rerun_cleanup),
        )
        .route(
            "/api/recordings/{id}/summary",
            post(handlers::rerun_summary),
        )
        .route("/api/tags", get(handlers::list_tags))
        .route("/api/queue", get(handlers::list_queue))
        .route("/api/search", get(handlers::search))
        .route("/api/recipes", get(handlers::list_recipes))
        .route("/api/import", post(handlers::import_recording))
        .route("/api/record/start", post(handlers::record_start))
        .route("/api/record/stop", post(handlers::record_stop))
        .route("/api/meeting/start", post(handlers::meeting_start))
        .route("/api/meeting/stop", post(handlers::meeting_stop))
        .route("/api/events", get(sse::events))
        .layer(axum::middleware::from_fn(loopback_guard))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use http_body_util::BodyExt;
    use phoneme_ipc::{
        IpcError, IpcErrorKind, NamedPipeListener, Request as IpcRequest, Response, ServerRequest,
    };
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt; // for `oneshot`

    /// A throwaway daemon stand-in on a unique pipe — records every request and
    /// answers via the supplied closure. Mirrors the CLI's `test_support`
    /// MockDaemon, inlined here so the dispatch tests need no live daemon.
    struct MockDaemon {
        pipe_name: String,
        received: Arc<Mutex<Vec<IpcRequest>>>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl MockDaemon {
        fn spawn<F>(label: &str, responder: F) -> Self
        where
            F: Fn(&IpcRequest) -> Response + Send + Sync + 'static,
        {
            let pid = std::process::id();
            let ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let pipe_name = format!("phoneme-rest-test-{label}-{pid}-{ns}");
            let mut listener = NamedPipeListener::bind(&pipe_name).expect("bind mock pipe");
            let received = Arc::new(Mutex::new(Vec::new()));
            let responder = Arc::new(responder);

            let handle = {
                let received = received.clone();
                tokio::spawn(async move {
                    loop {
                        let Ok(mut conn) = listener.accept().await else {
                            break;
                        };
                        let received = received.clone();
                        let responder = responder.clone();
                        tokio::spawn(async move {
                            while let Ok(Some(req)) = conn.recv().await {
                                let ServerRequest::Known(req) = req else {
                                    continue;
                                };
                                let req = *req;
                                let response = responder(&req);
                                received.lock().unwrap().push(req);
                                if conn.send_response(response).await.is_err() {
                                    return;
                                }
                            }
                        });
                    }
                })
            };

            Self {
                pipe_name,
                received,
                handle,
            }
        }

        fn received(&self) -> Vec<IpcRequest> {
            self.received.lock().unwrap().clone()
        }
    }

    impl Drop for MockDaemon {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// `GET /api/recordings?limit=5&kind=meeting` forwards exactly
    /// `ListRecordings` with that filter, and the daemon's JSON is returned
    /// verbatim.
    #[tokio::test]
    async fn get_recordings_forwards_list_request() {
        let mock = MockDaemon::spawn("list", |_req| {
            Response::Ok(serde_json::json!([{ "id": "x" }]))
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/recordings?limit=5&offset=2&kind=meeting")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json, serde_json::json!([{ "id": "x" }]));

        let got = mock.received();
        assert_eq!(got.len(), 1);
        match &got[0] {
            IpcRequest::ListRecordings { filter } => {
                assert_eq!(filter.limit, Some(5));
                assert_eq!(filter.offset, Some(2));
                assert_eq!(filter.kind, Some(phoneme_core::ListKind::Meeting));
            }
            other => panic!("expected ListRecordings, got {other:?}"),
        }
    }

    /// `POST /api/record/start` forwards `RecordStart`; `POST /api/record/stop`
    /// forwards `RecordStop`.
    #[tokio::test]
    async fn record_start_and_stop_forward_their_requests() {
        let mock = MockDaemon::spawn("record", |req| match req {
            IpcRequest::RecordStart { .. } => Response::Ok(serde_json::json!({ "id": "r1" })),
            IpcRequest::RecordStop => Response::Ok(serde_json::json!({ "id": "r1" })),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let start = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/record/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(start.status(), StatusCode::OK);

        let stop = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/record/stop")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stop.status(), StatusCode::OK);

        let got = mock.received();
        assert!(matches!(got[0], IpcRequest::RecordStart { .. }));
        assert!(matches!(got[1], IpcRequest::RecordStop));
    }

    /// A malformed `:id` never reaches the daemon and is a flat `400`.
    #[tokio::test]
    async fn bad_recording_id_is_400_and_not_forwarded() {
        let mock = MockDaemon::spawn("badid", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/recordings/not-a-real-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            mock.received().is_empty(),
            "a bad id must be rejected before any IPC request is sent"
        );
    }

    /// A daemon `not_found` error becomes HTTP `404`.
    #[tokio::test]
    async fn daemon_not_found_becomes_404() {
        let mock = MockDaemon::spawn("notfound", |_req| {
            Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: "no such recording".into(),
            })
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/recordings/20260519T143500042")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let json = body_json(resp).await;
        assert_eq!(json["error"], "no such recording");
    }

    /// When no daemon is listening, every forwarding endpoint is `503` (the
    /// daemon-unreachable mapping) — including `/api/health`.
    #[tokio::test]
    async fn unreachable_daemon_is_503() {
        // A pipe name nothing is bound to.
        let app = router(AppState {
            pipe_name: "phoneme-rest-test-nonexistent-pipe-xyzzy".into(),
        });

        for uri in ["/api/health", "/api/status", "/api/recordings"] {
            let resp = app
                .clone()
                .oneshot(HttpRequest::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri} should be 503 when the daemon is down"
            );
        }
    }

    /// `GET /api/health` is `200 {"status":"ok"}` when the daemon answers its
    /// `DaemonStatus` probe.
    #[tokio::test]
    async fn health_ok_when_daemon_answers() {
        let mock = MockDaemon::spawn("health", |req| {
            assert!(matches!(req, IpcRequest::DaemonStatus));
            Response::Ok(serde_json::json!({ "running": true }))
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["status"], "ok");
    }

    /// A request carrying a non-loopback `Host` (the DNS-rebinding signature) is
    /// refused with 403 before any IPC is forwarded.
    #[tokio::test]
    async fn spoofed_host_is_403_and_not_forwarded() {
        let mock = MockDaemon::spawn("spoofhost", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/recordings")
                    .header("host", "evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(
            mock.received().is_empty(),
            "a spoofed Host must be rejected before any IPC request is sent"
        );
    }

    /// A cross-origin `POST` (the CSRF signature) is refused with 403; a loopback
    /// Origin on the same POST is allowed through.
    #[tokio::test]
    async fn cross_origin_post_is_403_loopback_origin_ok() {
        let mock = MockDaemon::spawn("xorigin", |req| match req {
            IpcRequest::RecordStart { .. } => Response::Ok(serde_json::json!({ "id": "r1" })),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let foreign = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/record/start")
                    .header("origin", "http://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);

        let local = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/record/start")
                    .header("origin", "http://127.0.0.1:7777")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(local.status(), StatusCode::OK);
    }

    /// A cross-origin `GET` (e.g. EventSource from a foreign page) is refused
    /// with 403, matching the existing POST guard. This closes the SSE slot-
    /// exhaustion vector described in audit finding R27.
    #[tokio::test]
    async fn cross_origin_get_is_403() {
        let mock = MockDaemon::spawn("xorigin-get", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/api/recordings")
                    .header("origin", "http://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(
            mock.received().is_empty(),
            "a cross-origin GET must be rejected before any IPC request is sent"
        );
    }

    /// `GET /api/tags` forwards `ListTags`; `GET /api/queue` forwards
    /// `ListQueue`. Both pass the daemon's JSON through verbatim.
    #[tokio::test]
    async fn list_tags_and_queue_forward_their_requests() {
        let mock = MockDaemon::spawn("tagsqueue", |req| match req {
            IpcRequest::ListTags => Response::Ok(serde_json::json!([{ "id": 1, "name": "work" }])),
            IpcRequest::ListQueue => Response::Ok(serde_json::json!([{ "id": "q1" }])),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let tags = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/tags")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tags.status(), StatusCode::OK);
        assert_eq!(
            body_json(tags).await,
            serde_json::json!([{ "id": 1, "name": "work" }])
        );

        let queue = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/queue")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(queue.status(), StatusCode::OK);

        let got = mock.received();
        assert!(matches!(got[0], IpcRequest::ListTags));
        assert!(matches!(got[1], IpcRequest::ListQueue));
    }

    /// `GET /api/recordings/{id}/words` and `/similar` forward `GetWords` and
    /// `MoreLikeThis` (the latter honoring `?limit=`).
    #[tokio::test]
    async fn words_and_similar_forward_with_id_and_limit() {
        let mock = MockDaemon::spawn("wordsim", |req| match req {
            IpcRequest::GetWords { .. } => Response::Ok(serde_json::json!([])),
            IpcRequest::MoreLikeThis { .. } => Response::Ok(serde_json::json!([])),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let words = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/recordings/20260519T143500042/words")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(words.status(), StatusCode::OK);

        let similar = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/recordings/20260519T143500042/similar?limit=4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(similar.status(), StatusCode::OK);

        let got = mock.received();
        assert!(matches!(got[0], IpcRequest::GetWords { .. }));
        match &got[1] {
            IpcRequest::MoreLikeThis { limit, .. } => assert_eq!(*limit, 4),
            other => panic!("expected MoreLikeThis, got {other:?}"),
        }
    }

    /// `POST /api/recordings/{id}/title` with a JSON body forwards
    /// `SetRecordingTitle` carrying that title.
    #[tokio::test]
    async fn set_title_forwards_json_body() {
        let mock = MockDaemon::spawn("settitle", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/recordings/20260519T143500042/title")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Quarterly review"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        match &mock.received()[0] {
            IpcRequest::SetRecordingTitle { title, .. } => {
                assert_eq!(title.as_deref(), Some("Quarterly review"));
            }
            other => panic!("expected SetRecordingTitle, got {other:?}"),
        }
    }

    /// `POST /api/recordings/{id}/favorite` forwards `SetFavorite` with the
    /// flag; `POST .../cleanup` and `.../summary` forward their re-run variants.
    #[tokio::test]
    async fn favorite_and_reruns_forward_their_requests() {
        let mock = MockDaemon::spawn("favrerun", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let fav = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/recordings/20260519T143500042/favorite")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"favorite":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fav.status(), StatusCode::OK);

        for path in ["cleanup", "summary"] {
            let resp = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .method("POST")
                        .uri(format!("/api/recordings/20260519T143500042/{path}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "POST .../{path} should be 200"
            );
        }

        let got = mock.received();
        match &got[0] {
            IpcRequest::SetFavorite { favorite, .. } => assert!(*favorite),
            other => panic!("expected SetFavorite, got {other:?}"),
        }
        assert!(matches!(got[1], IpcRequest::RerunCleanup { .. }));
        assert!(matches!(got[2], IpcRequest::RerunSummary { .. }));
    }

    /// `POST /api/recordings/{id}/tags` attaches; `DELETE
    /// /api/recordings/{id}/tags/{tag_id}` detaches.
    #[tokio::test]
    async fn attach_and_detach_tag_forward_their_requests() {
        let mock = MockDaemon::spawn("attachtag", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let attach = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/recordings/20260519T143500042/tags")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"tag_id":7}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(attach.status(), StatusCode::OK);

        let detach = app
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri("/api/recordings/20260519T143500042/tags/7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detach.status(), StatusCode::OK);

        let got = mock.received();
        match &got[0] {
            IpcRequest::AttachTag { tag_id, .. } => assert_eq!(*tag_id, 7),
            other => panic!("expected AttachTag, got {other:?}"),
        }
        match &got[1] {
            IpcRequest::DetachTag { tag_id, .. } => assert_eq!(*tag_id, 7),
            other => panic!("expected DetachTag, got {other:?}"),
        }
    }

    /// `POST /api/meeting/start` and `/stop` forward the meeting variants.
    #[tokio::test]
    async fn meeting_start_and_stop_forward_their_requests() {
        let mock = MockDaemon::spawn("meeting", |req| match req {
            IpcRequest::StartMeeting => Response::Ok(serde_json::json!({ "meeting_id": "m1" })),
            IpcRequest::StopMeeting => Response::Ok(serde_json::json!({ "meeting_id": "m1" })),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        for path in ["start", "stop"] {
            let resp = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .method("POST")
                        .uri(format!("/api/meeting/{path}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        let got = mock.received();
        assert!(matches!(got[0], IpcRequest::StartMeeting));
        assert!(matches!(got[1], IpcRequest::StopMeeting));
    }

    /// `POST /api/import` forwards `ImportRecording` carrying the body's path and
    /// recipe id.
    #[tokio::test]
    async fn import_forwards_path_and_recipe() {
        let mock = MockDaemon::spawn("import", |req| match req {
            IpcRequest::ImportRecording { .. } => Response::Ok(serde_json::json!({ "id": "i1" })),
            _ => Response::Ok(serde_json::Value::Null),
        });
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/import")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"C:/audio/talk.m4a","recipe_id":"lecture"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        match &mock.received()[0] {
            IpcRequest::ImportRecording { path, recipe_id } => {
                assert_eq!(path, "C:/audio/talk.m4a");
                assert_eq!(recipe_id.as_deref(), Some("lecture"));
            }
            other => panic!("expected ImportRecording, got {other:?}"),
        }
    }

    /// A malformed `:id` on a new id-bearing route is `400` and never forwarded
    /// — same guard as `/api/recordings/{id}`.
    #[tokio::test]
    async fn bad_id_on_new_routes_is_400_and_not_forwarded() {
        let mock = MockDaemon::spawn("badidnew", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/recordings/not-a-real-id/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            mock.received().is_empty(),
            "a bad id must be rejected before any IPC request is sent"
        );
    }

    /// The cross-origin POST guard also covers the new mutating routes (CSRF):
    /// a foreign `Origin` on `POST .../favorite` is `403` and never forwarded.
    #[tokio::test]
    async fn cross_origin_post_on_new_route_is_403() {
        let mock = MockDaemon::spawn("xorignew", |_req| Response::Ok(serde_json::Value::Null));
        let app = router(AppState {
            pipe_name: mock.pipe_name.clone(),
        });

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/recordings/20260519T143500042/favorite")
                    .header("origin", "http://evil.example.com")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"favorite":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(mock.received().is_empty());
    }

    #[test]
    fn loopback_host_and_origin_classification() {
        for h in ["127.0.0.1", "127.0.0.1:7777", "localhost:80", "[::1]:7777"] {
            assert!(host_is_loopback(h), "{h} should be loopback");
        }
        for h in ["evil.com", "evil.com:7777", "127.0.0.1.evil.com", "0.0.0.0"] {
            assert!(!host_is_loopback(h), "{h} should NOT be loopback");
        }
        assert!(origin_is_loopback("http://localhost:7777"));
        assert!(!origin_is_loopback("http://evil.com"));
        assert!(!origin_is_loopback("null"));
    }
}
