//! Ask-my-archive — local RAG: answer a question grounded in the user's own
//! transcripts, with citations.
//!
//! This is the spawned worker behind [`Request::Ask`](phoneme_ipc::Request::Ask).
//! The IPC handler validates synchronously (embedder loaded, an LLM provider
//! configured), ACKs `ok_null()`, and hands the work to [`run_ask`], which:
//!
//! 1. embeds the question off-thread,
//! 2. retrieves the top grounding chunks via the same hybrid (vector + FTS5/RRF)
//!    path the search bar uses ([`phoneme_core::Catalog::retrieve_context`]),
//! 3. assembles the citation sources + the grounded prompt (pure helpers,
//!    unit-tested below),
//! 4. emits one `AskActivity` carrying the `sources` (before any answer token),
//! 5. streams the answer through the configured `[llm_post_process]` provider,
//!    reusing the *same* coalesce/cap constants as the cleanup/summary stage
//!    ([`crate::pipeline::DELTA_FLUSH_CHARS`] / `MAX_STREAMED_CHARS`), and
//! 6. flushes a terminal `done` marker (with `error` set on failure).
//!
//! It is deliberately NOT a per-recording pipeline stage: no `RecordingId`, no
//! `PipelineStage`, no AI-activity persistence, and no `skip_active_queue_item`
//! coupling (a queue ⏭ must never collaterally abort an Ask). All activity rides
//! the new [`DaemonEvent::AskActivity`] event, keyed by the client-supplied
//! `request_id`.

use crate::app_state::AppState;
use crate::pipeline::{DELTA_FLUSH_CHARS, MAX_STREAMED_CHARS};
use phoneme_core::config::LlmPostProcessConfig;
use phoneme_core::{Embedder, ListFilter, LlmProvider, RetrievedChunk};
use phoneme_ipc::{AskSource, DaemonEvent};
use std::sync::Arc;

/// Minimum calibrated relevance an Ask grounding chunk must clear — the same
/// floor `SemanticSearch` uses (`ipc_handler::SEMANTIC_MIN_RELEVANCE`) so Ask is
/// grounded on the same evidence the search bar would surface. Kept local so the
/// retrieval module owns no daemon-wide constant.
const ASK_MIN_RELEVANCE: f32 = 0.12;

/// Snippet budget per cited chunk inside the prompt (char-boundary-truncated).
const ASK_PER_SOURCE_CHARS: usize = 1200;

/// Total context-char budget across all cited snippets. Small to keep the prompt
/// within reach of a modest local model (see the weak-PC memory note): once
/// adding the next source's snippet would exceed this, no further sources are
/// included.
const ASK_CONTEXT_CHAR_BUDGET: usize = 12_000;

/// The grounded-answer worker. Runs detached after the handler's ACK, so every
/// post-ACK failure (query-embed, retrieval, generation) surfaces as a terminal
/// `AskActivity { done: true, error }` rather than vanishing.
pub(crate) async fn run_ask(
    state: &AppState,
    embedder: Arc<Embedder>,
    llm_cfg: LlmPostProcessConfig,
    request_id: String,
    query: String,
    top_k: usize,
    filter: Option<ListFilter>,
) {
    // Embed the question off-thread (ONNX is blocking CPU work).
    let q = query.clone();
    let query_vec = match tokio::task::spawn_blocking(move || embedder.embed_query(&q)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            emit_error(
                state,
                &request_id,
                format!("couldn't embed the question: {e}"),
            );
            return;
        }
        Err(e) => {
            emit_error(state, &request_id, format!("embedding task failed: {e}"));
            return;
        }
    };

    // The seam the mocked-LLM end-to-end test uses: production resolves the real
    // provider, the test injects its own and a precomputed query vector.
    run_ask_with_vec(
        state, &llm_cfg, request_id, query, query_vec, top_k, filter, None,
    )
    .await;
}

