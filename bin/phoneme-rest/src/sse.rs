//! `GET /api/events` — the daemon's [`DaemonEvent`] broadcast as
//! Server-Sent Events.
//!
//! This reuses the CLI `watch` subscription pattern (`bin/phoneme/src/
//! commands/watch.rs`): open a dedicated subscription connection to the daemon,
//! then forward each [`DaemonEvent`] as one SSE `data:` line carrying the
//! serialized event JSON — the exact same line `phoneme watch` prints, just
//! framed for `text/event-stream`.
//!
//! ## Disconnect handling
//!
//! The stream ends cleanly on either side hanging up:
//! - **Daemon side** — the underlying pipe stream yields `None` (connection
//!   closed) or an error; either way the SSE item stream stops yielding and
//!   axum closes the HTTP response.
//! - **Client side** — when the HTTP client drops the `EventSource`, axum drops
//!   this stream, which drops the boxed pipe stream and closes the daemon
//!   connection. No explicit unsubscribe is needed (the daemon detects the
//!   closed pipe and removes the subscriber).
//!
//! A keep-alive comment is emitted periodically so idle connections (and any
//! intermediary) don't time the stream out between events.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use futures::stream::{Stream, StreamExt};
use phoneme_ipc::DaemonEvent;

use crate::daemon;
use crate::server::AppState;

/// Maximum number of concurrent `/api/events` SSE subscribers. Each one holds a
/// dedicated daemon pipe subscription, so an unbounded fan-out — a local page
/// opening hundreds of `EventSource`s — could exhaust the daemon's pipe
/// instances and starve the CLI/GUI/MCP. A small cap keeps legitimate local
/// dashboards working while bounding the blast radius.
const MAX_SSE_CLIENTS: usize = 16;

/// Process-global permit pool enforcing [`MAX_SSE_CLIENTS`]. `const_new` lets it
/// live in a `static` with no lazy init; a permit is held for the lifetime of
/// each stream and released when the client disconnects (the stream drops).
static SSE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_SSE_CLIENTS);

