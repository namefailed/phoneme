//! Pipeline orchestration: transcribe → hook → done.
//!
//! Called by the queue worker per claimed payload.

use crate::app_state::{AppState, WhisperModelOverride};
use phoneme_core::config::{
    Config, InPlaceConfig, LlmPostProcessConfig, TranscriptionBackend, WhisperConfig, WhisperMode,
};
use phoneme_core::error::Result;
use phoneme_core::{
    Catalog, Embedder, HookMetadata, HookPayload, HookRunner, RecordingId, RecordingStatus,
};
use phoneme_ipc::{DaemonEvent, PipelineStage};
use std::sync::Arc;

/// Coalesce streamed deltas until this many chars accumulate, then flush one
/// LlmActivity event — keeps the event bus from being flooded token-by-token.
const DELTA_FLUSH_CHARS: usize = 48;
/// Cap on total response chars forwarded to the UI per stage (the full result
/// is still returned and stored; only the live "thinking" view is bounded).
const MAX_STREAMED_CHARS: usize = 16 * 1024;

/// Longest we wait for the bundled whisper-server to come back up after a
/// one-job model-override swap before transcribing anyway. A model load can take
/// several seconds (large models, cold disk); if it overruns this the transcribe
/// attempt fails with `WhisperUnreachable`, which the queue worker already
/// retries with backoff — so this bound only avoids a needless first failure, it
/// is never a correctness requirement.
const WHISPER_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Restores the configured whisper model when a one-job model override goes out
/// of scope. Dropping it pings the supervisor to swap the bundled server back,
/// so the override is undone on EVERY pipeline exit path (success, transcribe
/// error, cancel) without each path having to remember to clear it. A no-op
/// (`inner = None`) when the job had no override or used a cloud backend (no
/// server to restore).
struct WhisperOverrideGuard {
    inner: Option<Arc<WhisperModelOverride>>,
}

impl Drop for WhisperOverrideGuard {
    fn drop(&mut self) {
        if let Some(o) = self.inner.take() {
            // Clear the override → supervisor restarts the bundled server back
            // onto the configured model for subsequent jobs.
            o.set(None);
        }
    }
}

/// Apply a recording's one-time whisper model override for THIS job only,
/// returning the per-job [`WhisperConfig`] to build the provider from plus a
/// guard that restores the configured model on drop.
///
/// - No override (or a blank one): returns the configured config unchanged with
///   a no-op guard — the steady-state path, byte-for-byte the prior behavior.
/// - Local bundled backend: the override is a model FILE the single shared
///   server must load, so we publish it to the supervisor (which performs one
///   controlled restart), wait for the server to report ready, and pin the
///   per-job `model_path` to the override for labels/stored model. The override
///   never touches the process-global config, so previews and other jobs keep
///   running the configured model. The returned guard clears the override.
/// - Cloud / custom backend: the override is just a model id sent in the HTTP
///   request, so we swap it into a per-job config clone — no server, no
///   supervisor, no guard work needed.
///
/// Runs while the caller holds the `whisper_sem` permit, so for the local case
/// the restart + readiness wait is serialized against the preview and any other
/// final transcription.
async fn apply_model_override(
    state: &AppState,
    configured: &WhisperConfig,
    requested: Option<String>,
) -> (WhisperConfig, WhisperOverrideGuard) {
    let model = match requested {
        Some(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => return (configured.clone(), WhisperOverrideGuard { inner: None }),
    };

    let mut whisper_cfg = configured.clone();
    match configured.provider {
        TranscriptionBackend::Local => {
            tracing::info!(model = %model, "re-transcribe: applying one-job whisper model override");
            // Publish the override; the supervisor swaps the server's model.
            state.whisper_model_override.set(Some(model.clone()));
            // Pin the per-job model_path so the activity label and the stored
            // `model` reflect the override (the local provider talks to the
            // server over HTTP and ignores model_path itself).
            whisper_cfg.model_path = model;
            // Only the bundled server is ours to wait on; External is a
            // user-managed endpoint we never restart. The URL is re-resolved
            // on every poll because the override restart re-runs the
            // supervisor's port probe — the server can come back on a
            // different port than the one it left (its preferred port freed
            // up, or a fresh fallback was assigned).
            if matches!(
                configured.mode,
                WhisperMode::BundledModel | WhisperMode::BundledDownload
            ) {
                let poll_state = state.clone();
                let poll_cfg = whisper_cfg.clone();
                wait_for_whisper_ready(
                    move || {
                        let cfg = poll_state.config.load();
                        poll_state
                            .whisper_ports
                            .apply(&cfg, &poll_cfg)
                            .server_base_url()
                    },
                    WHISPER_READY_TIMEOUT,
                )
                .await;
            }
            (
                whisper_cfg,
                WhisperOverrideGuard {
                    inner: Some(state.whisper_model_override.clone()),
                },
            )
        }
        // Cloud / custom: the model is a request parameter, not a server model.
        _ => {
            tracing::info!(model = %model, "re-transcribe: applying one-job cloud model override");
            whisper_cfg.model = model;
            (whisper_cfg, WhisperOverrideGuard { inner: None })
        }
    }
}

/// Best-effort wait until the bundled whisper-server answers `GET {base}/health`
/// with success, or `timeout` elapses. Used right after a one-job model-override
/// restart so the transcription doesn't fire at a server that's still loading
/// the model. `base_url` is a closure, evaluated fresh each poll, because the
/// restart can move the server to a different port (the supervisor re-runs its
/// port probe on every spawn). Never errors: on timeout it logs and returns,
/// letting the normal transcribe attempt (and the queue worker's
/// `WhisperUnreachable` retry) take over.
async fn wait_for_whisper_ready(base_url: impl Fn() -> String, timeout: Duration) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "could not build health-probe client; skipping readiness wait");
            return;
        }
    };
    let deadline = std::time::Instant::now() + timeout;
    let mut poll = tokio::time::interval(Duration::from_millis(200));
    loop {
        let health = format!("{}/health", base_url().trim_end_matches('/'));
        if let Ok(resp) = client.get(&health).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        if std::time::Instant::now() >= deadline {
            tracing::warn!(
                url = %health,
                "whisper-server not ready within timeout after model-override swap; transcribing anyway"
            );
            return;
        }
        poll.tick().await;
    }
}