/// `run_ask`'s body after the query vector exists. `provider_override` lets a
/// test inject a `MockProvider` and bypass `LlmPostProcessor` (and the ONNX
/// model); `None` resolves the real provider through
/// [`crate::pipeline::llm_provider_for_run`] (so a local Ollama auto-launches off
/// the IPC connection, exactly like the cleanup re-run).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_ask_with_vec(
    state: &AppState,
    llm_cfg: &LlmPostProcessConfig,
    request_id: String,
    query: String,
    query_vec: Vec<f32>,
    top_k: usize,
    filter: Option<ListFilter>,
    provider_override: Option<Box<dyn LlmProvider>>,
) {
    // Retrieve grounding chunks. A failure after the ACK must stream, not vanish.
    let chunks = match state
        .catalog
        .retrieve_context(
            &query,
            &query_vec,
            top_k,
            ASK_MIN_RELEVANCE,
            filter.as_ref(),
        )
        .await
    {
        Ok(c) => c,
        Err(e) => {
            emit_error(state, &request_id, format!("retrieval failed: {e}"));
            return;
        }
    };

    // Assemble the citation sources (per-source truncation + total budget). Needs
    // each recording's row for the display label.
    let sources = assemble_sources(state, &chunks).await;

    // Emit the sources event first, before any answer token, so the UI renders
    // the source list while the answer streams. An empty `sources` means nothing
    // matched.
    state.events.emit(DaemonEvent::AskActivity {
        request_id: request_id.clone(),
        sources: sources.clone(),
        delta: String::new(),
        done: false,
        error: String::new(),
    });

    // Empty retrieval: a terminal "nothing matched" answer WITHOUT calling the
    // LLM (calling it with no context invites hallucination).
    if sources.is_empty() {
        state.events.emit(DaemonEvent::AskActivity {
            request_id: request_id.clone(),
            sources: Vec::new(),
            delta: "I couldn't find anything about that in your recordings.".into(),
            done: false,
            error: String::new(),
        });
        emit_done(state, &request_id, String::new());
        return;
    }

    // Resolve the provider (test override, else the real run-resolver).
    let provider = match provider_override {
        Some(p) => p,
        None => match crate::pipeline::llm_provider_for_run(state, llm_cfg).await {
            Some(p) => p,
            None => {
                emit_error(
                    state,
                    &request_id,
                    "no LLM provider available for Ask".to_string(),
                );
                return;
            }
        },
    };

    // Build the grounded prompt. The wire message every provider sends is
    // `combine(prompt, text) = "{prompt}:\n{text}"`, so the entire grounded
    // instruction+excerpts is the `prompt` and the bare question is the `text`.
    let grounded_prompt = build_ask_prompt(&sources);

    // Stream the answer. Coalesce to DELTA_FLUSH_CHARS and cap at
    // MAX_STREAMED_CHARS (char-boundary safe), reusing the exact constants the
    // cleanup/summary stage uses. No skip-stage select arm — a queue ⏭ must not
    // abort an Ask.
    let mut pending = String::new();
    let mut streamed = 0usize;
    let result = {
        let req = request_id.clone();
        let events = state.events.clone();
        let mut on_delta = |d: &str| {
            if streamed >= MAX_STREAMED_CHARS {
                return;
            }
            let remaining = MAX_STREAMED_CHARS - streamed;
            let slice = if d.len() > remaining {
                let mut end = remaining;
                while end > 0 && !d.is_char_boundary(end) {
                    end -= 1;
                }
                &d[..end]
            } else {
                d
            };
            pending.push_str(slice);
            streamed += slice.len();
            if pending.len() >= DELTA_FLUSH_CHARS {
                events.emit(DaemonEvent::AskActivity {
                    request_id: req.clone(),
                    sources: Vec::new(),
                    delta: std::mem::take(&mut pending),
                    done: false,
                    error: String::new(),
                });
            }
        };
        provider
            .process_streaming(&grounded_prompt, &query, &mut on_delta)
            .await
    };

    // Flush any tail, then the terminal marker (regardless of outcome).
    if !pending.is_empty() {
        state.events.emit(DaemonEvent::AskActivity {
            request_id: request_id.clone(),
            sources: Vec::new(),
            delta: std::mem::take(&mut pending),
            done: false,
            error: String::new(),
        });
    }
    let error = match result {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };
    emit_done(state, &request_id, error);
}

/// `run_ask_with_vec` with the real provider resolution skipped: the production
/// path embeds and retrieves, the test injects a `MockProvider`. Kept as a thin
/// wrapper so the mocked end-to-end test can drive the assembly→stream path with
/// a precomputed query vector and a fake provider (no ONNX model, no network).
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) async fn run_ask_with_provider(
    state: &AppState,
    request_id: String,
    query: String,
    query_vec: Vec<f32>,
    top_k: usize,
    filter: Option<ListFilter>,
    provider: Box<dyn LlmProvider>,
) {
    // A throwaway config — the override means it's never probed.
    let llm_cfg = state.config.load().llm_post_process.clone();
    run_ask_with_vec(
        state,
        &llm_cfg,
        request_id,
        query,
        query_vec,
        top_k,
        filter,
        Some(provider),
    )
    .await;
}