/// Idle ceiling on a daemon subscription. `forward()` bounds its response read
/// with a request timeout; the subscription pipe has no equivalent, so a daemon
/// that accepts the subscribe pipe but never emits an event and never closes
/// would otherwise park its SSE slot forever (the 15s keep-alive only feeds the
/// HTTP layer — it never polls the daemon pipe). When no event arrives within
/// this window the stream ends, dropping the pipe and freeing the slot.
///
/// Deliberately generous: a healthy connection is legitimately silent whenever
/// nothing is recording (the daemon sends no heartbeat), so this only reaps a
/// genuinely wedged subscription, never a quiet-but-live dashboard.
const SUBSCRIPTION_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Serialize one [`DaemonEvent`] into an SSE [`Event`] whose `data:` line is the
/// event's JSON. Factored out so the round-trip is unit-testable without a live
/// daemon or HTTP server.
///
/// Serialization cannot realistically fail for these types; on the off chance
/// it does, the event is rendered as a JSON error object rather than dropped, so
/// the stream stays well-formed.
pub fn event_to_sse(event: &DaemonEvent) -> Event {
    match serde_json::to_string(event) {
        Ok(json) => Event::default().data(json),
        Err(e) => Event::default().data(format!(r#"{{"error":"serialize: {e}"}}"#)),
    }
}

/// Build the SSE item stream from the daemon's event subscription.
///
/// Daemon-side stream errors (a lagging subscriber the daemon disconnects, or a
/// dropped pipe) simply end the stream — SSE has no per-item error channel, and
/// a client is expected to reconnect and re-fetch state, matching the IPC
/// broadcast contract.
///
/// The next-event read is bounded by [`SUBSCRIPTION_IDLE_TIMEOUT`]: a daemon
/// that accepted the subscribe pipe but goes silent forever ends the stream
/// instead of pinning its SSE slot, mirroring `forward()`'s bounded read.
fn into_sse_stream(
    events: futures::stream::BoxStream<'static, phoneme_ipc::TransportResult<DaemonEvent>>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    // `unfold` lets us wrap each next-event read in a timeout (reset per item):
    // a `None`/`Err` from the daemon, or an idle window with no event, all
    // terminate the stream — which drops the pipe and frees the slot.
    futures::stream::unfold(events, |mut events| async move {
        match tokio::time::timeout(SUBSCRIPTION_IDLE_TIMEOUT, events.next()).await {
            Ok(Some(Ok(event))) => Some((Ok(event_to_sse(&event)), events)),
            // End-of-stream, a daemon-side error, or a silent idle window: stop.
            Ok(Some(Err(_))) | Ok(None) | Err(_) => None,
        }
    })
}

/// `GET /api/events` handler.
///
/// Returns `503` if too many event streams are already open (the concurrency cap,
/// [`MAX_SSE_CLIENTS`]) or if the daemon can't be reached to open the
/// subscription; otherwise streams events until either side disconnects.
pub async fn events(State(state): State<AppState>) -> impl IntoResponse {
    // Cap concurrent subscribers before dialing the daemon, so a flood can't open
    // a pipe subscription per request. The permit rides the stream and frees its
    // slot only when the client disconnects (axum drops the stream).
    let permit = match SSE_SLOTS.try_acquire() {
        Ok(p) => p,
        Err(_) => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "too many event streams",
            )
                .into_response()
        }
    };
    match daemon::subscribe(&state.pipe_name).await {
        Ok(events) => {
            let stream = into_sse_stream(events);
            // Hold the permit for the whole stream lifetime: capturing it in the
            // map closure means it drops (freeing the slot) when the stream is
            // dropped on disconnect. A failed subscription drops it immediately.
            let stream = stream.map(move |item| {
                let _ = &permit;
                item
            });
            Sse::new(stream)
                .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
                .into_response()
        }
        Err(e) => crate::error::RestError::Transport(e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use phoneme_core::RecordingId;

    /// Render one [`Event`] to the exact bytes axum would write on the wire by
    /// pushing it through `Sse` → `IntoResponse` → body, so tests can assert on
    /// the real `data: …\n\n` frame rather than the (non-wire) `Debug` form.
    async fn render_event_wire(event: Event) -> String {
        let stream = futures::stream::once(async move { Ok::<_, Infallible>(event) });
        let resp = Sse::new(stream).into_response();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// An event serialized into an SSE frame must carry the same JSON the
    /// daemon put on the wire — i.e. `serde_json::to_string(event)` — so a
    /// browser `EventSource` can `JSON.parse(e.data)` straight back into the
    /// event shape clients already know.
    #[tokio::test]
    async fn sse_data_line_is_the_event_json_roundtrip() {
        let event = DaemonEvent::RecordingStarted {
            id: RecordingId::parse("20260519T143500042").unwrap(),
            started_at: Local::now(),
            meeting_id: None,
            track: None,
        };
        let expected = serde_json::to_string(&event).unwrap();

        // Render what `event_to_sse` actually produced to its wire form. A
        // single-line JSON payload frames as exactly `data: <json>\n\n`.
        let wire = render_event_wire(event_to_sse(&event)).await;
        assert_eq!(
            wire,
            format!("data: {expected}\n\n"),
            "the SSE data line must be exactly serde_json::to_string(event)"
        );

        // And the data: payload, pulled back out, deserializes to the same event
        // — what a browser `EventSource` does with `JSON.parse(e.data)`.
        let data = wire
            .strip_prefix("data: ")
            .and_then(|s| s.strip_suffix("\n\n"))
            .expect("frame must be a single `data: …` field block");
        assert_eq!(data, expected);
        let parsed: DaemonEvent = serde_json::from_str(data).unwrap();
        assert_eq!(parsed, event);
    }

    /// The serialize-failure fallback (`event_to_sse`'s `Err` arm) must still
    /// emit a well-formed JSON error object on the `data:` line so the stream
    /// stays parseable, never a dropped/blank frame. `DaemonEvent` is infallible
    /// to serialize, so drive the same fallback `Event` directly: it is the exact
    /// value the `Err` arm constructs, byte-for-byte.
    #[tokio::test]
    async fn sse_serialize_failure_emits_well_formed_error_object() {
        // The literal the Err arm builds (see `event_to_sse`): a JSON object with
        // a single "error" string starting "serialize: ".
        let fallback = Event::default().data(r#"{"error":"serialize: boom"}"#);
        let wire = render_event_wire(fallback).await;
        assert_eq!(wire, "data: {\"error\":\"serialize: boom\"}\n\n");

        // The error payload is itself valid JSON with the documented shape.
        let data = wire
            .strip_prefix("data: ")
            .and_then(|s| s.strip_suffix("\n\n"))
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(parsed["error"], "serialize: boom");
    }

    /// With every SSE slot already taken, a further subscriber is turned away
    /// with `503` and the cap-specific message — before it dials the daemon (the
    /// pipe name here is never used, so a "too many" body proves the cap path,
    /// not a daemon-unreachable 503). Releasing the held permits frees the slots.
    #[tokio::test]
    async fn sse_cap_rejects_extra_streams_with_503() {
        let mut held = Vec::new();
        for _ in 0..MAX_SSE_CLIENTS {
            held.push(
                SSE_SLOTS
                    .try_acquire()
                    .expect("a slot should be free at start"),
            );
        }
        let resp = events(State(AppState {
            pipe_name: "phoneme-rest-test-sse-cap-unused".into(),
        }))
        .await
        .into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("too many event streams"),
            "the cap rejection (not a daemon-unreachable 503) must surface, got: {text}"
        );
        drop(held); // free the slots for any other test
    }

    /// A daemon subscription that accepts the pipe but never emits and never
    /// closes must not pin its SSE slot forever: the idle bound ends the stream.
    /// `start_paused` auto-advances tokio's clock past
    /// [`SUBSCRIPTION_IDLE_TIMEOUT`] while the inner stream stays pending, so the
    /// test resolves instantly instead of waiting the real 300s.
    #[tokio::test(start_paused = true)]
    async fn silent_subscription_times_out_and_ends() {
        // A stream that is forever pending — models a wedged daemon pipe.
        let pending = futures::stream::pending::<phoneme_ipc::TransportResult<DaemonEvent>>();
        let mut stream = Box::pin(into_sse_stream(pending.boxed()));
        assert!(
            stream.next().await.is_none(),
            "a silent subscription must end (freeing its slot), not hang forever"
        );
    }

    /// A serializable event with payload fields keeps every field in the SSE
    /// `data:` JSON.
    #[test]
    fn sse_preserves_event_payload_fields() {
        let event = DaemonEvent::TranscriptionDone {
            id: RecordingId::parse("20260519T143500042").unwrap(),
            transcript: "hello world".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("transcription_done"));
        assert!(json.contains("hello world"));
        let parsed: DaemonEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, event);
    }
}