/// Reason string carried by the `Err` a user-initiated stage skip produces
/// (the queue panel's ⏭ button / `phoneme queue skip`). It reaches the GUI
/// verbatim inside `SummaryFailed.error`, where the toast layer matches on
/// the phrase to report "skipped" instead of a failure — keep
/// `frontend/src/services/notifications.ts` in sync if it ever changes.
pub(crate) const STAGE_SKIPPED_REASON: &str = "step skipped by user";

/// True when an LLM-stage error is the user's skip, not a real failure.
pub(crate) fn stage_skipped(e: &phoneme_core::Error) -> bool {
    matches!(e, phoneme_core::Error::Internal(m) if m == STAGE_SKIPPED_REASON)
}

/// Run one LLM stage (cleanup or summary) through the streaming path, emitting
/// `DaemonEvent::LlmActivity` so the GUI can show the exact prompt and the
/// response as it streams. Returns the final (normalized) text.
pub(crate) async fn run_llm_stage(
    state: &AppState,
    id: &RecordingId,
    stage: PipelineStage,
    provider: &dyn phoneme_core::LlmProvider,
    prompt: &str,
    text: &str,
) -> Result<String> {
    // (1) Start event carrying the verbatim prompt.
    state.events.emit(DaemonEvent::LlmActivity {
        id: id.clone(),
        stage,
        prompt: provider.exact_prompt(prompt, text),
        delta: String::new(),
        done: false,
    });

    // (2) Stream deltas, coalesced and capped. The stream races the queue UI's
    // "skip this step" signal — a skip aborts just this stage (the pipeline
    // treats it like a non-fatal stage failure and moves on).
    let mut pending = String::new();
    let mut streamed = 0usize;
    let result = {
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
                state.events.emit(DaemonEvent::LlmActivity {
                    id: id.clone(),
                    stage,
                    prompt: String::new(),
                    delta: std::mem::take(&mut pending),
                    done: false,
                });
            }
        };
        tokio::select! {
            r = provider.process_streaming(prompt, text, &mut on_delta) => r,
            _ = state.skip_stage.notified() => {
                tracing::info!(?stage, "stage skipped by user");
                Err(phoneme_core::Error::Internal(STAGE_SKIPPED_REASON.into()))
            }
        }
    };

    // (3) Flush any tail and the terminal `done` marker (regardless of outcome).
    if !pending.is_empty() {
        state.events.emit(DaemonEvent::LlmActivity {
            id: id.clone(),
            stage,
            prompt: String::new(),
            delta: std::mem::take(&mut pending),
            done: false,
        });
    }
    state.events.emit(DaemonEvent::LlmActivity {
        id: id.clone(),
        stage,
        prompt: String::new(),
        delta: String::new(),
        done: true,
    });

    result
}
use std::time::Duration;