/// Emit a terminal failure event (the ACK already went out, so a post-ACK error
/// must surface on the stream).
fn emit_error(state: &AppState, request_id: &str, error: String) {
    tracing::warn!(%request_id, %error, "ask failed after ack");
    state.events.emit(DaemonEvent::AskActivity {
        request_id: request_id.to_string(),
        sources: Vec::new(),
        delta: String::new(),
        done: true,
        error,
    });
}

/// Emit the terminal `done` marker (mirrors `run_llm_stage`'s
/// done-regardless-of-outcome). `error` empty on success.
fn emit_done(state: &AppState, request_id: &str, error: String) {
    state.events.emit(DaemonEvent::AskActivity {
        request_id: request_id.to_string(),
        sources: Vec::new(),
        delta: String::new(),
        done: true,
        error,
    });
}

/// Turn retrieved chunks into citation sources: assign 1-based markers in rank
/// order, fetch each recording for its display label, truncate every snippet to
/// [`ASK_PER_SOURCE_CHARS`] on a char boundary, and stop once the running context
/// total would exceed [`ASK_CONTEXT_CHAR_BUDGET`].
async fn assemble_sources(state: &AppState, chunks: &[RetrievedChunk]) -> Vec<AskSource> {
    let mut sources: Vec<AskSource> = Vec::new();
    let mut total = 0usize;
    for chunk in chunks {
        let snippet = truncate_on_char_boundary(&chunk.text, ASK_PER_SOURCE_CHARS);
        if snippet.is_empty() {
            continue; // never cite an empty snippet
        }
        // Stop adding sources once the budget would be exceeded (keep at least
        // the first one even if it alone is large, so a single long chunk still
        // grounds an answer).
        if !sources.is_empty() && total + snippet.len() > ASK_CONTEXT_CHAR_BUDGET {
            break;
        }
        total += snippet.len();

        // The display label comes from the recording row: title → meeting name →
        // formatted start time. A missing row (raced deletion) still cites by id.
        let label = match state.catalog.get(&chunk.recording_id).await {
            Ok(Some(r)) => recording_label(&r),
            _ => chunk.recording_id.as_str().to_string(),
        };

        sources.push(AskSource {
            n: sources.len() + 1,
            recording_id: chunk.recording_id.clone(),
            meeting_id: chunk.meeting_id.clone(),
            label,
            chunk_index: chunk.chunk_index,
            snippet,
            relevance: chunk.relevance,
        });
    }
    sources
}

/// The display label for a cited recording: title → meeting name → formatted
/// start time. Mirrors how the rest of the UI names a recording.
fn recording_label(r: &phoneme_core::Recording) -> String {
    if let Some(title) = r.title.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return title.to_string();
    }
    if let Some(name) = r
        .meeting_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return name.to_string();
    }
    r.started_at.format("%b %-d, %Y %H:%M").to_string()
}

/// Build the grounded instruction+excerpts block sent as the provider `prompt`.
/// The model is told to answer ONLY from the excerpts and to cite each with its
/// `[n]` marker; the daemon owns the marker↔source mapping, so the model only
/// echoes the number.
fn build_ask_prompt(sources: &[AskSource]) -> String {
    let mut p = String::new();
    p.push_str(
        "You are answering a question using ONLY the excerpts below, taken from the \
user's own voice recordings. Each excerpt is tagged with a citation marker like [1]. Rules:\n\
- Answer from the excerpts only. If they don't contain the answer, say you couldn't find \
it in the recordings — do not use outside knowledge.\n\
- After every sentence that uses an excerpt, cite it inline with its marker, e.g. \
\"...the migration was delayed [2].\"\n\
- Be concise and use the user's own domain language.\n\n\
Excerpts:\n",
    );
    for s in sources {
        p.push_str(&format!("[{}] ({}): {}\n", s.n, s.label, s.snippet));
    }
    p
}

