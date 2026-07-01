//! IPC request routing — where every client request meets the daemon.
//!
//! [`ipc_server`](crate::ipc_server) hands each accepted pipe connection to
//! `handle_connection`, which loops:
//!   1. Read one Request (leniently — an unknown variant from a newer client
//!      is answered with an error, never a dropped connection).
//!   2. Call `handle_request` to produce a Response.
//!   3. Send the Response.
//!   4. Repeat until the client closes.
//!
//! `SubscribeEvents` is special — it permanently converts the connection
//! into a one-way `DaemonEvent` stream fed from the [`crate::event_bus`],
//! closing it when the subscriber lags.
//!
//! `handle_request` is the single dispatch point for the whole wire contract
//! (see `phoneme-ipc::schema` for the per-request documentation). Most handlers
//! read or write the catalog directly and return; the ones that create
//! transcription work (`RecordStop` via the recorder, `ImportRecording`,
//! `RetranscribeRecording`) enqueue into the inbox for the queue worker.
//! Handlers have to stay fast: anything slow (hook re-fires, LLM cleanup/summary
//! re-runs, import decoding) runs in a spawned task or on a blocking thread and
//! reports through `DaemonEvent`s, since one stalled handler would stall every
//! queued request on that connection, and the tray's single-connection bridge
//! with it.
//!
//! A few security invariants live here. `RefireHook` only runs commands already
//! in the configured hook allowlist (S-C2); `HookTest` output is secret-redacted
//! on both outcomes; `DeleteRecording` only unlinks audio under the configured
//! audio dir; `ImportRecording` canonicalizes the path and enforces a size cap
//! before decoding.

use crate::app_state::AppState;
use phoneme_core::hook::redact_secrets;
use phoneme_core::{HookMetadata, HookPayload, HookRunner, RecordingStatus};
use phoneme_ipc::{
    DaemonEvent, IpcError, IpcErrorKind, NamedPipeConnection, PipelineStage, Request, Response,
    ServerRequest,
};

/// Collapse the boilerplate the uniform catalog handler arms all share: await a
/// fallible catalog (or recorder) call and map it to a `Response`, with the
/// `Err` arm always going through [`err_response`]. The trailing tag picks how the
/// `Ok` value becomes the response, matching the four shapes that recur verbatim:
///
///   - `=> serialize`         → [`serialize_response`] of the value
///   - `=> to_value`          → `Response::Ok(serde_json::to_value(v).unwrap_or_default())`
///   - `=> json "key"`        → `Response::Ok(serde_json::json!({ "key": v }))`
///   - `=> ok_null`           → `Ok(())` answered with [`ok_null`]
///
/// Only the genuinely-uniform arms use this — anything that emits a `DaemonEvent`,
/// inspects the Ok value, or does extra work stays written out longhand.
macro_rules! catalog_call {
    ($call:expr => serialize) => {
        match $call {
            Ok(v) => serialize_response(v),
            Err(e) => err_response(&e),
        }
    };
    ($call:expr => to_value) => {
        match $call {
            Ok(v) => Response::Ok(serde_json::to_value(v).unwrap_or_default()),
            Err(e) => err_response(&e),
        }
    };
    ($call:expr => json $key:literal) => {
        match $call {
            Ok(v) => Response::Ok(serde_json::json!({ $key: v })),
            Err(e) => err_response(&e),
        }
    };
    ($call:expr => ok_null) => {
        match $call {
            Ok(()) => ok_null(),
            Err(e) => err_response(&e),
        }
    };
}

/// How long the `Shutdown` handler waits after returning its Ok response
/// before actually triggering the shutdown. The response write itself takes
/// microseconds — this just guarantees the reply is on the pipe before the
/// process begins to exit, so the caller always sees the acknowledgement.
const SHUTDOWN_REPLY_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// Minimum calibrated relevance (0..1) a semantic-only hit must clear to
/// surface. Hybrid search ranks by per-chunk best-match cosine fused (RRF) with
/// the FTS5 lexical ranking, so this isn't a raw-cosine floor: a lexical
/// (exact-term) hit is never filtered by it, and the score is calibrated so
/// 0.12 ≈ "barely related". See `catalog::hybrid_search`. `MoreLikeThis` applies
/// the same floor to its pure-vector ranking so both search paths agree on
/// what's too weak to show.
const SEMANTIC_MIN_RELEVANCE: f32 = 0.12;

/// Default number of grounding chunks Ask-my-archive retrieves when the client
/// sends `top_k = 0`. Small for a tight, citable answer on a modest local model.
const ASK_DEFAULT_TOP_K: usize = 8;
/// Hard cap on the client-supplied Ask `top_k`, so a huge value can't blow up the
/// retrieval / prompt budget.
const ASK_MAX_TOP_K: usize = 24;

pub async fn handle_connection(mut conn: NamedPipeConnection, state: AppState) {
    loop {
        // Read one request. An unrecognized-but-well-formed request (a client
        // ahead of this daemon during a rolling rebuild) is answered with an
        // error and the connection is kept open. A single unknown request should
        // never tear down the pipe and break this client's other commands.
        let req = match conn.recv().await {
            Ok(Some(ServerRequest::Known(req))) => *req,
            Ok(Some(ServerRequest::Unknown { detail })) => {
                tracing::warn!(
                    %detail,
                    "unrecognized IPC request; replying with an error and keeping the connection alive"
                );
                let resp = Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("unsupported or unrecognized request: {detail}"),
                });
                if let Err(e) = conn.send_response(resp).await {
                    tracing::warn!(error = %e, "send_response failed");
                    return;
                }
                continue;
            }
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "recv failed");
                return;
            }
        };
        match req {
            Request::SubscribeEvents => {
                // No ACK Response is sent. The client reframes its connection as
                // a `DaemonEvent` stream the instant it writes `SubscribeEvents`,
                // so an ACK `Response` would be decoded by that reframed codec as
                // a malformed `DaemonEvent`, abort the stream, and make every
                // blocking `phoneme record` fail. Go straight into event
                // streaming.
                //
                // Backpressure contract: this connection uses a broadcast
                // receiver, which drops old events under lag rather than blocking
                // the producer. On `Lagged(n)` we tear down the subscription; the
                // client sees the connection close and is expected to reconnect
                // (which freshly re-subscribes) and re-fetch state via
                // `ListRecordings`. Subscribers treat a subscription close as "the
                // world may have moved on; refetch."
                //
                // Closing on lag beats silently dropping events, which would leave
                // the client's incremental UI state diverged from the catalog with
                // no signal that anything's wrong.
                let mut rx = state.events.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            if let Err(e) = conn.send_event(event).await {
                                tracing::debug!(error = %e, "event send failed; subscriber gone");
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                lag = n,
                                "event subscriber lagged; closing subscription so client re-syncs"
                            );
                            return; // client reconnects, re-fetches ListRecordings.
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
            other => {
                let response = handle_request(other, &state).await;
                if let Err(e) = conn.send_response(response).await {
                    tracing::warn!(error = %e, "send_response failed");
                    return;
                }
            }
        }
    }
}