/// Embed `transcript` for semantic search and persist both representations:
/// per-chunk vectors (the high-recall path that powers paraphrase matching) and
/// the legacy whole-recording vector (kept so anything that still reads the old
/// `embeddings` table — and the search fallback — stays consistent).
///
/// Shared by every place a transcript becomes final or changes (pipeline, manual
/// edit, cleanup re-run, retroactive backfill) so all paths embed identically.
/// Best-effort: a failure is logged, never fatal — search degrades gracefully
/// rather than failing the recording.
pub(crate) async fn embed_and_store(
    embedder: &Embedder,
    catalog: &Catalog,
    id: &RecordingId,
    transcript: &str,
) {
    match embedder.embed_chunks(transcript) {
        Ok(chunks) => {
            if let Err(e) = catalog.upsert_chunk_embeddings(id, &chunks).await {
                tracing::warn!(error = %e, "Failed to save chunk embeddings");
            } else {
                tracing::info!(
                    chunks = chunks.len(),
                    "Saved chunk embeddings for {}",
                    id.as_str()
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "Failed to embed transcript chunks"),
    }
    // Keep the whole-recording vector in sync too (cheap; one extra embed).
    match embedder.embed(transcript) {
        Ok(vec) => {
            if let Err(e) = catalog.upsert_embedding(id, &vec).await {
                tracing::warn!(error = %e, "Failed to save embedding to catalog");
            }
        }
        Err(e) => tracing::warn!(error = %e, "Failed to embed transcript"),
    }
}

/// Mint the LLM provider for a step that is about to RUN, launching the local
/// Ollama first when the effective connection needs it (`ollama_launcher`).
/// Every actual LLM execution path (cleanup, summary, tags, titles, in-place
/// polish, the cleanup re-run) resolves its provider through this; validation
/// and "is this configured?" checks keep calling `LlmPostProcessor::provider`
/// directly so a mere settings probe can never spawn a process.
pub(crate) async fn llm_provider_for_run(
    state: &AppState,
    llm_cfg: &LlmPostProcessConfig,
) -> Option<Box<dyn phoneme_core::LlmProvider>> {
    // Resolve first: a disabled/unrecognized provider must short-circuit
    // before any probe or launch happens.
    let provider = state.llm.provider(llm_cfg)?;
    crate::ollama_launcher::ensure_ready(state, llm_cfg).await;
    Some(provider)
}

/// Build the effective LLM config for summaries: start from `[llm_post_process]`
/// and overlay any summary-specific provider / URL / key / model the user set.
/// Each blank summary field inherits the cleanup value, so summaries can run on
/// a fully independent provider+model or simply reuse the cleanup connection.
/// Always `enabled` — summaries have their own on/off gate (`summary.auto` /
/// the explicit on-demand request).
pub fn summary_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    let mut llm = cfg.llm_post_process.clone();
    llm.enabled = true;
    let s = &cfg.summary;
    if !s.provider.trim().is_empty() {
        llm.provider = s.provider.clone();
    }
    if !s.api_url.trim().is_empty() {
        llm.api_url = s.api_url.clone();
    }
    let key = s.api_key_str();
    if !key.trim().is_empty() {
        llm.set_api_key(key.to_string());
    }
    if !s.model.trim().is_empty() {
        llm.model = s.model.clone();
    }
    llm
}

/// Generate an LLM summary of `transcript`, returning `(summary, model)` on
/// success or a human-readable reason on failure — the reason reaches the UI
/// toast verbatim, so it must say WHAT went wrong (a stale endpoint, an
/// unreachable provider, an empty reply), not just that something did.
/// Non-fatal: callers surface the error and continue.
///
/// Summaries reuse the `[llm_post_process]` provider connection (endpoint, API
/// key, provider type) wherever the `[summary]` fields are blank. The
/// post-processor's `enabled` flag is irrelevant here — summarization is gated
/// by its own switch — so we force a working config clone with the summary
/// model/prompt swapped in.
pub async fn generate_summary(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
    // `Result` here is std's two-arg form, NOT the crate's `error::Result`
    // alias that the rest of this module uses — the Err side is a plain
    // user-facing string, not a phoneme error.
) -> std::result::Result<(String, String), String> {
    if transcript.trim().is_empty() {
        return Err("the transcript is empty — nothing to summarize".into());
    }
    let llm_cfg = summary_llm_config(cfg);
    let model = llm_cfg.model.clone();
    let llm = match llm_provider_for_run(state, &llm_cfg).await {
        Some(llm) => llm,
        None => {
            tracing::warn!(
                provider = %llm_cfg.provider,
                "summary requested but no usable LLM provider is configured"
            );
            return Err(format!(
                "no usable AI provider configured (provider \"{}\") — set one under Settings → Post-Processing",
                llm_cfg.provider
            ));
        }
    };
    match run_llm_stage(
        state,
        id,
        PipelineStage::Summarizing,
        &*llm,
        &cfg.summary.prompt,
        transcript,
    )
    .await
    {
        Ok(summary) if !summary.trim().is_empty() => Ok((summary, model)),
        Ok(_) => {
            tracing::warn!("summary LLM returned empty output");
            Err(format!("the model ({model}) returned empty output"))
        }
        Err(e) if stage_skipped(&e) => {
            // The user hit "skip" — not a failure. Pass the bare sentinel
            // through (no "internal error:" wrapper, no endpoint hint) so the
            // GUI's toast layer can tell a skip from a broken provider.
            tracing::info!("summary stage skipped by user");
            Err(STAGE_SKIPPED_REASON.to_string())
        }
        Err(e) => {
            tracing::error!(error = %e, "summary generation failed");
            // Name the endpoint when one is overridden — a stale per-step URL
            // (e.g. left over from trying a different provider) is the classic
            // cause and invisible in a generic message.
            if cfg.summary.api_url.trim().is_empty() {
                Err(e.to_string())
            } else {
                Err(format!(
                    "{e} (summary endpoint override: {})",
                    cfg.summary.api_url
                ))
            }
        }
    }
}

/// Generate a summary (if `summary.auto`) and persist it. Runs as the final
/// pipeline step, after post-processing/cleanup, so it summarizes the text the
/// user will actually see.
/// Whether the auto-summary step will run — drives the `Summarizing` status,
/// which is only set when true so disabled steps never flash in the UI.
fn summary_enabled(cfg: &Config) -> bool {
    cfg.summary.auto
}

/// Whether the auto-tag step will run (same contract as [`summary_enabled`]).
fn auto_tag_enabled(cfg: &Config) -> bool {
    cfg.auto_tag.auto
}

/// Whether this pipeline run should type the transcript at the cursor.
///
/// Only in-place dictations type, and only when the text hasn't already
/// landed: with `[in_place].full_pipeline` + `[in_place].type_first` the
/// recorder spawned a type-only pass that typed the quick transcription the
/// moment it was ready, so this run owns everything else (cleanup, summary,
/// tags, hooks, the library copy) but must NOT type again — the text would
/// land twice. Pure so the decision is testable without an input simulator.
fn pipeline_should_type(in_place: &InPlaceConfig, rec_in_place: bool, transcript: &str) -> bool {
    rec_in_place && !transcript.is_empty() && !(in_place.full_pipeline && in_place.type_first)
}

async fn maybe_auto_summarize(state: &AppState, cfg: &Config, id: &RecordingId, transcript: &str) {
    if !cfg.summary.auto {
        return;
    }
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Summarizing,
    });
    match generate_summary(state, cfg, id, transcript).await {
        Ok((summary, model)) => {
            if let Err(e) = state
                .catalog
                .update_summary(id, &summary, Some(&model))
                .await
            {
                tracing::warn!(error = %e, "failed to persist auto summary");
                state.events.emit(DaemonEvent::SummaryFailed {
                    id: id.clone(),
                    error: e.to_string(),
                });
            } else {
                tracing::info!(id = %id.as_str(), "auto summary saved");
                state
                    .events
                    .emit(DaemonEvent::SummaryUpdated { id: id.clone() });
            }
        }
        Err(reason) => {
            // Auto-summary failed — surface the REAL reason (the transcript
            // itself is fine; only the optional summary step failed).
            state.events.emit(DaemonEvent::SummaryFailed {
                id: id.clone(),
                error: reason,
            });
        }
    }
}