/// A char-boundary-safe leading slice of `text`, at most `max` bytes, with an
/// ellipsis when truncated. Empty in → empty out.
fn truncate_on_char_boundary(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= max {
        return trimmed.to_string();
    }
    let mut end = max;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &trimmed[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoneme_core::types::{Recording, RecordingStatus};
    use phoneme_core::RecordingId;

    fn chunk(idx: i64, text: &str, relevance: f32) -> RetrievedChunk {
        RetrievedChunk {
            recording_id: RecordingId::new(),
            meeting_id: None,
            chunk_index: idx,
            text: text.into(),
            relevance,
            is_lexical: false,
        }
    }

    fn source(n: usize, label: &str, snippet: &str) -> AskSource {
        AskSource {
            n,
            recording_id: RecordingId::new(),
            meeting_id: None,
            label: label.into(),
            chunk_index: 0,
            snippet: snippet.into(),
            relevance: 0.6,
        }
    }

    #[test]
    fn truncate_respects_char_boundaries_and_budget() {
        // ASCII: simple cut + ellipsis.
        let t = truncate_on_char_boundary("hello world", 5);
        assert_eq!(t, "hello…");
        // Under the cap: returned trimmed, no ellipsis.
        assert_eq!(truncate_on_char_boundary("  hi  ", 100), "hi");
        // Multibyte: a cut that would land mid-char backs up to a boundary.
        let s = "café latté"; // 'é' is 2 bytes
        let cut = truncate_on_char_boundary(s, 4); // 4 = "caf" + first byte of é
        assert!(cut.starts_with("caf"));
        assert!(std::str::from_utf8(cut.as_bytes()).is_ok(), "valid UTF-8");
    }

    #[test]
    fn build_prompt_contains_the_contract_and_every_marker() {
        let sources = vec![
            source(1, "Standup notes", "we deferred the migration"),
            source(2, "1:1 with Sam", "the budget was approved"),
        ];
        let prompt = build_ask_prompt(&sources);
        assert!(
            prompt.contains("ONLY the excerpts"),
            "states the grounding contract"
        );
        assert!(
            prompt.contains("cite it inline"),
            "states the citation contract"
        );
        assert!(prompt.contains("[1] (Standup notes): we deferred the migration"));
        assert!(prompt.contains("[2] (1:1 with Sam): the budget was approved"));
    }

    /// Build an in-process AppState in a temp data dir (explicit path → no env
    /// read, so parallel-safe). Mirrors the harness the ipc_handler tests use.
    async fn test_state(tmp: &std::path::Path) -> crate::app_state::AppState {
        crate::app_state::AppState::new_in(phoneme_core::Config::default(), Some(tmp.join("data")))
            .await
            .expect("build test AppState")
    }

    #[tokio::test]
    async fn assemble_sources_numbers_contiguously_and_respects_budgets() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // Three chunks; markers must be 1,2,3 and snippets non-empty.
        let chunks = vec![
            chunk(0, "first chunk text", 0.7),
            chunk(1, "second chunk text", 0.6),
            chunk(2, "third chunk text", 0.5),
        ];
        let sources = assemble_sources(&state, &chunks).await;
        assert_eq!(sources.len(), 3);
        assert_eq!(
            sources.iter().map(|s| s.n).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "markers are 1-based and contiguous"
        );
        assert!(sources.iter().all(|s| !s.snippet.is_empty()));
    }

    #[tokio::test]
    async fn assemble_sources_caps_each_snippet_and_stops_at_the_total_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // Many oversized chunks. Each is truncated to ASK_PER_SOURCE_CHARS, and
        // adding sources stops once the running total would exceed
        // ASK_CONTEXT_CHAR_BUDGET — so fewer than all of them survive.
        let big = "word ".repeat(ASK_PER_SOURCE_CHARS); // well over the per-source cap
        let n_chunks = (ASK_CONTEXT_CHAR_BUDGET / ASK_PER_SOURCE_CHARS) + 5;
        let chunks: Vec<RetrievedChunk> =
            (0..n_chunks as i64).map(|i| chunk(i, &big, 0.7)).collect();

        let sources = assemble_sources(&state, &chunks).await;
        assert!(!sources.is_empty(), "at least the first source is kept");
        assert!(
            sources.len() < chunks.len(),
            "the total context budget stops adding sources ({} of {})",
            sources.len(),
            chunks.len()
        );
        // Each snippet is capped at the per-source budget (+1 char for the
        // ellipsis the truncation appends).
        for s in &sources {
            assert!(
                s.snippet.chars().count() <= ASK_PER_SOURCE_CHARS + 1,
                "each snippet is capped at the per-source budget, got {}",
                s.snippet.chars().count()
            );
        }
        // Markers stay 1-based and contiguous even after the budget cut.
        assert_eq!(
            sources.iter().map(|s| s.n).collect::<Vec<_>>(),
            (1..=sources.len()).collect::<Vec<_>>()
        );
    }

    // ── Mocked-LLM end-to-end of the spawned worker (Test plan C) ─────────────

    /// A test `LlmProvider` that emits a fixed set of deltas (referencing the
    /// `[n]` citation markers) then returns. Lets the end-to-end test drive the
    /// sources-then-stream-then-done lifecycle without the ONNX model or a
    /// network call. `should_fail` makes `process` error so the terminal-error
    /// path can be asserted too.
    struct MockProvider {
        deltas: Vec<String>,
        should_fail: bool,
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockProvider {
        async fn process(&self, _prompt: &str, _text: &str) -> phoneme_core::Result<String> {
            if self.should_fail {
                return Err(phoneme_core::Error::Internal("mock provider failed".into()));
            }
            Ok(self.deltas.concat())
        }

        async fn process_streaming(
            &self,
            _prompt: &str,
            _text: &str,
            on_delta: phoneme_core::llm::DeltaSink<'_>,
        ) -> phoneme_core::Result<String> {
            if self.should_fail {
                return Err(phoneme_core::Error::Internal("mock provider failed".into()));
            }
            for d in &self.deltas {
                on_delta(d);
            }
            Ok(self.deltas.concat())
        }
    }

    /// Seed a Done recording with `transcript` and a single chunk vector, so
    /// `retrieve_context` returns it for a query equal to that vector. Returns
    /// the recording id.
    async fn seed_recording(
        state: &crate::app_state::AppState,
        transcript: &str,
        vector: Vec<f32>,
    ) -> RecordingId {
        let id = RecordingId::new();
        let row = Recording {
            id: id.clone(),
            started_at: chrono::Local::now(),
            duration_ms: 1000,
            audio_path: "x.wav".into(),
            transcript: Some(transcript.to_string()),
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
            title: Some("Standup notes".into()),
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            mean_confidence: None,
            tags: vec![],
            speaker_names: vec![],
        };
        state.catalog.insert(&row).await.unwrap();
        state
            .catalog
            .upsert_chunk_embeddings(&id, std::slice::from_ref(&vector))
            .await
            .unwrap();
        id
    }

    /// Drain every `AskActivity` for `request_id` already buffered on the bus
    /// receiver, in order. The worker has already finished (we `.await` it), so
    /// all its events are sitting in the broadcast buffer.
    fn drain_ask_events(
        rx: &mut tokio::sync::broadcast::Receiver<DaemonEvent>,
        request_id: &str,
    ) -> Vec<(Vec<AskSource>, String, bool, String)> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let DaemonEvent::AskActivity {
                request_id: rid,
                sources,
                delta,
                done,
                error,
            } = ev
            {
                if rid == request_id {
                    out.push((sources, delta, done, error));
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn run_ask_emits_sources_then_deltas_then_done() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;

        // One recording whose only chunk vector is the basis vector e0; a query
        // equal to e0 retrieves it with cosine 1.0.
        let rec_id = seed_recording(
            &state,
            "we deferred the database migration",
            vec![1.0, 0.0, 0.0],
        )
        .await;

        let mut rx = state.events.subscribe();
        let request_id = "req-e2e-1".to_string();
        let provider = Box::new(MockProvider {
            deltas: vec!["The migration was deferred [1].".into()],
            should_fail: false,
        });
        run_ask_with_provider(
            &state,
            request_id.clone(),
            "what happened to the migration".into(),
            vec![1.0, 0.0, 0.0],
            8,
            None,
            provider,
        )
        .await;

        let events = drain_ask_events(&mut rx, &request_id);
        assert!(
            events.len() >= 3,
            "sources + ≥1 delta + done, got {events:?}"
        );

        // (1) the first event carries non-empty sources and is not done.
        let (sources, delta, done, error) = &events[0];
        assert!(
            !sources.is_empty(),
            "first event ships the citation sources"
        );
        assert!(delta.is_empty() && !done && error.is_empty());
        assert_eq!(sources[0].n, 1, "first marker is 1-based");
        assert_eq!(sources[0].recording_id.as_str(), rec_id.as_str());

        // (2) at least one delta event with the answer text (and no source list).
        let answer: String = events
            .iter()
            .filter(|(_, _, done, _)| !done)
            .map(|(_, delta, _, _)| delta.clone())
            .collect();
        assert!(
            answer.contains("[1]"),
            "the streamed answer echoes the marker, got {answer:?}"
        );

        // (3) exactly one terminal done event, with no error.
        let dones: Vec<_> = events.iter().filter(|(_, _, done, _)| *done).collect();
        assert_eq!(dones.len(), 1, "exactly one terminal marker");
        assert!(dones[0].3.is_empty(), "successful generation has no error");
    }

    #[tokio::test]
    async fn run_ask_empty_retrieval_answers_without_calling_the_llm() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        // No recordings at all → nothing matches.

        let mut rx = state.events.subscribe();
        let request_id = "req-empty".to_string();
        // A provider that would FAIL if called — proving the empty path never
        // touches the LLM.
        let provider = Box::new(MockProvider {
            deltas: vec![],
            should_fail: true,
        });
        run_ask_with_provider(
            &state,
            request_id.clone(),
            "anything at all".into(),
            vec![1.0, 0.0, 0.0],
            8,
            None,
            provider,
        )
        .await;

        let events = drain_ask_events(&mut rx, &request_id);
        // An empty-sources event, a "nothing matched" delta, then a clean done.
        assert!(
            events.iter().any(|(s, _, _, _)| s.is_empty()),
            "an empty sources event is emitted"
        );
        let answer: String = events.iter().map(|(_, d, _, _)| d.clone()).collect();
        assert!(
            answer.to_lowercase().contains("couldn't find"),
            "a terminal nothing-matched answer is streamed, got {answer:?}"
        );
        let dones: Vec<_> = events.iter().filter(|(_, _, done, _)| *done).collect();
        assert_eq!(dones.len(), 1);
        assert!(dones[0].3.is_empty(), "empty retrieval is not an error");
    }

    #[tokio::test]
    async fn run_ask_provider_error_surfaces_as_terminal_error() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        seed_recording(&state, "the migration was deferred", vec![1.0, 0.0, 0.0]).await;

        let mut rx = state.events.subscribe();
        let request_id = "req-fail".to_string();
        let provider = Box::new(MockProvider {
            deltas: vec![],
            should_fail: true,
        });
        run_ask_with_provider(
            &state,
            request_id.clone(),
            "what happened".into(),
            vec![1.0, 0.0, 0.0],
            8,
            None,
            provider,
        )
        .await;

        let events = drain_ask_events(&mut rx, &request_id);
        // Sources still ship (retrieval succeeded); the failure is the terminal
        // done's error, not a swallow.
        assert!(!events[0].0.is_empty(), "sources shipped first");
        let dones: Vec<_> = events.iter().filter(|(_, _, done, _)| *done).collect();
        assert_eq!(dones.len(), 1, "one terminal marker even on failure");
        assert!(
            !dones[0].3.is_empty(),
            "the provider error is on the terminal event"
        );
    }

    #[tokio::test]
    async fn run_ask_with_vec_embed_already_done_retrieval_error_is_not_swallowed() {
        // A retrieval against a fresh catalog with a *dimension-mismatched* query
        // can't error (mismatches are skipped), so this asserts the post-ack
        // contract a different way: the worker always reaches a terminal `done`
        // for any input, never returning silently. Empty library + a valid query
        // → terminal done via the nothing-matched path.
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).await;
        let mut rx = state.events.subscribe();
        let request_id = "req-terminal".to_string();
        let llm_cfg = state.config.load().llm_post_process.clone();
        run_ask_with_vec(
            &state,
            &llm_cfg,
            request_id.clone(),
            "q".into(),
            vec![1.0, 0.0, 0.0],
            8,
            None,
            Some(Box::new(MockProvider {
                deltas: vec![],
                should_fail: true,
            })),
        )
        .await;
        let events = drain_ask_events(&mut rx, &request_id);
        assert!(
            events.iter().any(|(_, _, done, _)| *done),
            "the worker always reaches a terminal done"
        );
    }
}