pub async fn handle_request(req: Request, state: &AppState) -> Response {
    match req {
        Request::Handshake { protocol_version } => {
            // Cheap, config-free wire handshake (F3). Report our protocol and app
            // version and whether the client's protocol matches ours. Never an
            // error: even an incompatible client gets a clear, parseable answer.
            Response::Ok(serde_json::json!({
                "protocol_version": phoneme_ipc::PROTOCOL_VERSION,
                "app_version": env!("CARGO_PKG_VERSION"),
                "compatible": protocol_version == phoneme_ipc::PROTOCOL_VERSION,
            }))
        }
        Request::DaemonStatus => {
            // Bundled whisper-server ports: `preferred` is the configured
            // value, `effective` is the port the supervisor actually bound —
            // it falls back to a free port when the preferred one is held by
            // another app, and is `null` while that server isn't running.
            // Clients probing the local server (the tray's connection test,
            // doctor wiring) should dial the effective port when present.
            let cfg = state.config.load();
            Response::Ok(serde_json::json!({
                "running": true,
                "pid": std::process::id(),
                "version": env!("CARGO_PKG_VERSION"),
                "whisper_preferred_port": cfg.whisper.bundled_server_port,
                "whisper_effective_port": state.whisper_ports.main(),
                "preview_whisper_preferred_port":
                    cfg.preview_whisper.as_ref().map(|p| p.bundled_server_port),
                "preview_whisper_effective_port": state.whisper_ports.preview(),
                "dictation_whisper_preferred_port":
                    cfg.in_place.stt.as_ref().map(|s| s.bundled_server_port),
                "dictation_whisper_effective_port": state.whisper_ports.dictation(),
            }))
        }
        Request::RecordStatus => {
            let active = state.recorder.current().await;
            let meeting_active = state.recorder.meeting_active().await;
            let paused = state.recorder.is_paused().await;
            Response::Ok(serde_json::json!({
                "recording": active.is_some(),
                "id": active.as_ref().map(|a| a.id.to_string()),
                "meeting": meeting_active,
                "paused": paused,
            }))
        }
        Request::RecordStart {
            mode,
            in_place,
            recipe_id,
            whisper_model,
            source,
        } => {
            match state.recorder.start(state, mode, in_place, source).await {
                Ok(id) => {
                    // Custom-hotkey overrides: a binding that named a recipe or STT
                    // model stashes them against this recording's id, mirroring how
                    // `RetranscribeRecording` populates the per-job ledgers. The
                    // pipeline consumes (and removes) them in `run`. Empty or None
                    // means the normal record path (global default recipe +
                    // configured model), so non-custom recordings are untouched.
                    // See `stash_hotkey_overrides`.
                    stash_hotkey_overrides(state, &id, recipe_id, whisper_model);
                    Response::Ok(serde_json::json!({ "id": id.to_string() }))
                }
                Err(e) => err_response(&e),
            }
        }
        Request::StartMeeting => {
            catalog_call!(state.recorder.start_meeting(state).await => json "meeting_id")
        }
        Request::StopMeeting => {
            catalog_call!(state.recorder.stop_meeting(state).await => json "meeting_id")
        }
        Request::MeetingToggle => {
            // Atomic toggle: the recorder holds a guard across the read+act so a
            // double-tapped hotkey can't race two starts (or two stops). See
            // `DaemonRecorder::toggle_meeting`.
            catalog_call!(state.recorder.toggle_meeting(state).await => json "started")
        }
        Request::RecordStop => match state.recorder.stop(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::RecordToggle {
            in_place,
            recipe_id,
            whisper_model,
            source,
        } => {
            if state.recorder.current().await.is_some() {
                // Stop half of the toggle: there is no new recording to attach the
                // binding's overrides to (the active one was started with its own,
                // if any), so the recipe/model fields are ignored here.
                match state.recorder.stop(state).await {
                    Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
                    Err(e) => err_response(&e),
                }
            } else {
                match state
                    .recorder
                    .start(state, phoneme_core::RecordMode::Hold, in_place, source)
                    .await
                {
                    Ok(id) => {
                        // Start half: stash the binding's recipe/model overrides
                        // against the new recording id (see `RecordStart`).
                        stash_hotkey_overrides(state, &id, recipe_id, whisper_model);
                        Response::Ok(serde_json::json!({ "id": id.to_string() }))
                    }
                    Err(e) => err_response(&e),
                }
            }
        }
        Request::RecordPause => match state.recorder.pause(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::RecordResume => match state.recorder.resume(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::RecordCancel => match state.recorder.cancel(state).await {
            Ok(id) => Response::Ok(serde_json::json!({ "id": id.to_string() })),
            Err(e) => err_response(&e),
        },
        Request::ListRecordings { filter } => {
            catalog_call!(state.catalog.list(&filter).await => serialize)
        }
        Request::GetRecording { id } => match state.catalog.get(&id).await {
            Ok(Some(r)) => serialize_response(r),
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
        },
        Request::ListAiActivity {
            recording_id,
            limit,
        } => catalog_call!(
            state
                .catalog
                .list_ai_activity(recording_id.as_deref(), limit as i64)
                .await => serialize
        ),
        Request::ListSavedSearches => {
            catalog_call!(state.catalog.list_saved_searches().await => serialize)
        }
        Request::UpsertSavedSearch {
            id,
            name,
            filter_json,
        } => match state
            .catalog
            .upsert_saved_search(&id, &name, &filter_json)
            .await
        {
            Ok(()) => Response::Ok(serde_json::json!({})),
            Err(e) => err_response(&e),
        },
        Request::DeleteSavedSearch { id } => {
            catalog_call!(state.catalog.delete_saved_search(&id).await => json "removed")
        }
        // ── Dictation history (re-grab) ──────────────────────────────────
        Request::ListDictationHistory { limit } => {
            catalog_call!(state.catalog.list_dictation_history(limit as i64).await => serialize)
        }
        Request::DeleteDictationHistory { id } => {
            catalog_call!(state.catalog.delete_dictation_history(id).await => json "removed")
        }
        Request::ClearDictationHistory => {
            catalog_call!(state.catalog.clear_dictation_history().await => json "removed")
        }
        // Re-insert a past dictation's text at the current cursor. Resolves the
        // type/paste mode (the request's, else the global `type_mode`) and reuses
        // the dictation typing primitive verbatim — its `input_injection_disabled`
        // test guard + clipboard-restore handling apply, so this no-ops under
        // tests. An unknown id is `not_found`; a typing failure maps the String
        // error to `Internal`, like other input-injection failures.
        Request::RegrabDictation { id, mode } => {
            match state.catalog.get_dictation_history(id).await {
                Ok(Some(text)) => {
                    let m = mode
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| state.config.load().in_place.type_mode.clone());
                    match crate::in_place::type_at_cursor(&text, &m).await {
                        Ok(()) => Response::Ok(serde_json::json!({})),
                        Err(e) => err_response(&phoneme_core::Error::Internal(e)),
                    }
                }
                Ok(None) => not_found(format!("dictation {id} not found")),
                Err(e) => err_response(&e),
            }
        }
        // S2: run a stored saved search by id server-side. Same recordings shape
        // as `ListRecordings`: the catalog parses `filter_json` into a `ListFilter`
        // and runs the normal list query.
        Request::RunSavedSearch { id } => {
            let threshold = state.config.load().whisper.low_confidence_threshold;
            catalog_call!(state.catalog.run_saved_search(&id, threshold).await => serialize)
        }
        Request::ListMeeting { meeting_id } => {
            catalog_call!(state.catalog.list_by_meeting(&meeting_id).await => serialize)
        }
        // An unknown/never-digested meeting yields `null` (not `NotFound`): "no
        // digest yet" is a normal state, and the merged view treats null as "show
        // nothing / offer to generate", mirroring how segments/words return empty.
        Request::GetMeetingDigest { meeting_id } => {
            catalog_call!(state.catalog.meeting_digest(&meeting_id).await => serialize)
        }
        // Every stored digest, for the library-backup export to capture (the
        // digests live in a side table the per-recording list never carries).
        Request::ListMeetingDigests => {
            catalog_call!(state.catalog.list_all_meeting_digests().await => serialize)
        }
        // An unknown/never-generated range yields `null` (not `NotFound`): "no
        // digest yet" is a normal state, mirroring `GetMeetingDigest`.
        Request::GetPeriodDigest { key } => {
            catalog_call!(state.catalog.period_digest(&key).await => serialize)
        }
        // Every stored period digest (newest range first), for the digest panel's
        // history and the library-backup export.
        Request::ListPeriodDigests => {
            catalog_call!(state.catalog.list_all_period_digests().await => serialize)
        }
        // The library's user-added (`source='manual'`) task/entity keys, grouped
        // by recording — read by the backup export so restore can flip the rows
        // back to manual (the Task/Entity DTOs don't carry `source`).
        Request::ManualSources => {
            let tasks = match state.catalog.manual_task_texts_all().await {
                Ok(t) => t,
                Err(e) => return err_response(&e),
            };
            let mut entities = match state.catalog.manual_entity_keys_all().await {
                Ok(e) => e,
                Err(e) => return err_response(&e),
            };
            let mut out: Vec<phoneme_core::backup::ManualSources> = Vec::new();
            for (rid, task_texts) in tasks {
                let entity_keys = entities.remove(&rid).unwrap_or_default();
                out.push(phoneme_core::backup::ManualSources {
                    recording_id: rid,
                    task_texts,
                    entity_keys,
                });
            }
            // Recordings with manual entities but no manual tasks.
            for (rid, entity_keys) in entities {
                out.push(phoneme_core::backup::ManualSources {
                    recording_id: rid,
                    task_texts: Vec::new(),
                    entity_keys,
                });
            }
            serialize_response(out)
        }
        // An unknown id yields an empty list, not `NotFound`: "no segments" is a
        // normal state (pre-capture recordings, providers without timing) and
        // callers treat the two identically.
        Request::GetSegments { id, variant } => {
            let v = variant.as_deref().unwrap_or("raw");
            catalog_call!(state.catalog.segments_for_variant(&id, v).await => serialize)
        }
        // Like `GetSegments`, an unknown id yields an empty list (not `NotFound`):
        // "no words" is a normal state (pre-capture recordings, providers without
        // per-word timing). Each object carries an explicit 0-based `idx` (the
        // array order) so the frontend can rely on it without re-deriving it from
        // position. `TranscriptWord` itself stores no idx, so we attach it here
        // via enumerate.
        Request::GetWords { id, variant } => match state
            .catalog
            .words_for_variant(&id, variant.as_deref().unwrap_or("raw"))
            .await
        {
            Ok(words) => {
                let with_idx: Vec<serde_json::Value> = words
                    .into_iter()
                    .enumerate()
                    .map(|(idx, w)| {
                        serde_json::json!({
                            "idx": idx,
                            "start_ms": w.start_ms,
                            "end_ms": w.end_ms,
                            "text": w.text,
                            // Powers the Synced view's spacing (whisper's word-start
                            // marker); without it the view space-joins every token
                            // and shows "I don 't know" / "over ste pped".
                            "leading_space": w.leading_space,
                            "speaker": w.speaker,
                            "confidence": w.confidence,
                        })
                    })
                    .collect();
                serialize_response(with_idx)
            }
            Err(e) => err_response(&e),
        },
        // Compounding chain (PB-COMPOUND): list / fetch / revert transcript versions.
        Request::ListTranscriptVersions { id } => {
            catalog_call!(state.catalog.transcript_versions_for(&id).await => serialize)
        }
        Request::GetTranscriptVersion { id, idx } => {
            catalog_call!(state.catalog.transcript_version(&id, idx).await => serialize)
        }
        Request::RevertToVersion { id, idx } => {
            match state.catalog.transcript_version(&id, idx).await {
                Ok(Some(v)) => match state.catalog.update_user_transcript(&id, &v.text).await {
                    Ok(()) => {
                        // Same path a manual edit takes: re-flow timing + re-embed.
                        reflow_and_reembed_after_edit(state, &id, &v.text).await;
                        ok_null()
                    }
                    Err(e) => err_response(&e),
                },
                Ok(None) => not_found(format!(
                    "no transcript version {idx} for recording {}",
                    id.as_str()
                )),
                Err(e) => err_response(&e),
            }
        }
        Request::SemanticSearch {
            query,
            limit,
            filter,
        } => {
            // Clamp the client-supplied limit so a huge value can't force an
            // unbounded result allocation and JSON serialization over the pipe.
            let limit = limit.min(MAX_SEARCH_RESULTS);
            // Clone the Arc and drop the read guard before embedding: ONNX
            // inference is CPU-bound and runs under a std Mutex inside the
            // embedder, so doing it inline would block this Tokio worker (and
            // every config-reload writer) for the whole inference. Hand it to
            // spawn_blocking, matching the ingest path in pipeline.rs.
            let embedder = state.embedder.read().await.as_ref().cloned();
            if let Some(embedder) = embedder {
                let q = query.clone();
                let embed_res = tokio::task::spawn_blocking(move || embedder.embed_query(&q)).await;
                match embed_res {
                    Ok(Ok(query_vec)) => match state
                        .catalog
                        .hybrid_search(
                            &query,
                            &query_vec,
                            limit,
                            SEMANTIC_MIN_RELEVANCE,
                            filter.as_ref(),
                        )
                        .await
                    {
                        Ok(results) => {
                            // Batch the recording fetch into one query + child
                            // queries instead of one `get` per result (up to
                            // MAX_SEARCH_RESULTS sequential round-trips), then
                            // re-join on id to keep the relevance order + scores.
                            let ids: Vec<_> = results.iter().map(|(id, _)| id.clone()).collect();
                            let by_id: std::collections::HashMap<String, _> = state
                                .catalog
                                .get_batch(&ids)
                                .await
                                .unwrap_or_default()
                                .into_iter()
                                .map(|r| (r.id.as_str().to_string(), r))
                                .collect();
                            let mut full_results = Vec::new();
                            for (id, score) in results {
                                if let Some(r) = by_id.get(id.as_str()) {
                                    full_results.push(serde_json::json!({
                                        "recording": r,
                                        "score": score,
                                    }));
                                }
                            }
                            Response::Ok(serde_json::Value::Array(full_results))
                        }
                        Err(e) => err_response(&e),
                    },
                    Ok(Err(e)) => Response::Err(IpcError {
                        kind: IpcErrorKind::Internal,
                        message: format!("embedding failed: {e}"),
                    }),
                    Err(e) => Response::Err(IpcError {
                        kind: IpcErrorKind::Internal,
                        message: format!("embedding task failed: {e}"),
                    }),
                }
            } else {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "Semantic search is not enabled or model is missing.".to_string(),
                })
            }
        }
        Request::MoreLikeThis { id, limit } => {
            // Clamp the client-supplied limit (see SemanticSearch) so it can't
            // force an unbounded allocation.
            let limit = limit.min(MAX_SEARCH_RESULTS);
            // No embedder needed: the source recording's stored vectors are the
            // query (that's the whole point — recall is free once indexed), so
            // this works even while the embedding model isn't loaded. The catalog
            // returns a clear "isn't indexed yet" error when the recording has no
            // vectors; forward it verbatim for the UI/CLI.
            match state
                .catalog
                .more_like_this(&id, limit, SEMANTIC_MIN_RELEVANCE)
                .await
            {
                Ok(results) => {
                    // Same `[{ recording, score }]` shape as `SemanticSearch` so
                    // clients reuse the relevance-chip rendering unchanged. Batch
                    // the fetch (one query + child queries) and re-join on id to
                    // keep the relevance order, rather than a `get` per result.
                    let ids: Vec<_> = results.iter().map(|(rec_id, _)| rec_id.clone()).collect();
                    let by_id: std::collections::HashMap<String, _> = state
                        .catalog
                        .get_batch(&ids)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|r| (r.id.as_str().to_string(), r))
                        .collect();
                    let mut full_results = Vec::new();
                    for (rec_id, score) in results {
                        if let Some(r) = by_id.get(rec_id.as_str()) {
                            full_results.push(serde_json::json!({
                                "recording": r,
                                "score": score,
                            }));
                        }
                    }
                    Response::Ok(serde_json::Value::Array(full_results))
                }
                Err(e) => err_response(&e),
            }
        }
        Request::Ask {
            request_id,
            query,
            top_k,
            filter,
        } => {
            // 1. Embedder gate (same shape as SemanticSearch): Ask must embed the
            //    question, so the model has to be loaded.
            let embedder = state.embedder.read().await.as_ref().cloned();
            let Some(embedder) = embedder else {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::InvalidConfig,
                    message: "Ask needs semantic search enabled and the embedding model loaded"
                        .into(),
                });
            };

            // 2. LLM provider gate. Reuse the cleanup connection's provider, but
            //    route it through `ondemand_connection`: the cleanup ENTRY can pin
            //    its own connection to `none` (the user doesn't want auto-cleanup),
            //    and the legacy fallback's `enabled` can be false when the cleanup
            //    STEP is toggled off — either way `provider()` returns None. Ask is
            //    a separate on-demand feature, so the helper force-enables and falls
            //    back to the global `[llm_post_process]` connection. Ask only borrows
            //    the provider/model/endpoint/key; its own grounded prompt is built
            //    in `ask.rs`, so the resolved prompt here is discarded.
            let cfg = state.config.load();
            let (llm_cfg, _cleanup_prompt) = crate::pipeline::cleanup_entry_config(&cfg);
            let llm_cfg = crate::pipeline::ondemand_connection(&state.llm, &cfg, llm_cfg);
            if state.llm.provider(&llm_cfg).is_none() {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::InvalidConfig,
                    message:
                        "no LLM provider configured for Ask — set a provider in Settings → Post-Processing"
                            .into(),
                });
            }

            let top_k = if top_k == 0 {
                ASK_DEFAULT_TOP_K
            } else {
                top_k.min(ASK_MAX_TOP_K)
            };

            // ACK immediately; the work runs detached and streams over
            // `DaemonEvent::AskActivity`. A failure after this ack (query-embed,
            // retrieval, generation) surfaces as a terminal AskActivity error,
            // never a swallow (see `ask::run_ask`).
            let task_state = state.clone();
            tokio::spawn(async move {
                crate::ask::run_ask(
                    &task_state,
                    embedder,
                    llm_cfg,
                    request_id,
                    query,
                    top_k,
                    filter,
                )
                .await;
            });
            ok_null()
        }
        Request::ReembedAll => {
            let cfg = state.config.load();
            if !cfg.semantic_search.enabled {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "semantic search is disabled — enable it before re-embedding".into(),
                })
            } else if state.embedder.read().await.is_none() {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "embedding model is not loaded (check the model path)".into(),
                })
            } else if state
                .reembed_in_flight
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_err()
            {
                Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: "a re-embed is already running — wait for it to finish".into(),
                })
            } else {
                // Re-embed the whole library with the current model, in place, one
                // recording at a time — never an upfront global wipe. Wiping first
                // (a single `clear_all_embeddings()`) means a crash, kill, or
                // model-unload between the clear and the end of the background loop
                // leaves the entire library permanently un-embedded with no
                // recovery.
                //
                // `embed_and_store` → `upsert_chunk_embeddings` replaces a single
                // recording's chunks atomically (DELETE-then-INSERT in one tx), so
                // each step swaps that recording's old-model vectors for new-model
                // ones with no gap. If the pass is interrupted, the recordings not
                // yet reached keep their old-model embeddings and stay searchable;
                // the worst case is a partly-migrated library, not a wiped one.
                // Returns immediately; the work runs in the background.
                let bg = state.clone();
                tokio::spawn(async move {
                    // Clear the in-flight flag on every exit path (including the
                    // early returns below) so a future ReembedAll isn't locked out.
                    struct InFlightGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);
                    impl Drop for InFlightGuard {
                        fn drop(&mut self) {
                            self.0.store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    let _in_flight = InFlightGuard(bg.reembed_in_flight.clone());
                    if bg.embedder.read().await.is_none() {
                        return;
                    }
                    // Every recording with a transcript (no chunk-presence filter:
                    // we want to overwrite existing vectors, not skip them).
                    let filter = phoneme_core::ListFilter::default();
                    match bg.catalog.list(&filter).await {
                        Ok(records) => {
                            // Count only embeddable rows (non-empty transcript), so
                            // the progress/completion log reads N/N on a full pass
                            // rather than N/all-rows (the skipped empties confused it).
                            let total = records
                                .iter()
                                .filter(|r| r.transcript.as_ref().is_some_and(|t| !t.is_empty()))
                                .count();
                            tracing::info!(
                                "re-embedding {total} recordings in place with the current model"
                            );
                            let mut done = 0usize;
                            for r in records {
                                let Some(t) = r.transcript.as_ref().filter(|t| !t.is_empty())
                                else {
                                    continue;
                                };
                                // Re-acquire the embedder per item: this loop runs
                                // for minutes on a big library, and holding the
                                // read guard across it blocks every config-reload
                                // write. Clone the Arc, drop the guard, then embed,
                                // so writers interleave between items. If it's gone
                                // mid-run (semantic search turned off) we stop;
                                // recordings already done keep their fresh vectors
                                // and the rest keep their old (still-searchable)
                                // ones.
                                let embedder = bg.embedder.read().await.as_ref().cloned();
                                let Some(embedder) = embedder else {
                                    tracing::info!(
                                        "re-embed stopped after {done}/{total}: embedding model unloaded"
                                    );
                                    return;
                                };
                                crate::pipeline::embed_and_store(embedder, &bg.catalog, &r.id, t)
                                    .await;
                                done += 1;
                            }
                            tracing::info!("re-embed complete ({done}/{total} recordings)");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "re-embed: failed to list recordings")
                        }
                    }
                });
                ok_null()
            }
        }
        Request::DeleteRecording { id, keep_audio } => match state.catalog.get(&id).await {
            Ok(Some(r)) => {
                // Delete the catalog row first. If it fails, report the error and
                // leave the audio alone — otherwise the client sees `Ok`, the WAV
                // is gone, and the row lingers pointing at nothing.
                if let Err(e) = state.catalog.delete(&id).await {
                    return Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: format!("catalog delete failed: {e}"),
                    });
                }
                if !keep_audio {
                    // Defense in depth: only ever unlink files that live under our
                    // own audio directory, and never a symlink (whose target could
                    // be anywhere — the lexical guard can't see through it). The
                    // path comes from the catalog (which we control), but guarding
                    // here means a poisoned or hand-edited row can't turn a delete
                    // into "rm any file".
                    if !audio_path_is_ours(&r.audio_path, &state.paths.audio_dir) {
                        tracing::warn!(
                            path = %r.audio_path,
                            "refusing to delete audio file outside the audio directory"
                        );
                    } else if is_symlink(&r.audio_path).await {
                        tracing::warn!(
                            path = %r.audio_path,
                            "refusing to delete audio entry that is a symlink"
                        );
                    } else {
                        // Best-effort — the file may already be gone. Log, don't fail.
                        if let Err(e) = tokio::fs::remove_file(&r.audio_path).await {
                            tracing::warn!(
                                path = %r.audio_path,
                                error = %e,
                                "audio file removal failed"
                            );
                        }
                    }
                }
                // If this was a meeting track and it was the last one, drop the
                // whole-meeting digest too: it lives in `meeting_digests` keyed by
                // `meeting_id` with no FK to `recordings`, so `Catalog::delete`
                // (which only cascades FK child tables) never reaches it. Mirrors
                // the `DeleteSession` cleanup. Best-effort: a leftover digest row is
                // harmless, so a cleanup miss only warns and never fails the delete.
                if let Some(mid) = r.meeting_id.as_deref() {
                    match state.catalog.list_by_meeting(mid).await {
                        Ok(remaining) if remaining.is_empty() => {
                            if let Err(e) = state.catalog.delete_meeting_digest(mid).await {
                                tracing::warn!(session = %mid, error = %e, "recording delete: digest cleanup failed");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(session = %mid, error = %e, "recording delete: meeting track lookup failed");
                        }
                    }
                }
                state.events.emit(DaemonEvent::RecordingDeleted { id });
                ok_null()
            }
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
        },
        Request::DeleteSession {
            meeting_id,
            keep_audio,
        } => match state.catalog.list_by_meeting(&meeting_id).await {
            Ok(tracks) if !tracks.is_empty() => {
                // Delete each track exactly like `DeleteRecording`: row first (an
                // error there leaves that track's audio untouched), then the WAV
                // unless keep_audio, and only when it's under our audio dir. One
                // track failing doesn't abandon the rest; each removed track emits
                // its own RecordingDeleted so every view drops it.
                let total = tracks.len();
                let mut deleted = 0usize;
                let mut last_err = None;
                for r in tracks {
                    if let Err(e) = state.catalog.delete(&r.id).await {
                        // Error, not warn: a track that survives the delete is a
                        // real partial failure the client must hear about (it's
                        // reflected in the error response below), not a routine
                        // best-effort miss like an already-gone audio file.
                        tracing::error!(id = %r.id, session = %meeting_id, error = %e, "session delete: track row delete failed");
                        last_err = Some(e);
                        continue;
                    }
                    if !keep_audio {
                        if !audio_path_is_ours(&r.audio_path, &state.paths.audio_dir) {
                            tracing::warn!(path = %r.audio_path, "refusing to delete audio file outside the audio directory");
                        } else if is_symlink(&r.audio_path).await {
                            tracing::warn!(path = %r.audio_path, "refusing to delete audio entry that is a symlink");
                        } else if let Err(e) = tokio::fs::remove_file(&r.audio_path).await {
                            tracing::warn!(path = %r.audio_path, error = %e, "session delete: audio removal failed");
                        }
                    }
                    state
                        .events
                        .emit(DaemonEvent::RecordingDeleted { id: r.id.clone() });
                    deleted += 1;
                }
                // Drop the whole-meeting digest too — its table is keyed by
                // meeting_id with no FK to recordings, so deleting the tracks
                // doesn't cascade it. Best-effort: a leftover digest row is
                // harmless, so a cleanup error never fails the session delete.
                if let Err(e) = state.catalog.delete_meeting_digest(&meeting_id).await {
                    tracing::warn!(session = %meeting_id, error = %e, "session delete: digest cleanup failed");
                }
                // Report any failure to the client instead of silently returning
                // Ok when some tracks were removed, matching `DeleteRecording`,
                // which always surfaces a delete error. The removed tracks already
                // emitted RecordingDeleted (so views drop them); a partial failure
                // returns an error carrying the deleted/total counts so the client
                // knows the session is only partly gone.
                match last_err {
                    None => ok_null(),
                    Some(e) if deleted == 0 => err_response(&e),
                    Some(e) => Response::Err(IpcError {
                        kind: error_to_kind(&e),
                        message: format!(
                            "session partly deleted: {deleted}/{total} tracks removed, last error: {e}"
                        ),
                    }),
                }
            }
            Ok(_) => not_found(format!("meeting {meeting_id} not found")),
            Err(e) => err_response(&e),
        },
        Request::UpdateTranscript { id, text } => {
            match state.catalog.update_user_transcript(&id, &text).await {
                Ok(()) => {
                    reflow_and_reembed_after_edit(state, &id, &text).await;
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        // S6: literal find-and-replace across the live transcript. The catalog
        // does the replacement and persist (the no-op and NotFound cases are
        // handled there); we run the same re-flow/re-embed/event upkeep as a hand
        // edit, but only when something actually changed, so a zero-match is a
        // true no-op.
        Request::FindReplace {
            id,
            find,
            replace,
            case_sensitive,
        } => match state
            .catalog
            .find_replace_transcript(&id, &find, &replace, case_sensitive)
            .await
        {
            Ok(outcome) => {
                if outcome.replaced > 0 {
                    reflow_and_reembed_after_edit(state, &id, &outcome.transcript).await;
                }
                Response::Ok(serde_json::json!({ "replaced": outcome.replaced }))
            }
            Err(e) => err_response(&e),
        },
        Request::FindReplaceLibrary {
            find,
            replace,
            case_sensitive,
        } => match state
            .catalog
            .find_replace_transcript_library(&find, &replace, case_sensitive)
            .await
        {
            Ok(outcome) => {
                // The catalog already persisted every replacement and counted
                // them, so answer with the counts now and run the per-recording
                // re-flow/re-embed upkeep off the connection. embed_and_store is
                // ONNX-heavy, so a library-wide replace touching many recordings
                // would otherwise stall the single-connection bridge for the whole
                // pass (handler invariant up top). Mirrors rerun_cleanup: spawn,
                // report progress via DaemonEvent (one TranscriptUpdated per changed
                // recording, just emitted from the task). Zero-match recordings were
                // skipped by the catalog and never appear in `changed`.
                let response = Response::Ok(serde_json::json!({
                    "recordings_changed": outcome.recordings_changed,
                    "total_replacements": outcome.total_replacements,
                    "failed": outcome.failed,
                }));
                let task_state = state.clone();
                let changed = outcome.changed;
                tokio::spawn(async move {
                    for (id, transcript) in &changed {
                        reflow_and_reembed_after_edit(&task_state, id, transcript).await;
                    }
                });
                response
            }
            Err(e) => err_response(&e),
        },
        Request::UpdateMeetingName { meeting_id, name } => {
            match state
                .catalog
                .update_meeting_name(&meeting_id, name.as_deref())
                .await
            {
                Ok(()) => {
                    state
                        .events
                        .emit(DaemonEvent::MeetingNameUpdated { meeting_id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::GetOriginalTranscript { id } => {
            catalog_call!(state.catalog.get_original_transcript(&id).await => serialize)
        }
        Request::GetCleanTranscript { id } => {
            catalog_call!(state.catalog.get_clean_transcript(&id).await => serialize)
        }
        Request::SetFavorite { id, favorite } => {
            // Emit the generic recording-refresh event on success so OTHER
            // subscribers (a second window, `phoneme watch`, the MCP bridge) see
            // the favorite flip — the toggling view already has the new state, but
            // it isn't the only client. Reuses TranscriptUpdated (a re-fetch trigger
            // every library view already handles) rather than a new event.
            match state.catalog.set_favorite(&id, favorite).await {
                Ok(()) => {
                    state
                        .events
                        .emit(DaemonEvent::TranscriptUpdated { id: id.clone() });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::SetPinned { id, pinned } => {
            match state.catalog.set_pinned(&id, pinned).await {
                Ok(()) => {
                    state
                        .events
                        .emit(DaemonEvent::TranscriptUpdated { id: id.clone() });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::SetRecordingTitle { id, title } => {
            // A blank title means "clear back to auto", same as None. `Some` marks
            // the title user-owned, so the pipeline never overwrites it; `None`
            // resets ownership and the next run generates a fresh one.
            let title = title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
            let is_auto = title.is_none();
            // A user/CLI title write carries no model, so pass `None`, which also
            // clears any stale auto-title model so a user title never shows one.
            match state
                .catalog
                .set_title(&id, title.as_deref(), is_auto, None)
                .await
            {
                Ok(true) => {
                    // Same event a transcript edit emits — open views re-fetch
                    // the recording and pick the new title up.
                    state.events.emit(DaemonEvent::TranscriptUpdated { id });
                    ok_null()
                }
                Ok(false) => not_found(format!("no recording {}", id.as_str())),
                Err(e) => err_response(&e),
            }
        }
        Request::SuggestTags { id } => {
            // On-demand tag suggestions (the UI's ✨ Suggest button). Runs the
            // same step as the auto pipeline, regardless of `auto_tag.auto`.
            let cfg = state.config.load();
            match state.catalog.get(&id).await {
                Ok(Some(rec)) => {
                    let transcript = rec.transcript.unwrap_or_default();
                    if transcript.trim().is_empty() {
                        Response::Err(IpcError {
                            kind: IpcErrorKind::InvalidConfig,
                            message: "recording has no transcript to tag yet".into(),
                        })
                    } else {
                        // Read the migrated `tags` enrichment entry (provider,
                        // model, prompt) so editing it in the Playbook changes what
                        // an on-demand re-run does — the Playbook is the source of
                        // truth. Fall back to the legacy `[auto_tag]` section when
                        // no such entry exists (a user deleted it).
                        match crate::pipeline::entry_config_for_target(&cfg, "tags") {
                            Some((llm_cfg, prompt)) => {
                                crate::pipeline::suggest_tags_with(
                                    state,
                                    &cfg,
                                    &id,
                                    &transcript,
                                    llm_cfg,
                                    &prompt,
                                )
                                .await;
                            }
                            None => {
                                crate::pipeline::suggest_tags(state, &cfg, &id, &transcript).await;
                            }
                        }
                        ok_null()
                    }
                }
                Ok(None) => not_found(format!("no recording {}", id.as_str())),
                Err(e) => err_response(&e),
            }
        }
        Request::SuggestEntities { id } => {
            // On-demand entity extraction (the UI's 🔎 Extract button). Runs the
            // same step as the auto pipeline, regardless of recipe membership.
            let cfg = state.config.load();
            match state.catalog.get(&id).await {
                Ok(Some(rec)) => {
                    let transcript = rec.transcript.unwrap_or_default();
                    if transcript.trim().is_empty() {
                        Response::Err(IpcError {
                            kind: IpcErrorKind::InvalidConfig,
                            message: "recording has no transcript to extract entities from yet"
                                .into(),
                        })
                    } else {
                        // Read the migrated `entities` enrichment entry (provider,
                        // model, prompt) so editing it in the Playbook changes what
                        // an on-demand re-run does. Fall back to the built-in
                        // default entity prompt when no such entry exists.
                        let probe = match crate::pipeline::entry_config_for_target(&cfg, "entities")
                        {
                            Some((c, _)) => c,
                            None => crate::pipeline::entities_llm_config(&cfg),
                        };
                        // Match what the extractor will use: a "none" entry falls
                        // back to the global LLM for this on-demand action.
                        let probe = crate::pipeline::ondemand_connection(&state.llm, &cfg, probe);
                        if state.llm.provider(&probe).is_none() {
                            no_provider_response(&probe.provider)
                        } else {
                            crate::pipeline::extract_entities(state, &cfg, &id, &transcript).await;
                            ok_null()
                        }
                    }
                }
                Ok(None) => not_found(format!("no recording {}", id.as_str())),
                Err(e) => err_response(&e),
            }
        }
        Request::SuggestChapters { id } => {
            // On-demand auto-chapter generation (the UI's ✨ Generate-chapters
            // button). Runs the same step as the auto pipeline, regardless of recipe
            // membership; awaits the model like SuggestEntities. The step itself
            // short-circuits to a clean no-op when the recording has no segments (no
            // timing to chapter), so that case is not an error here.
            let cfg = state.config.load();
            match state.catalog.get(&id).await {
                Ok(Some(rec)) => {
                    let transcript = rec.transcript.unwrap_or_default();
                    if transcript.trim().is_empty() {
                        Response::Err(IpcError {
                            kind: IpcErrorKind::InvalidConfig,
                            message: "recording has no transcript to chapter yet".into(),
                        })
                    } else {
                        // Read the migrated `chapters` enrichment entry so editing it
                        // in the Playbook changes what an on-demand re-run does; fall
                        // back to the built-in default chapter prompt when absent.
                        // (A provider that's set but a recording with no segments is a
                        // clean no-op inside the step, not an error here.)
                        let probe = match crate::pipeline::entry_config_for_target(&cfg, "chapters")
                        {
                            Some((c, _)) => c,
                            None => crate::pipeline::chapters_llm_config(&cfg),
                        };
                        // Match what the extractor will use: a "none" entry falls
                        // back to the global LLM for this on-demand action.
                        let probe = crate::pipeline::ondemand_connection(&state.llm, &cfg, probe);
                        if state.llm.provider(&probe).is_none() {
                            no_provider_response(&probe.provider)
                        } else {
                            crate::pipeline::extract_chapters(state, &cfg, &id, &transcript).await;
                            ok_null()
                        }
                    }
                }
                Ok(None) => not_found(format!("no recording {}", id.as_str())),
                Err(e) => err_response(&e),
            }
        }
        Request::SuggestTasks { id } => {
            // On-demand task extraction (the UI's Extract-tasks button). Runs the
            // same step as the auto pipeline, regardless of recipe membership.
            let cfg = state.config.load();
            match state.catalog.get(&id).await {
                Ok(Some(rec)) => {
                    let transcript = rec.transcript.unwrap_or_default();
                    if transcript.trim().is_empty() {
                        Response::Err(IpcError {
                            kind: IpcErrorKind::InvalidConfig,
                            message: "recording has no transcript to extract tasks from yet".into(),
                        })
                    } else {
                        // Read the migrated `tasks` enrichment entry (provider,
                        // model, prompt) so editing it in the Playbook changes what
                        // an on-demand re-run does. Fall back to the built-in
                        // default task prompt when no such entry exists.
                        let probe = match crate::pipeline::entry_config_for_target(&cfg, "tasks") {
                            Some((c, _)) => c,
                            None => crate::pipeline::tasks_llm_config(&cfg),
                        };
                        // Match what the extractor will use: a "none" entry falls
                        // back to the global LLM for this on-demand action.
                        let probe = crate::pipeline::ondemand_connection(&state.llm, &cfg, probe);
                        if state.llm.provider(&probe).is_none() {
                            no_provider_response(&probe.provider)
                        } else {
                            crate::pipeline::extract_tasks(state, &cfg, &id, &transcript).await;
                            ok_null()
                        }
                    }
                }
                Ok(None) => not_found(format!("no recording {}", id.as_str())),
                Err(e) => err_response(&e),
            }
        }
        // Like `GetSegments`, an unknown id yields an empty list (not `NotFound`):
        // "no chapters" is a normal state (the recording has no timing to chapter,
        // or the auto-chapter step never ran).
        Request::GetChapters { id } => {
            catalog_call!(state.catalog.chapters_for(&id).await => serialize)
        }
        // Per-recording entities, the cheap read the detail-pane chips use instead
        // of pulling the whole `GetRecording` row. Like `GetChapters`, an unknown id
        // yields an empty list (not `NotFound`) — `list_entities` is the same N+1
        // child query that fills `Recording::entities`.
        Request::GetEntities { id } => {
            catalog_call!(state.catalog.list_entities(&id).await => serialize)
        }
        Request::SetTaskDone { id, task_id, done } => {
            // Toggle one task's done flag, then refresh open views. `not_found`
            // when the task id matches no row (a stale UI / bad id).
            match state.catalog.set_task_done(&id, task_id, done).await {
                Ok(0) => not_found(format!("no task {task_id}")),
                Ok(_) => {
                    state.events.emit(DaemonEvent::TasksUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::AddTask { id, text, due_hint } => {
            // Add a user ('manual') task; it survives later re-extraction.
            match state
                .catalog
                .add_task(&id, &text, due_hint.as_deref())
                .await
            {
                Ok(_) => {
                    state.events.emit(DaemonEvent::TasksUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::UpdateTask {
            id,
            task_id,
            text,
            due_hint,
        } => {
            // Edit one task's text/due, scoped to its recording.
            match state
                .catalog
                .update_task(&id, task_id, &text, due_hint.as_deref())
                .await
            {
                Ok(0) => not_found(format!("no task {task_id}")),
                Ok(_) => {
                    state.events.emit(DaemonEvent::TasksUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::DeleteTask { id, task_id } => {
            match state.catalog.delete_task(&id, task_id).await {
                Ok(0) => not_found(format!("no task {task_id}")),
                Ok(_) => {
                    state.events.emit(DaemonEvent::TasksUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::ReorderTasks { id, task_ids } => {
            // Position-by-id rewrite of sort_order; ids outside the recording are
            // ignored by the scoped UPDATE.
            match state.catalog.reorder_tasks(&id, &task_ids).await {
                Ok(()) => {
                    state.events.emit(DaemonEvent::TasksUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::AddEntity { id, kind, value } => {
            match state.catalog.add_entity(&id, &kind, &value).await {
                Ok(_) => {
                    state.events.emit(DaemonEvent::EntitiesUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::UpdateEntity {
            id,
            kind,
            value,
            new_kind,
            new_value,
        } => {
            match state
                .catalog
                .update_entity(&id, &kind, &value, &new_kind, &new_value)
                .await
            {
                Ok(_) => {
                    state.events.emit(DaemonEvent::EntitiesUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::DeleteEntity { id, kind, value } => {
            match state.catalog.delete_entity(&id, &kind, &value).await {
                Ok(_) => {
                    state.events.emit(DaemonEvent::EntitiesUpdated { id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::MergeEntities {
            kind,
            from_values,
            to_value,
        } => match state
            .catalog
            .merge_entities(&kind, &from_values, &to_value)
            .await
        {
            Ok(renamed) => {
                state.events.emit(DaemonEvent::EntitiesMerged { renamed });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::ApproveTagSuggestion { id, name } => {
            // Create-or-fetch the tag, attach it, then drop the suggestion.
            match state.catalog.add_tag(&name, None).await {
                Ok(tag) => match state.catalog.attach_tag(&id, tag.id).await {
                    Ok(()) => {
                        state.events.emit(DaemonEvent::TagAttached {
                            tag_id: tag.id,
                            recording_id: Some(id.clone()),
                        });
                        if let Ok(Some(rec)) = state.catalog.get(&id).await {
                            let rest: Vec<String> = rec
                                .tag_suggestions
                                .into_iter()
                                .filter(|n| !n.eq_ignore_ascii_case(&name))
                                .collect();
                            if let Err(e) = state.catalog.set_tag_suggestions(&id, &rest).await {
                                tracing::warn!(error = %e, "failed to drop approved tag suggestion");
                            }
                        }
                        state.events.emit(DaemonEvent::TagSuggestionsUpdated { id });
                        Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                    }
                    Err(e) => err_response(&e),
                },
                Err(e) => err_response(&e),
            }
        }
        Request::ClearAllTagSuggestions => match state.catalog.clear_all_tag_suggestions().await {
            Ok(cleared) => {
                state
                    .events
                    .emit(DaemonEvent::AllTagSuggestionsCleared { cleared });
                Response::Ok(serde_json::json!({ "cleared": cleared }))
            }
            Err(e) => err_response(&e),
        },
        Request::DismissTagSuggestion { id, name } => match state.catalog.get(&id).await {
            Ok(Some(rec)) => {
                let rest: Vec<String> = rec
                    .tag_suggestions
                    .into_iter()
                    .filter(|n| !n.eq_ignore_ascii_case(&name))
                    .collect();
                match state.catalog.set_tag_suggestions(&id, &rest).await {
                    Ok(()) => {
                        state.events.emit(DaemonEvent::TagSuggestionsUpdated { id });
                        ok_null()
                    }
                    Err(e) => err_response(&e),
                }
            }
            Ok(None) => not_found(format!("no recording {}", id.as_str())),
            Err(e) => err_response(&e),
        },
        Request::UpdateNotes { id, notes } => match state.catalog.update_notes(&id, &notes).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::NotesUpdated { id });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::SetSpeakerName {
            id,
            speaker_label,
            name,
        } => {
            // Speaker indices are 1-based (`[Speaker 1]`, …), so reject a
            // non-positive label rather than writing a row that can never match a
            // marker.
            if speaker_label < 1 {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("invalid speaker label {speaker_label} (must be >= 1)"),
                });
            }
            match state
                .catalog
                .set_speaker_name(&id, speaker_label, &name)
                .await
            {
                Ok(()) => {
                    // Implicit enrollment (#9): naming a speaker folds its captured
                    // voiceprint into the cross-recording library; clearing the
                    // name un-enrolls it. Best-effort, and a no-op when no
                    // voiceprint was captured (cloud-diarized recordings) —
                    // recognition is a convenience, never a reason to fail the
                    // rename.
                    let enrolled_id = if name.trim().is_empty() {
                        if let Err(e) = state
                            .catalog
                            .unenroll_speaker(id.as_str(), speaker_label)
                            .await
                        {
                            tracing::warn!(id = %id.as_str(), label = speaker_label, "voiceprint unenroll failed: {e}");
                        }
                        None
                    } else {
                        match state
                            .catalog
                            .enroll_speaker(id.as_str(), speaker_label, &name)
                            .await
                        {
                            Ok(nid) => nid,
                            Err(e) => {
                                tracing::warn!(id = %id.as_str(), label = speaker_label, "voiceprint enroll failed: {e}");
                                None
                            }
                        }
                    };
                    state
                        .events
                        .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });

                    // Name propagation (V5): when the speaker actually enrolled
                    // into the library, optionally back-fill that name onto the
                    // same unnamed voice in other recordings, per policy. Naming
                    // never fails over propagation — it's a convenience layered on
                    // top, so any error here is logged and swallowed.
                    let cfg = state.config.load();
                    let propagation = match enrolled_id {
                        Some(nid) if cfg.diarization.recognize_speakers => {
                            speaker_name_propagation(state, &nid, &cfg.diarization).await
                        }
                        // No enrollment (cleared name / cloud-diarized / recognition
                        // off) → nothing to propagate.
                        _ => serde_json::json!({ "policy": "off", "applied": 0, "candidates": [] }),
                    };
                    Response::Ok(serde_json::json!({ "propagation": propagation }))
                }
                Err(e) => err_response(&e),
            }
        }
        // ── In-recording speaker correction (U1) ─────────────────────────
        // Each op keeps `transcript_segments` authoritative and rebuilds the prose
        // `[Speaker N]:` markers in one transaction (catalog side), so every view
        // the user sees agrees. They emit `SpeakerNameUpdated` so open clients
        // refresh the recording (segments, names, prose).
        Request::ReassignSegmentSpeaker { id, idx, new_label } => {
            match state.catalog.reassign_segment(&id, idx, new_label).await {
                Ok(()) => {
                    clear_cleaned_timing(state, &id).await;
                    state
                        .events
                        .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::MergeSpeakers {
            id,
            from_label,
            into_label,
        } => match state
            .catalog
            .merge_speakers(&id, from_label, into_label)
            .await
        {
            Ok(()) => {
                clear_cleaned_timing(state, &id).await;
                state
                    .events
                    .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::SplitSpeaker {
            id,
            label,
            segment_idxs,
            new_label,
        } => match state
            .catalog
            .split_speaker(&id, label, &segment_idxs, new_label)
            .await
        {
            Ok(()) => {
                clear_cleaned_timing(state, &id).await;
                state
                    .events
                    .emit(DaemonEvent::SpeakerNameUpdated { id: id.clone() });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::RecognizeSpeakers { id } => {
            let cfg = state.config.load();
            if cfg.diarization.recognize_speakers {
                // V2 score normalization: when off (default), use the raw cosine
                // bar; when on, switch to the z-score bar.
                let (mode, threshold) = voiceprint_scorer(&cfg.diarization);
                // #243: scope the library to the current embedding model so a
                // models_dir swap can't match against incompatible centroids.
                let embedding_model =
                    phoneme_core::diarization::embedding_model_id(&cfg.diarization);
                catalog_call!(
                    state
                        .catalog
                        .recognize_speakers_for(id.as_str(), threshold, mode, &embedding_model)
                        .await => serialize
                )
            } else {
                serialize_response(Vec::<phoneme_core::types::SpeakerSuggestion>::new())
            }
        }
        Request::DismissSpeakerSuggestion { id, speaker_label } => catalog_call!(
            state
                .catalog
                .dismiss_speaker_suggestion(id.as_str(), speaker_label)
                .await => ok_null
        ),
        Request::ListNamedVoices => {
            catalog_call!(state.catalog.list_named_voices().await => serialize)
        }
        Request::RenameNamedVoice { id, name } => {
            catalog_call!(state.catalog.rename_named_voice(&id, &name).await => ok_null)
        }
        Request::MergeNamedVoices { from_id, into_id } => {
            catalog_call!(state.catalog.merge_named_voices(&from_id, &into_id).await => json "merged")
        }
        Request::ForgetNamedVoice { id } => {
            catalog_call!(state.catalog.forget_named_voice(&id).await => json "removed")
        }
        Request::UndoForgetNamedVoice { id } => {
            catalog_call!(state.catalog.undo_forget(&id).await => json "restored")
        }
        Request::ImportRecording {
            path,
            recipe_id,
            ext_ref,
        } => import_recording(state, path, recipe_id, ext_ref).await,
        Request::ExportClip {
            id,
            start_ms,
            end_ms,
            out_path,
        } => export_clip(state, id, start_ms, end_ms, out_path).await,
        Request::EditRecording {
            id,
            keep_ranges,
            new_recording,
        } => edit_recording(state, id, keep_ranges, new_recording).await,
        Request::ReimportFromDisk { dry_run } => reimport_from_disk(state, dry_run).await,
        Request::RebuildCatalog => {
            // Refuse while capture is live — clearing the table would orphan a
            // row still being written. The user must stop recording first.
            if state.recorder.is_busy().await {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::AlreadyRecording,
                    message: "can't rebuild the catalog while recording — stop the recording first"
                        .into(),
                });
            }
            // Clear every row (cascade takes tags/segments/words/embeddings), then
            // re-import from disk: with the table empty, every WAV is an orphan, so
            // the existing reimport re-links them all as Queued. The WAVs are never
            // touched, so this is recoverable to the audio even though transcripts
            // and tags are lost (and re-derived by re-transcription).
            match state.catalog.clear_all_recordings().await {
                Ok(removed) => {
                    tracing::info!(
                        removed,
                        "catalog rebuild: cleared all rows; re-importing from disk"
                    );
                    reimport_from_disk(state, false).await
                }
                Err(e) => err_response(&e),
            }
        }
        Request::RetranscribeRecording {
            id,
            model,
            run_hooks,
            post_process,
            all_overrides,
            recipe_id,
        } => match state.catalog.get(&id).await {
            Ok(Some(r)) => {
                // A per-recording model override is never written into the
                // process-global config. Writing it there makes the whisper
                // supervisor (which polls the global config) restart the server,
                // and the queue worker's blanket post-run reload restart it again —
                // a thrash that mass-fails other queued/preview jobs reading the
                // same global config (#49). Instead we record the requested model
                // against this recording id; the pipeline applies it to that one
                // job only (a single serialized server model-swap for the local
                // bundled backend, or a per-job config clone for cloud backends),
                // then restores. See `pipeline::run`.
                if let Some(m) = model {
                    let m = m.trim();
                    if m.is_empty() {
                        // Empty means "use the configured model"; clear any stale
                        // request so a prior override can't leak onto this run.
                        state
                            .pending_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                    } else {
                        state
                            .pending_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(id.clone(), m.to_string());
                    }
                }
                // One-time recipe override (Re-run → "Run with recipe"): record the
                // chosen Playbook recipe against this id; `pipeline::run` claims it
                // from `pending_recipe` and resolves that chain instead of the
                // global `default`. Empty clears any stale request, like the model.
                if let Some(rid) = recipe_id {
                    let rid = rid.trim();
                    let mut map = state
                        .pending_recipe
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if rid.is_empty() {
                        map.remove(&id);
                    } else {
                        map.insert(id.clone(), rid.to_string());
                    }
                }
                // The one-time LLM/hook overrides (hooks toggle, post-processing
                // opt-out, and the Re-run → "All" cleanup/summary/title values) are
                // also recorded per-recording, never written into the
                // process-global config. A temp-global write here races a
                // concurrent ReloadConfig: it could be clobbered, or leak its
                // forced-on pipeline onto another queued job. `pipeline::run`
                // applies these to this job's config clone only, mirroring the
                // per-recording model override above.
                let rerun = crate::app_state::PendingRerun {
                    run_hooks,
                    post_process,
                    all_overrides,
                };
                {
                    let mut map = state
                        .pending_all_overrides
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if rerun.is_empty() {
                        // Clear any stale request so a prior re-run's overrides
                        // can't leak onto this plain retranscribe.
                        map.remove(&id);
                    } else {
                        map.insert(id.clone(), rerun);
                    }
                }
                let payload = HookPayload {
                    id: r.id,
                    timestamp: r.started_at,
                    transcript: String::new(),
                    audio_path: r.audio_path,
                    duration_ms: r.duration_ms,
                    model: String::new(),
                    metadata: HookMetadata::current(),
                };
                match state.inbox.enqueue(&payload).await {
                    Ok(()) => {
                        // Queued, not Transcribing: it waits behind the serial
                        // inbox; the pipeline flips it to Transcribing when the
                        // worker claims it.
                        if let Err(e) = state
                            .catalog
                            .update_status(&id, RecordingStatus::Queued)
                            .await
                        {
                            tracing::error!("failed to update status to queued: {e}");
                        }
                        ok_null()
                    }
                    Err(e) => {
                        // Enqueue failed: this job never reaches `pipeline::run`
                        // (the sole place these per-recording ledgers are otherwise
                        // claimed), so the entries we just stashed would leak keyed
                        // by this id. Drop them on this terminal path — the "removed
                        // on every terminal path" invariant — recovering from a
                        // poisoned lock the way the other pending_* sites do.
                        // `pending_focused_app` isn't populated for a retranscribe,
                        // but a defensive remove keeps the contract airtight.
                        state
                            .pending_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        state
                            .pending_all_overrides
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        state
                            .pending_recipe
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        state
                            .pending_focused_app
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        err_response(&e)
                    }
                }
            }
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
        },
        Request::RefireHook { id, command } => match state.catalog.get(&id).await {
            Ok(Some(r)) if r.transcript.is_some() => {
                let payload = HookPayload {
                    id: r.id,
                    timestamp: r.started_at,
                    transcript: r.transcript.unwrap_or_default(),
                    audio_path: r.audio_path,
                    duration_ms: r.duration_ms,
                    model: r.model.unwrap_or_default(),
                    metadata: HookMetadata::current(),
                };
                let cfg = state.config.load();
                let timeout = std::time::Duration::from_secs(cfg.hook.timeout_secs);
                // The "configured hooks" are the Hook-step commands (where
                // `migrate_hooks` moved `[hook].commands`), each with the Phoneme
                // path tokens expanded — the allowlist semantics the rest of this
                // arm relies on. We take the UNION across every recipe, not just
                // `default`: a recording can be produced by a per-hotkey recipe
                // override or a "Run with recipe" re-run, and the row doesn't
                // record which recipe ran, so building the allowlist from `default`
                // alone would reject a command that legitimately fired on this
                // recording. Webhook-only Hook steps have no command and are
                // skipped, since `RefireHook` only re-runs commands.
                let configured: Vec<String> = {
                    use phoneme_core::config::{expand_cmd, PlaybookKind};
                    let mut cmds = Vec::new();
                    for recipe in &cfg.recipes {
                        for step_id in &recipe.steps {
                            if let Some(e) = cfg.playbook.iter().find(|e| &e.id == step_id) {
                                let c = e.hook.command.trim();
                                if e.kind == PlaybookKind::Hook && !c.is_empty() {
                                    let expanded = expand_cmd(c);
                                    if !cmds.contains(&expanded) {
                                        cmds.push(expanded);
                                    }
                                }
                            }
                        }
                    }
                    cmds
                };
                drop(cfg);
                let commands = if let Some(cmd) = command {
                    // Security (S-C2): a caller may only re-fire a command already
                    // in the configured hook allowlist, never an arbitrary command
                    // handed in over IPC. The UI only ever sends a command it picked
                    // from this same list, so legitimate flows are intact.
                    if !hook_command_allowed(&cmd, &configured) {
                        return Response::Err(IpcError {
                            kind: IpcErrorKind::Internal,
                            message: "refire command is not in the configured hook allowlist"
                                .into(),
                        });
                    }
                    vec![cmd]
                } else {
                    configured
                };
                // Run the hook off the IPC connection. A hook can take up to its
                // full timeout (30s default); running it inline would freeze the
                // connection, and with it the single-connection Tauri bridge,
                // stalling every other UI request. The outcome is reported via
                // DaemonEvents, exactly like the queue pipeline.
                //
                // Deliberately no re-enqueue (unlike `RetranscribeRecording`): the
                // queue pipeline always re-transcribes first, which would overwrite
                // a user's manual transcript edit. `RefireHook` re-runs only the
                // hook against the stored transcript.
                let task_state = state.clone();
                tokio::spawn(async move {
                    let hook_id = payload.id.clone();
                    task_state.events.emit(DaemonEvent::HookStarted {
                        id: hook_id.clone(),
                    });
                    // Surface this re-run in the queue as an active "Running hook…" item.
                    task_state.events.emit(DaemonEvent::PipelineStageChanged {
                        id: hook_id.clone(),
                        stage: PipelineStage::RunningHook,
                    });

                    // Mirror pipeline::run: execute every configured hook in
                    // order, stopping at the first non-zero exit, and record the
                    // last command that ran.
                    let mut final_exit_code = 0;
                    let mut total_duration = 0;
                    let mut last_cmd = String::new();
                    let mut hook_error: Option<String> = None;
                    for cmd in &commands {
                        let trimmed = cmd.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let runner = HookRunner::new(trimmed.to_string(), timeout);
                        match runner.run(&payload).await {
                            Ok(result) => {
                                final_exit_code = result.exit_code;
                                total_duration += result.duration_ms;
                                last_cmd = cmd.clone();
                                if result.exit_code != 0 {
                                    break;
                                }
                            }
                            Err(e) => {
                                hook_error = Some(e.to_string());
                                break;
                            }
                        }
                    }

                    if let Some(error) = hook_error {
                        if let Err(e) = task_state
                            .catalog
                            .update_status(&hook_id, RecordingStatus::HookFailed)
                            .await
                        {
                            tracing::error!("failed to update status to hook_failed: {e}");
                        }
                        task_state
                            .events
                            .emit(DaemonEvent::HookFailed { id: hook_id, error });
                    } else {
                        if let Err(e) = task_state
                            .catalog
                            .update_hook_result(
                                &hook_id,
                                &last_cmd,
                                final_exit_code,
                                total_duration,
                            )
                            .await
                        {
                            tracing::error!("failed to update hook result: {e}");
                        }
                        if let Err(e) = task_state
                            .catalog
                            .update_status(&hook_id, RecordingStatus::Done)
                            .await
                        {
                            tracing::error!("failed to update status to done: {e}");
                        }
                        task_state.events.emit(DaemonEvent::HookDone {
                            id: hook_id,
                            exit_code: final_exit_code,
                        });
                    }
                });
                ok_null()
            }
            Ok(Some(_)) => Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: "no transcript to fire hook against".into(),
            }),
            Ok(None) => not_found(format!("recording {id} not found")),
            Err(e) => err_response(&e),
        },
        Request::RerunCleanup {
            id,
            model,
            provider,
            prompt,
            api_url,
            api_key,
        } => {
            rerun_cleanup(
                state,
                id,
                CleanupOverrides {
                    model,
                    provider,
                    prompt,
                    api_url,
                    api_key,
                },
            )
            .await
        }
        Request::RerunSummary {
            id,
            model,
            prompt,
            provider,
            api_url,
            api_key,
        } => {
            rerun_summary(
                state,
                id,
                model,
                prompt,
                SummaryProviderOverrides {
                    provider,
                    api_url,
                    api_key,
                },
            )
            .await
        }
        Request::RerunMeetingDigest {
            meeting_id,
            model,
            recipe_id,
            provider,
            api_url,
            api_key,
        } => {
            rerun_meeting_digest(
                state,
                meeting_id,
                model,
                recipe_id,
                SummaryProviderOverrides {
                    provider,
                    api_url,
                    api_key,
                },
            )
            .await
        }
        Request::RerunPeriodDigest {
            since,
            until,
            label,
            model,
            provider,
            api_url,
            api_key,
        } => {
            rerun_period_digest(
                state,
                since,
                until,
                label,
                model,
                SummaryProviderOverrides {
                    provider,
                    api_url,
                    api_key,
                },
            )
            .await
        }
        Request::RunDoctor => {
            let cfg = state.config.load();
            // Thread the bundled servers' live ports into the backend probes so a
            // startup port fallback can't make Doctor probe the dead configured
            // port. The supervisors publish these in `whisper_ports` the same way
            // the pipeline reads them via `apply`.
            let ports = phoneme_core::doctor::EffectiveWhisperPorts {
                main: state.whisper_ports.main(),
                preview: state.whisper_ports.preview(),
                in_place: state.whisper_ports.dictation(),
            };
            let mut checks = phoneme_core::doctor::run_local_checks(&cfg);
            checks.extend(phoneme_core::doctor::run_backend_checks_with_ports(&cfg, &ports).await);
            // Daemon-side: needs the catalog + an audio-dir scan, so it can't live
            // in phoneme-core's stateless checks.
            checks.push(orphan_audio_check(state).await);
            // ANN search-index health (feature/flag/warm-state), also catalog-side.
            checks.push(phoneme_core::doctor::ann_index_check_result(
                state.catalog.ann_health().await,
            ));
            serialize_response(checks)
        }
        Request::ExportDiagnostics => {
            // Opt-in, local-only sanitized bundle for bug reports (#248): app +
            // OS info, the MASKED config (secrets redacted via the shared
            // `phoneme_core::secrets` layer), and a tail of this daemon's log —
            // no audio, no transcripts, no catalog, no network. Written under the
            // app data dir; the path is returned for the GUI to reveal.
            //
            // The data dir is the parent of the resolved log dir
            // (`<data_local>/logs`); the diagnostics file lands in a
            // `diagnostics/` sibling. The blocking file read/write runs on a
            // blocking thread so it never stalls the IPC connection.
            let cfg = state.config.load_full();
            let log_dir = state.paths.log_dir.clone();
            let data_dir = log_dir
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| log_dir.clone());
            let result = tokio::task::spawn_blocking(move || {
                phoneme_core::diagnostics::write_bundle(
                    &cfg,
                    env!("CARGO_PKG_VERSION"),
                    &data_dir,
                    &log_dir,
                    phoneme_core::diagnostics::DEFAULT_LOG_TAIL_LINES,
                )
            })
            .await;
            match result {
                Ok(Ok(path)) => Response::Ok(serde_json::json!({
                    "path": path.to_string_lossy(),
                })),
                Ok(Err(e)) => Response::Err(IpcError {
                    kind: IpcErrorKind::Io,
                    message: format!("failed to write diagnostics bundle: {e}"),
                }),
                Err(e) => Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("diagnostics export task failed: {e}"),
                }),
            }
        }
        Request::RestartWhisper => {
            // Sweep every whisper-server process (hung children AND orphans
            // from a dead daemon still holding the port), then wake both
            // supervisors so the main + preview servers respawn from config.
            crate::whisper_supervisor::sweep_stray_servers();
            state.whisper_restart.notify_waiters();
            tracing::info!("whisper-server restart requested via IPC (Doctor fix)");
            Response::Ok(serde_json::json!({
                "message": "whisper-server processes swept; supervisors respawning"
            }))
        }
        Request::SetPreviewSource { track } => {
            catalog_call!(state.recorder.set_preview_source(state, &track).await => ok_null)
        }
        Request::SkipCurrentStage => {
            // Wakes whichever LLM stage is currently streaming (no-op when none
            // is — the notify has no waiter then and stores nothing).
            state.skip_stage.notify_waiters();
            tracing::info!("skip-current-stage requested via IPC");
            ok_null()
        }
        Request::ListQueue => {
            let pending = state.inbox.list_pending().await;
            let processing = state.inbox.list_processing().await;
            match (pending, processing) {
                (Ok(pending), Ok(processing)) => {
                    let entry = |p: &phoneme_core::HookPayload, queue_state: &str| {
                        serde_json::json!({
                            "id": p.id,
                            "timestamp": p.timestamp,
                            "audio_path": p.audio_path,
                            "duration_ms": p.duration_ms,
                            "model": p.model,
                            "state": queue_state,
                        })
                    };
                    let mut items: Vec<serde_json::Value> = Vec::new();
                    // The actively-processing item(s) first, then the pending queue.
                    items.extend(processing.iter().map(|p| entry(p, "processing")));
                    items.extend(pending.iter().map(|p| entry(p, "pending")));
                    Response::Ok(serde_json::Value::Array(items))
                }
                (Err(e), _) | (_, Err(e)) => err_response(&e),
            }
        }
        Request::ReorderQueue { ids } => match state.inbox.set_order(&ids).await {
            Ok(()) => {
                crate::queue_worker::emit_queue_depth(state).await;
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::CancelQueued { id } => match state.inbox.cancel_pending(&id).await {
            Ok(true) => {
                // Mark the recording Cancelled — terminal, so it isn't stuck
                // showing "transcribing", but distinct from the failed states:
                // the user chose this, nothing broke. Re-runnable later.
                let _ = state
                    .catalog
                    .update_status(&id, RecordingStatus::Cancelled)
                    .await;
                state
                    .events
                    .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
                crate::queue_worker::emit_queue_depth(state).await;
                ok_null()
            }
            Ok(false) => Response::Err(IpcError {
                kind: IpcErrorKind::NotFound,
                message: "recording is not in the pending queue (already processing or finished)"
                    .into(),
            }),
            Err(e) => err_response(&e),
        },
        Request::SetQueuePaused { paused } => match state.inbox.set_paused(paused).await {
            Ok(()) => {
                // Nudge the panel so the pause state reflects immediately.
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "paused": paused }))
            }
            Err(e) => err_response(&e),
        },
        Request::QueuePaused => {
            Response::Ok(serde_json::json!({ "paused": state.inbox.is_paused().await }))
        }
        Request::QueueCounts => match state.inbox.counts().await {
            Ok(c) => Response::Ok(serde_json::json!({
                "pending": c.pending,
                "processing": c.processing,
                "done": c.done,
                "failed": c.failed,
            })),
            Err(e) => err_response(&e),
        },
        Request::ClearFailed => match state.inbox.clear_failed().await {
            Ok(removed) => {
                // Refresh the depth so the panel's failed badge clears at once.
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "removed": removed }))
            }
            Err(e) => err_response(&e),
        },
        Request::DismissFailed { id } => match state.inbox.dismiss_failed(&id).await {
            Ok(removed) => {
                // Refresh the depth so the failed badge reflects the new count.
                if removed {
                    crate::queue_worker::emit_queue_depth(state).await;
                }
                Response::Ok(serde_json::json!({ "removed": removed }))
            }
            Err(e) => err_response(&e),
        },
        Request::CancelProcessing { id } => {
            // Signal the in-flight cancellation token only if `id` is the item
            // currently processing; the worker + pipeline finalize the rest.
            let canceled = {
                match state.processing.lock() {
                    Ok(slot) => match slot.as_ref() {
                        Some((pid, token)) if *pid == id => {
                            token.cancel();
                            true
                        }
                        _ => false,
                    },
                    Err(_) => false,
                }
            };
            if canceled {
                ok_null()
            } else {
                Response::Err(IpcError {
                    kind: IpcErrorKind::NotFound,
                    message: "recording is not the item currently being processed".into(),
                })
            }
        }
        Request::CancelAllQueued => match state.inbox.cancel_all_pending().await {
            Ok(ids) => {
                // Mark each recording Cancelled (terminal, but not a failure)
                // so it isn't stuck showing "transcribing", mirroring
                // single-item CancelQueued.
                for id in &ids {
                    let _ = state
                        .catalog
                        .update_status(id, RecordingStatus::Cancelled)
                        .await;
                    state
                        .events
                        .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
                }
                crate::queue_worker::emit_queue_depth(state).await;
                Response::Ok(serde_json::json!({ "removed": ids.len() }))
            }
            Err(e) => err_response(&e),
        },
        // Unlike `RefireHook`, `HookTest` intentionally runs a caller-supplied
        // command: it's the Hook Manager's "test this command" affordance, used to
        // validate a hook the user is editing but hasn't saved yet. That's a
        // deliberate, user-initiated test, gated by the owner-only IPC pipe (S-C1),
        // so it's not an extra privilege-escalation channel and isn't subject to
        // the `RefireHook` allowlist (S-C2).
        Request::HookTest { custom_command } => {
            let cfg = state.config.load();
            let command = custom_command.unwrap_or_else(|| {
                cfg.hook.commands.first().cloned().unwrap_or_default()
            });
            let runner = HookRunner::new(
                command,
                std::time::Duration::from_secs(cfg.hook.timeout_secs),
            );
            // Build a representative test payload. Use a plausible-looking
            // audio path so hooks that reference it (e.g. file-logging hooks)
            // receive a non-empty string rather than silently writing nothing.
            let placeholder_audio = {
                let base = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_else(|_| String::from("C:\\Users\\user"));
                format!("{base}\\Documents\\phoneme\\audio\\test\\sample.wav")
            };
            let sample = HookPayload {
                id: phoneme_core::RecordingId::new(),
                timestamp: chrono::Local::now(),
                transcript: "This is a test transcript for the hook.".into(),
                audio_path: placeholder_audio,
                duration_ms: 3500,
                model: "test".into(),
                metadata: HookMetadata::current(),
            };
            // The test command is caller-supplied and its output is shown in the
            // UI/CLI verbatim — a script that echoes its environment or a config
            // file would hand any key it prints to the renderer. Mask
            // credential-shaped values on both outcomes before the text crosses the
            // pipe: the Ok path carries stderr directly, and the HookFailed error
            // embeds the stderr tail in its message.
            match runner.run(&sample).await {
                Ok(result) => Response::Ok(serde_json::json!({
                    "exit_code": result.exit_code,
                    "duration_ms": result.duration_ms,
                    "stderr_tail": redact_secrets(&result.stderr_tail),
                })),
                Err(e) => Response::Err(IpcError {
                    kind: error_to_kind(&e),
                    message: redact_secrets(&e.to_string()),
                }),
            }
        }
        Request::Shutdown => {
            tracing::info!("shutdown requested via IPC");
            // Reply first, exit second: the trigger is delayed so the Ok response
            // (written by `handle_connection` the moment this arm returns) reaches
            // the pipe before the process starts tearing down. The caller (`phoneme
            // daemon stop`, the tray's Quit) should never be left waiting on a reply
            // that died with the daemon.
            let coordinator = state.shutdown.clone();
            tokio::spawn(async move {
                tokio::time::sleep(SHUTDOWN_REPLY_GRACE).await;
                // Trigger the shared coordinator `main` waits on: it stops
                // the recorder, the workers, and every Owned child, then exits.
                coordinator.trigger();
            });
            ok_null()
        }
        Request::ListTags => catalog_call!(state.catalog.list_tags().await => to_value),
        Request::ListAllTags => catalog_call!(state.catalog.list_all_tags().await => to_value),
        Request::AddTag { name, color } => {
            match state.catalog.add_tag(&name, color.as_deref()).await {
                Ok(tag) => {
                    state.events.emit(DaemonEvent::TagCreated { id: tag.id });
                    Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                }
                Err(e) => err_response(&e),
            }
        }
        Request::UpdateTag { id, name, color } => {
            match state.catalog.update_tag(id, &name, color.as_deref()).await {
                Ok(tag) => {
                    state.events.emit(DaemonEvent::TagUpdated { id });
                    Response::Ok(serde_json::to_value(tag).unwrap_or_default())
                }
                Err(e) => err_response(&e),
            }
        }
        Request::DeleteTag { id } => match state.catalog.delete_tag(id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagDeleted { id });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::AttachTag {
            recording_id,
            tag_id,
        } => match state.catalog.attach_tag(&recording_id, tag_id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagAttached {
                    tag_id,
                    recording_id: Some(recording_id),
                });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::DetachTag {
            recording_id,
            tag_id,
        } => match state.catalog.detach_tag(&recording_id, tag_id).await {
            Ok(()) => {
                state.events.emit(DaemonEvent::TagDetached {
                    tag_id,
                    recording_id: Some(recording_id),
                });
                ok_null()
            }
            Err(e) => err_response(&e),
        },
        Request::TagsFor { recording_id } => {
            catalog_call!(state.catalog.tags_for(&recording_id).await => to_value)
        }
        Request::TagUsageCounts => {
            catalog_call!(state.catalog.tag_usage_counts().await => to_value)
        }
        Request::KindCounts => catalog_call!(state.catalog.kind_counts().await => to_value),
        Request::ListAllEntities => {
            catalog_call!(state.catalog.entity_facets().await => to_value)
        }
        Request::ListAllTasks { only_open } => {
            catalog_call!(state.catalog.list_all_tasks(only_open).await => to_value)
        }
        Request::TaskCounts => catalog_call!(state.catalog.task_counts().await => to_value),
        Request::MergeTags { from_id, into_id } => {
            match state.catalog.merge_tags(from_id, into_id).await {
                Ok(()) => {
                    // The source tag is gone; consumers refresh on TagDeleted.
                    state.events.emit(DaemonEvent::TagDeleted { id: from_id });
                    ok_null()
                }
                Err(e) => err_response(&e),
            }
        }
        Request::ReloadConfig => {
            tracing::info!("reloading config via IPC");
            match crate::load_config() {
                Ok(mut cfg) => {
                    // Same explicit sequence as startup: migrate and persist, then
                    // the in-memory-only runtime defaults, before the daemon adopts
                    // it.
                    crate::reconcile_and_persist_config(&mut cfg);
                    crate::apply_runtime_defaults(&mut cfg);
                    state.config.store(std::sync::Arc::new(cfg));

                    let cfg_arc = state.config.load();
                    // Hold the embedder write lock ONLY for the embedder swap, then
                    // drop it before the catalog / diarizer work below — those don't
                    // touch the embedder, and a concurrent SemanticSearch / ReembedAll
                    // shouldn't block on the read lock while they run.
                    {
                        let mut embedder_guard = state.embedder.write().await;
                        if cfg_arc.semantic_search.enabled {
                            // (Re)build on every reload so a changed model_dir /
                            // pooling / max_tokens / prefix actually takes effect on
                            // save — not only when no model was loaded before. On
                            // failure keep the previous model so search doesn't break.
                            match phoneme_core::Embedder::new(&cfg_arc.semantic_search) {
                                Ok(e) => *embedder_guard = Some(std::sync::Arc::new(e)),
                                Err(e) => {
                                    tracing::warn!(error = %e, "failed to (re)load semantic search model on reload; keeping the previous one")
                                }
                            }
                        } else {
                            *embedder_guard = None;
                        }
                    }

                    // Re-apply the ANN tuning config to the live catalog so a
                    // runtime enable/disable/tuning change takes effect without a
                    // restart. `set_ann_config` drops the warm index + sidecar when
                    // ANN is turned off; turning it on (or retuning) is picked up by
                    // the daemon's next background rebuild. A no-op on a default
                    // build (the `ann-usearch` feature isn't compiled).
                    state
                        .catalog
                        .set_ann_config(cfg_arc.semantic_search.ann.clone());

                    // Drop the cached local diarization pipeline when
                    // `[diarization]` changed (backend switch / model path) —
                    // the next run reloads under the new config, and switching
                    // away from Local frees the model RAM immediately.
                    state
                        .transcription
                        .diarizer_cache()
                        .invalidate_if_stale(&cfg_arc.diarization);

                    drop(cfg_arc);

                    // Start/stop idle pre-roll pre-capture to match the new
                    // config (e.g. user just toggled pre_roll_ms).
                    state.recorder.sync_preroll(state).await;
                    ok_null()
                }
                Err(e) => Response::Err(IpcError {
                    kind: IpcErrorKind::InvalidConfig,
                    message: format!("failed to load config: {e}"),
                }),
            }
        }
        Request::SubscribeEvents => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "subscribe_events is handled by the streaming path in handle_connection"
                .into(),
        }),
    }
}

/// Hard cap on how many results a single `SemanticSearch` / `MoreLikeThis` request
/// may return. The client picks the limit, so this bounds the result Vec + the
/// JSON serialized back over the pipe — a huge `limit` can't force an unbounded
/// allocation. Far above any real UI/CLI page size.
const MAX_SEARCH_RESULTS: usize = 1000;


// ── Split-out implementation modules ─────────────────────────────────────
mod exec;
mod helpers;
use exec::*;
use helpers::*;