/// Build the effective LLM config for tag suggestions, mirroring
/// `summary_llm_config`: start from `[llm_post_process]` and overlay any
/// auto-tag-specific provider / URL / key / model. Always `enabled` — the
/// auto-tag step has its own gate (`auto_tag.auto` / the on-demand request).
pub fn auto_tag_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    let mut llm = cfg.llm_post_process.clone();
    llm.enabled = true;
    let t = &cfg.auto_tag;
    if !t.provider.trim().is_empty() {
        llm.provider = t.provider.clone();
    }
    if !t.api_url.trim().is_empty() {
        llm.api_url = t.api_url.clone();
    }
    let key = t.api_key_str();
    if !key.trim().is_empty() {
        llm.set_api_key(key.to_string());
    }
    if !t.model.trim().is_empty() {
        llm.model = t.model.clone();
    }
    llm
}

/// Parse the tagger LLM's reply into clean tag names: prefer a JSON string
/// array anywhere in the output (models often wrap it in code fences); fall
/// back to comma/newline splitting. Trims quotes/hashes/bullets, drops empties
/// and case-insensitive duplicates, and caps the list at `max`.
fn parse_tag_names(raw: &str, max: usize) -> Vec<String> {
    let cleaned = raw.trim();
    // Find the FIRST valid JSON string-array anywhere in the reply. Scanning
    // every '[' (instead of slicing first-'[' .. last-']') matters because
    // chatty models wrap the array in bracket-bearing prose — "[1] as cited"
    // before it, "[hope that helps]" after — and the old greedy slice spanned
    // the prose, failed to parse, and comma-split the whole reply into junk
    // candidates. The stream deserializer parses exactly one value starting
    // at each '[' and ignores whatever follows, so trailing prose can't break
    // a well-formed array; a non-string array (e.g. "[1]") just fails fast
    // and the scan moves to the next bracket.
    let jsonish = cleaned
        .char_indices()
        .filter(|(_, c)| *c == '[')
        .find_map(|(start, _)| {
            serde_json::Deserializer::from_str(&cleaned[start..])
                .into_iter::<Vec<String>>()
                .next()?
                .ok()
        });
    let candidates: Vec<String> = jsonish.unwrap_or_else(|| {
        cleaned
            .split([',', '\n', ';'])
            .map(str::to_string)
            .collect()
    });
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for c in candidates {
        let name = c
            .trim()
            .trim_matches(|ch| ch == '"' || ch == '\'' || ch == '`')
            .trim_start_matches(['#', '-', '*', '•'])
            .trim()
            .to_string();
        // Tag names are short labels; anything sentence-length is the model
        // ignoring instructions — drop it rather than creating a junk tag.
        if name.is_empty() || name.len() > 40 {
            continue;
        }
        let key = name.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(name);
        if out.len() >= max {
            break;
        }
    }
    out
}

/// Ask the LLM for tag suggestions for `transcript` and persist them on the
/// recording (replacing any previous suggestions), emitting
/// `TagSuggestionsUpdated` so the UI shows the approval chips. The existing
/// tag list is included in the prompt so the model prefers reusing tags.
/// Non-fatal: failures are logged and leave existing suggestions untouched.
pub async fn suggest_tags(state: &AppState, cfg: &Config, id: &RecordingId, transcript: &str) {
    if transcript.trim().is_empty() {
        return;
    }
    let llm_cfg = auto_tag_llm_config(cfg);
    let llm = match llm_provider_for_run(state, &llm_cfg).await {
        Some(llm) => llm,
        None => {
            tracing::warn!(
                provider = %llm_cfg.provider,
                "tag suggestions requested but no usable LLM provider is configured"
            );
            return;
        }
    };
    // EVERY existing tag (attached or not) — the model reuses these where they
    // fit, so the user's tag vocabulary stays canonical.
    let existing: Vec<String> = match state.catalog.list_all_tags().await {
        Ok(tags) => tags.into_iter().map(|t| t.name).collect(),
        Err(_) => vec![],
    };
    let max = cfg.auto_tag.max_tags.clamp(1, 12) as usize;
    // The mix guidance is appended at run time (not stored), so it holds even
    // for configs that saved an older prompt that over-favored existing tags.
    let prompt = format!(
        "{}\n\nEXISTING TAGS: {}\nSuggest at most {} tags. Reuse existing tags that fit, and add NEW tags for topics the existing ones don't cover.",
        cfg.auto_tag.prompt,
        if existing.is_empty() {
            "(none yet)".to_string()
        } else {
            existing.join(", ")
        },
        max,
    );
    match run_llm_stage(
        state,
        id,
        PipelineStage::Tagging,
        &*llm,
        &prompt,
        transcript,
    )
    .await
    {
        Ok(reply) => {
            let mut names = parse_tag_names(&reply, max);
            // Don't suggest tags the recording already has.
            if let Ok(Some(rec)) = state.catalog.get(id).await {
                let have: Vec<String> = rec.tags.iter().map(|t| t.name.to_lowercase()).collect();
                names.retain(|n| !have.contains(&n.to_lowercase()));
            }
            // Canonicalize against the EXISTING tag set, case-insensitively:
            // a suggested "Code" when the library already has "code" becomes
            // "code" — so a chip can never read as a new tag when it isn't,
            // and approving can never mint a casing-duplicate. The same model
            // emitting "Code" AND "code" collapses to one suggestion.
            let canonical: std::collections::HashMap<String, String> =
                match state.catalog.list_all_tags().await {
                    Ok(tags) => tags
                        .into_iter()
                        .map(|t| (t.name.to_lowercase(), t.name))
                        .collect(),
                    Err(_) => std::collections::HashMap::new(),
                };
            let mut seen = std::collections::HashSet::new();
            names = names
                .into_iter()
                .map(|n| canonical.get(&n.to_lowercase()).cloned().unwrap_or(n))
                .filter(|n| seen.insert(n.to_lowercase()))
                .collect();
            // Auto-accept matches of EXISTING tags when enabled: a suggestion
            // whose tag already exists (any tag, attached anywhere or not) is
            // attached right away; only names that would CREATE a new tag stay
            // behind as approve/dismiss chips.
            let mut accepted = 0usize;
            if cfg.auto_tag.auto_accept_existing && !names.is_empty() {
                let (accept, keep): (Vec<String>, Vec<String>) = names
                    .into_iter()
                    .partition(|n| canonical.contains_key(&n.to_lowercase()));
                names = keep;
                for name in accept {
                    match state.catalog.add_tag(&name, None).await {
                        Ok(tag) => match state.catalog.attach_tag(id, tag.id).await {
                            Ok(()) => {
                                accepted += 1;
                                state
                                    .events
                                    .emit(DaemonEvent::TagAttached { tag_id: tag.id });
                            }
                            Err(e) => tracing::warn!(error = %e, "auto-accept: attach failed"),
                        },
                        Err(e) => tracing::warn!(error = %e, "auto-accept: tag lookup failed"),
                    }
                }
                if accepted > 0 {
                    tracing::info!(id = %id.as_str(), accepted, "auto-accepted existing-tag suggestions");
                }
            }
            if names.is_empty() && accepted == 0 {
                tracing::info!(id = %id.as_str(), "tag suggestion produced nothing new");
                return;
            }
            // Persist the remaining (new-tag) names — empty clears any previous
            // suggestions, which is right when everything was auto-accepted.
            match state.catalog.set_tag_suggestions(id, &names).await {
                Ok(()) => {
                    tracing::info!(id = %id.as_str(), count = names.len(), "tag suggestions saved");
                    state
                        .events
                        .emit(DaemonEvent::TagSuggestionsUpdated { id: id.clone() });
                }
                Err(e) => tracing::warn!(error = %e, "failed to persist tag suggestions"),
            }
        }
        Err(e) => tracing::warn!(error = %e, "tag suggestion LLM call failed"),
    }
}

/// Build the effective LLM config for titles, mirroring `summary_llm_config`:
/// start from `[llm_post_process]` and overlay any title-specific provider /
/// URL / key / model. Always `enabled` — the title step has its own gates
/// (`title.enabled` + `title.use_llm`).
pub fn title_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    let mut llm = cfg.llm_post_process.clone();
    llm.enabled = true;
    let t = &cfg.title;
    if !t.provider.trim().is_empty() {
        llm.provider = t.provider.clone();
    }
    if !t.api_url.trim().is_empty() {
        llm.api_url = t.api_url.clone();
    }
    let key = t.api_key_str();
    if !key.trim().is_empty() {
        llm.set_api_key(key.to_string());
    }
    if !t.model.trim().is_empty() {
        llm.model = t.model.clone();
    }
    llm
}

/// Tame an LLM's title reply into something displayable: first non-empty
/// line, wrapping quotes/markdown and a "Title:" prefix stripped, capped at
/// 8 words, no trailing punctuation. `None` when nothing usable remains —
/// the caller keeps the heuristic title instead.
fn sanitize_llm_title(raw: &str) -> Option<String> {
    let unwrap_quotes = |s: &str| -> String {
        s.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '#' | '_'))
            .trim()
            .to_string()
    };
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = unwrap_quotes(line);
    // Models love announcing "Title: …" despite instructions — and quote the
    // value as often as the whole reply, so unwrap on both sides of the strip.
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .map(|rest| unwrap_quotes(rest.trim()))
        .unwrap_or(line);
    let capped = line
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    let title = capped.trim_end_matches(|c: char| !c.is_alphanumeric());
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Generate and store the recording's auto title. The heuristic (first
/// meaningful sentence) is computed from the clean transcript, falling back
/// to the raw one; when `[title].use_llm` is on AND a provider resolves, the
/// LLM's title replaces it — and the heuristic remains the fallback on ANY
/// LLM problem (no provider, call error, unusable reply).
///
/// The write goes through `Catalog::set_title`'s auto-guard, so a title the
/// user typed is never overwritten — a retranscribe refreshes auto titles
/// and silently skips user-owned ones. Best-effort: a failure here costs
/// only the title. No status flip and no events — the title lands before
/// `TranscriptionDone`, whose refresh paints it.
async fn maybe_auto_title(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
    raw_transcript: &str,
) {
    if !cfg.title.enabled {
        return;
    }
    let heuristic = phoneme_core::title::heuristic_title(transcript)
        .or_else(|| phoneme_core::title::heuristic_title(raw_transcript));
    let mut title = heuristic;
    if cfg.title.use_llm && !transcript.trim().is_empty() {
        let title_cfg = title_llm_config(cfg);
        if let Some(llm) = llm_provider_for_run(state, &title_cfg).await {
            match llm.process(&cfg.title.prompt, transcript).await {
                Ok(reply) => match sanitize_llm_title(&reply) {
                    Some(t) => title = Some(t),
                    None => {
                        tracing::warn!("title LLM returned nothing usable; keeping the heuristic")
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "title LLM call failed; keeping the heuristic")
                }
            }
        }
    }
    let Some(title) = title else {
        // Nothing usable in the transcript either — leave any stored title be.
        return;
    };
    match state.catalog.set_title(id, Some(&title), true).await {
        Ok(true) => tracing::info!(id = %id.as_str(), title = %title, "auto title saved"),
        Ok(false) => {
            tracing::debug!(id = %id.as_str(), "auto title skipped — the user owns this title")
        }
        Err(e) => tracing::warn!(error = %e, "failed to persist auto title"),
    }
}

/// Run the auto-tag step when enabled (`auto_tag.auto`). Best-effort and
/// quiet: the transcript is already saved; only the optional suggestions step
/// is affected by a failure.
async fn maybe_auto_tag(state: &AppState, cfg: &Config, id: &RecordingId, transcript: &str) {
    if !cfg.auto_tag.auto {
        return;
    }
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Tagging,
    });
    suggest_tags(state, cfg, id, transcript).await;
}

/// Finalize an in-flight item canceled by the user: move the inbox file out of
/// `processing/`, mark the recording `Cancelled`, and emit the cancel events.
/// Best-effort — logs (but doesn't propagate) errors so a cancel always settles.
/// `Cancelled` is terminal like the failed states, but it is the user's own
/// action — it never shows up as a failure in the list or the failed panel.
async fn finalize_canceled(state: &AppState, id: &RecordingId) {
    if let Err(e) = state
        .catalog
        .update_status(id, RecordingStatus::Cancelled)
        .await
    {
        tracing::warn!(error = %e, "cancel: failed to set status");
    }
    if let Err(e) = state.inbox.finish_cancelled(id).await {
        tracing::warn!(error = %e, "cancel: failed to move inbox item out of processing");
    }
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Failed,
    });
    state
        .events
        .emit(DaemonEvent::RecordingCancelled { id: id.clone() });
    tracing::info!(id = %id, "in-flight recording canceled by user");
}

/// Process a single claimed payload through the full pipeline.
///
/// Updates catalog, fires events, moves inbox files to done/ or failed/.
pub async fn run(
    state: &AppState,
    mut payload: HookPayload,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<()> {
    let id = payload.id.clone();
    state
        .events
        .emit(DaemonEvent::TranscriptionStarted { id: id.clone() });
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Transcribing,
    });

    // Transcribe — reuse the process-wide client (AppState) so the HTTP
    // connection pool to the local whisper-server stays warm across items.
    let cfg = state.config.load();
    let audio_path = std::path::Path::new(&payload.audio_path).to_path_buf();
    // Filter empty string to None — frontend sends "" for "auto-detect"
    let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());

    // Hold the whisper-server permit for the whole final transcription so the
    // streaming preview backs off and can't starve it (the "Whisper timed out
    // after 60s" bug). Acquiring waits for any in-flight preview tick to finish.
    // Crucially, a model-override swap (below) happens UNDER this permit, so the
    // preview and any other final transcription never run while the bundled
    // server is mid-restart for a one-job model override (#49).
    let _whisper_permit = state.whisper_sem.acquire().await;

    // Apply this recording's one-time model override (if any), scoped to JUST
    // this job. `override_guard` restores the configured model on every exit
    // path (success, error, cancel) via Drop, so the override can never leak
    // onto a later job or persist in config. `whisper_cfg` is the per-job
    // transcription config the provider is built from.
    let requested_override = state.pending_overrides.lock().unwrap().remove(&id);
    let (whisper_cfg, override_guard) =
        apply_model_override(state, &cfg.whisper, requested_override).await;
    // Dial the port the bundled server is ACTUALLY listening on: the
    // supervisor falls back to a free port when the configured one is held by
    // another app, and publishes the live value in `whisper_ports`.
    let whisper_cfg = state.whisper_ports.apply(&cfg, &whisper_cfg);
    let provider = state.transcription.provider(&whisper_cfg, &cfg.diarization);

    // Report transcription to the unified AI-activity ("brain") popout via the
    // Transcribing stage of LlmActivity: a start event naming the model/file,
    // then a done event with timing + size once it finishes. This lets the same
    // popout that shows cleanup/summary also surface what the STT engine is up to.
    let model_label = {
        use phoneme_core::config::TranscriptionBackend as TB;
        match whisper_cfg.provider {
            TB::Local => std::path::Path::new(&whisper_cfg.model_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("local model")
                .to_string(),
            _ => whisper_cfg.model.clone(),
        }
    };
    let provider_label = format!("{:?}", whisper_cfg.provider).to_lowercase();
    let file_label = audio_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    state.events.emit(DaemonEvent::LlmActivity {
        id: id.clone(),
        stage: PipelineStage::Transcribing,
        prompt: format!(
            "Transcribing with {provider_label} · {model_label}\nfile: {file_label}\nlanguage: {}",
            language.as_deref().unwrap_or("auto-detect")
        ),
        delta: String::new(),
        done: false,
    });
    let transcribe_started = std::time::Instant::now();

    // Race transcription against cancellation. Dropping the transcribe future on
    // cancel tears down the in-flight HTTP request (reqwest futures are
    // cancel-safe); the native path stops at the next stage boundary.
    let transcription = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            state.events.emit(DaemonEvent::LlmActivity {
                id: id.clone(),
                stage: PipelineStage::Transcribing,
                prompt: String::new(),
                delta: "✕ canceled".into(),
                done: true,
            });
            finalize_canceled(state, &id).await;
            return Ok(());
        }
        res = provider.transcribe_with_segments(&audio_path, language.as_deref()) => match res {
            Ok(t) => t,
            Err(e) => {
                let transient = matches!(
                    e,
                    phoneme_core::Error::WhisperUnreachable { .. }
                        | phoneme_core::Error::WhisperTimeout { .. }
                );
                state.events.emit(DaemonEvent::LlmActivity {
                    id: id.clone(),
                    stage: PipelineStage::Transcribing,
                    prompt: String::new(),
                    delta: if transient {
                        format!("✕ {e} — will retry")
                    } else {
                        format!("✕ failed: {e}")
                    },
                    done: true,
                });
                // A transient error (server down / restarting, request timed
                // out) must NOT bury the item in failed/ — the queue worker
                // requeues it and retries with backoff, so a whisper-server
                // blip never costs a recording. Only permanent errors (bad
                // audio, 4xx, decode failures) take the failed path.
                if !transient {
                    state
                        .catalog
                        .update_status(&id, RecordingStatus::TranscribeFailed)
                        .await?;
                    state
                        .inbox
                        .finish_failed(&id, "whisper_error", &e.to_string())
                        .await?;
                    state.events.emit(DaemonEvent::TranscriptionFailed {
                        id: id.clone(),
                        error: e.to_string(),
                    });
                }
                return Err(e);
            }
        }
    };

    // Checkpoint between transcription and post-processing.
    if cancel.is_cancelled() {
        finalize_canceled(state, &id).await;
        return Ok(());
    }

    // The segment timeline is machine truth (it describes the raw whisper
    // output, not the LLM-cleaned text), so split it off here — the rest of
    // the pipeline only transforms the text.
    let phoneme_core::transcription::Transcription {
        text: transcript,
        segments,
    } = transcription;

    // Finish the Transcribing activity entry with timing + size for the popout.
    {
        let secs = transcribe_started.elapsed().as_secs_f32();
        let chars = transcript.chars().count();
        let words = transcript.split_whitespace().count();
        state.events.emit(DaemonEvent::LlmActivity {
            id: id.clone(),
            stage: PipelineStage::Transcribing,
            prompt: String::new(),
            delta: format!("✓ {words} words · {chars} chars in {secs:.1}s"),
            done: true,
        });
    }

    // Restore the configured whisper model (if this job overrode it) BEFORE
    // releasing the permit, so the bundled server is swapped back while the
    // preview is still gated — the resumed preview then runs the configured
    // model, not this job's one-time override. Dropping the guard pings the
    // supervisor; for non-override jobs it's a no-op. (Both early-return paths
    // above — cancel and transcribe error — drop this guard implicitly, so the
    // model is always restored.)
    drop(override_guard);

    // Release the whisper-server permit now that transcription is done — LLM
    // post-processing and hooks below don't touch the server, so the preview
    // can resume immediately.
    drop(_whisper_permit);

    // Preserve the raw Whisper output as the "original" transcript regardless
    // of whether LLM post-processing rewrites the live version. Users can
    // always restore to this via "View original transcript" in the detail pane.
    let raw_transcript = transcript.clone();

    // Optional LLM post-processing. Non-fatal: on any failure we keep the raw
    // transcript. `provider()` returns None when disabled or provider = none.
    let mut transcript = transcript;
    let mut cleanup_model: Option<String> = None;
    if let Some(llm) = llm_provider_for_run(state, &cfg.llm_post_process).await {
        // The list/detail/activity views read the DB status, so it tracks the
        // stage events step for step — "transcribing" no longer covers the
        // whole pipeline. Best-effort: a status write failing must not kill
        // the stage itself.
        let _ = state
            .catalog
            .update_status(&id, RecordingStatus::CleaningUp)
            .await;
        state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::CleaningUp,
        });
        let cleanup_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                finalize_canceled(state, &id).await;
                return Ok(());
            }
            r = run_llm_stage(
                state,
                &id,
                PipelineStage::CleaningUp,
                &*llm,
                &cfg.llm_post_process.prompt,
                &transcript,
            ) => r,
        };
        match cleanup_result {
            Ok(processed) => {
                tracing::info!("LLM post-processing succeeded");
                transcript = processed;
                cleanup_model = Some(cfg.llm_post_process.model.clone());
            }
            Err(e) => {
                tracing::error!(error = %e, "LLM post-processing failed, falling back to raw transcript");
            }
        }
    }

    payload.transcript = transcript.clone();
    // Record the model that actually ran. Use the per-job whisper config so a
    // one-time model-override re-transcription stores the override's model file
    // stem (for the local backend) rather than the configured default.
    payload.model = std::path::Path::new(&whisper_cfg.model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // `transcript` = LLM-processed (or raw if LLM is disabled/failed).
    // `raw_transcript` = raw Whisper output, always preserved as the original.
    state
        .catalog
        .update_transcript(&id, &transcript, &raw_transcript, &payload.model)
        .await?;

    // Persist the provider's segment timeline (replacing any previous one —
    // a retranscribe describes a new machine output). Best-effort: a failure
    // here costs the timeline views, not the recording.
    if let Err(e) = state.catalog.replace_segments(&id, &segments).await {
        tracing::warn!(id = %id.as_str(), "failed to persist transcript segments: {e}");
    }

    // Record which post-processing model was used and whether diarization was
    // actually applied. Diarization is only meaningfully "applied" when it
    // produced multi-speaker labels — both the local diarizer (`assign_speakers`)
    // and the cloud providers emit a "[Speaker N]: " prefix and fall back to
    // plain, unlabeled text when diarization is off, fails, or finds ≤1 speaker.
    // So a recording is diarized iff its raw (pre-cleanup) transcript carries
    // those labels — not merely because the setting is enabled. (Check the raw
    // transcript: LLM cleanup may strip the labels from the live version.)
    let diarized = raw_transcript.contains("[Speaker ");
    state
        .catalog
        .update_processing_meta(&id, cleanup_model.as_deref(), diarized)
        .await?;

    // Auto title, right after the transcript settles (cleanup included) and
    // before TranscriptionDone fires, so the refreshed list shows the title
    // together with the new text. A retranscribe re-runs this and refreshes
    // auto titles; user-set titles are protected inside.
    maybe_auto_title(state, &cfg, &id, &transcript, &raw_transcript).await;

    let recording = state.catalog.get(&id).await?;
    if let Some(rec) = recording {
        if pipeline_should_type(&cfg.in_place, rec.in_place, &transcript) {
            tracing::info!(id = %id.as_str(), "in-place dictation: typing transcript at cursor");
            // This is the FULL-PIPELINE dictation path ([in_place].full_pipeline
            // = true) without `type_first`: the text lands only after every
            // configured step. (With `type_first` the recorder's type-only
            // pass already typed it, so `pipeline_should_type` keeps this run
            // from landing the text twice.) The insertion itself (typing vs
            // clipboard-paste, input-simulator failure modes) lives in
            // `in_place::type_at_cursor`, shared with the fast lane.
            // Best-effort — a failure is logged loudly but never fails the
            // recording.
            if let Err(e) =
                crate::in_place::type_at_cursor(&transcript, &cfg.in_place.type_mode).await
            {
                tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to insert transcript");
            }
        }
    }

    let embedder_guard = state.embedder.read().await;
    if let Some(embedder) = embedder_guard.as_ref() {
        embed_and_store(embedder, &state.catalog, &id, &transcript).await;
    }
    drop(embedder_guard);

    // Hooks are optional. When `run_on_transcribe` is off, finalize the
    // recording right after transcription without firing hooks or the webhook;
    // the user can run them on demand later via "Re-fire hook". This is what
    // lets a re-transcription update the text without re-triggering side effects
    // (e.g. re-appending to an Obsidian daily note).
    if !cfg.hook.run_on_transcribe {
        // Auto-summary is the final step — runs after post-processing so it
        // summarizes the text the user actually sees; auto-tag suggestions ride
        // along after it (approve-to-apply, so they're side-effect free).
        // Statuses only flip when the step will actually run, so a recording
        // with summaries off never flashes "Summarizing".
        if summary_enabled(&cfg) {
            let _ = state
                .catalog
                .update_status(&id, RecordingStatus::Summarizing)
                .await;
        }
        maybe_auto_summarize(state, &cfg, &id, &transcript).await;
        if auto_tag_enabled(&cfg) {
            let _ = state
                .catalog
                .update_status(&id, RecordingStatus::Tagging)
                .await;
        }
        maybe_auto_tag(state, &cfg, &id, &transcript).await;
        state
            .catalog
            .update_status(&id, RecordingStatus::Done)
            .await?;
        state.events.emit(DaemonEvent::TranscriptionDone {
            id: id.clone(),
            transcript: transcript.clone(),
        });
        state.inbox.finish_done(&id, &payload).await?;
        return Ok(());
    }

    state
        .catalog
        .update_status(&id, RecordingStatus::HookRunning)
        .await?;
    state.events.emit(DaemonEvent::TranscriptionDone {
        id: id.clone(),
        transcript: transcript.clone(),
    });

    // Hooks.
    state
        .events
        .emit(DaemonEvent::HookStarted { id: id.clone() });
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::RunningHook,
    });
    payload.metadata = HookMetadata::current();

    let mut final_exit_code = 0;
    let mut total_duration = 0;
    let mut last_cmd = String::new();

    let expanded_cfg = cfg.expanded().unwrap_or_else(|_| (**cfg).clone());

    for cmd in &expanded_cfg.hook.commands {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            continue;
        }
        let runner = HookRunner::new(
            trimmed.to_string(),
            Duration::from_secs(cfg.hook.timeout_secs),
        );
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
                state
                    .catalog
                    .update_status(&id, RecordingStatus::HookFailed)
                    .await?;
                state
                    .inbox
                    .finish_failed(&id, "hook_failed", &e.to_string())
                    .await?;
                state.events.emit(DaemonEvent::HookFailed {
                    id,
                    error: e.to_string(),
                });
                return Err(e);
            }
        }
    }

    // Conditional keyword-triggered hooks: run each rule whose pattern matches
    // the (post-processed) transcript. These are supplementary — a failure is
    // logged but does NOT fail the recording, since the always-on commands
    // above already succeeded.
    for rule in &expanded_cfg.hook.keyword_rules {
        if !rule.matches(&payload.transcript) {
            continue;
        }
        let cmd = rule.command.trim();
        if cmd.is_empty() {
            continue;
        }
        let runner = HookRunner::new(cmd.to_string(), Duration::from_secs(cfg.hook.timeout_secs));
        match runner.run(&payload).await {
            Ok(result) => {
                total_duration += result.duration_ms;
                last_cmd = rule.command.clone();
                if result.exit_code != 0 {
                    tracing::warn!(pattern = %rule.pattern, exit_code = result.exit_code, "keyword hook exited non-zero");
                } else {
                    tracing::info!(pattern = %rule.pattern, "keyword hook ran");
                }
            }
            Err(e) => {
                tracing::warn!(pattern = %rule.pattern, error = %e, "keyword hook failed to run");
            }
        }
    }

    // Auto-summary is the final step — runs after post-processing and hooks so
    // it summarizes the text the user actually sees; auto-tag suggestions ride
    // along after it (approve-to-apply, so they're side-effect free).
    if summary_enabled(&cfg) {
        let _ = state
            .catalog
            .update_status(&id, RecordingStatus::Summarizing)
            .await;
    }
    maybe_auto_summarize(state, &cfg, &id, &payload.transcript).await;
    if auto_tag_enabled(&cfg) {
        let _ = state
            .catalog
            .update_status(&id, RecordingStatus::Tagging)
            .await;
    }
    maybe_auto_tag(state, &cfg, &id, &payload.transcript).await;

    state
        .catalog
        .update_hook_result(&id, &last_cmd, final_exit_code, total_duration)
        .await?;
    state
        .catalog
        .update_status(&id, RecordingStatus::Done)
        .await?;
    state.inbox.finish_done(&id, &payload).await?;
    state.events.emit(DaemonEvent::HookDone {
        id,
        exit_code: final_exit_code,
    });

    // Only POST when a non-blank URL is configured — an empty string (e.g. the
    // Settings field was filled then cleared) must not fire a request per run.
    if let Some(url) = cfg
        .hook
        .webhook_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        if let Err(e) = state
            .webhook
            .post(
                url,
                Duration::from_secs(cfg.hook.timeout_secs),
                &payload,
                // The [webhook] policy (SSRF guard) is read per-run so a
                // config reload takes effect without restarting the daemon.
                &cfg.webhook,
            )
            .await
        {
            tracing::warn!(error = %e, "webhook failed");
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "pipeline_test.rs"]
mod pipeline_test;
