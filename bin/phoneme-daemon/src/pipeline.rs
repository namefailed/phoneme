//! Pipeline orchestration — the stage after the queue: every claimed
//! recording flows through [`run`] on its way from WAV to finished library
//! entry.
//!
//! Stage order (each optional stage is gated by config and non-fatal):
//! transcribe (with segments + diarization) → LLM cleanup → auto title →
//! in-place typing (full-pipeline dictations only) → embed for semantic
//! search → hooks + keyword hooks → auto summary → auto tags → done +
//! webhook. Results land in the catalog as they settle; progress is
//! broadcast as `PipelineStageChanged` / `LlmActivity` events, and the
//! catalog status column tracks the stages step for step.
//!
//! Invariants owned here:
//! - Whisper-server serialization: the final transcription holds the
//!   `whisper_sem` permit for its whole STT call, so the live preview can't
//!   starve it. A one-job model-override swap happens under the permit too, so
//!   nothing else talks to the bundled server mid-restart.
//! - One-job model overrides: `apply_model_override` reads the recording's
//!   pending override, publishes it to the supervisor (local backend) or
//!   clones it into the per-job config (cloud), and the drop guard restores
//!   the configured model on every exit path. The process-global config is
//!   never mutated.
//! - Effective ports: the per-job whisper config is rewritten through
//!   `whisper_ports.apply` right before building the provider, so the request
//!   dials the port the server actually bound after any fallback.
//! - Cancellation: the queue worker passes a token; transcription races it
//!   directly, and checkpoints between stages finalize a canceled item
//!   (`finalize_canceled`: status `Cancelled`, inbox `finish_cancelled`,
//!   cancel events) so a cancel always settles.
//! - Transient vs permanent failures: unreachable/timeout STT errors leave the
//!   inbox item claimed for the worker to requeue and retry; permanent errors
//!   quarantine it in `failed/` and mark the row failed.
//! - Skip: the in-flight LLM stage races `skip_stage` and aborts as a
//!   non-fatal stage failure when the user hits skip.
//!
//! The helpers here (`run_llm_stage`, `generate_summary`, `suggest_tags`,
//! `embed_and_store`, the per-step `*_llm_config` builders) are also called by
//! the IPC re-run handlers, so on-demand re-runs behave exactly like their
//! pipeline counterparts.

use crate::app_state::{AppState, WhisperModelOverride};
use phoneme_core::config::{
    Config, InPlaceConfig, LlmPostProcessConfig, TranscriptionBackend, WhisperConfig, WhisperMode,
    WhisperServerRole,
};
use phoneme_core::error::Result;
use phoneme_core::transcription::DiarizationTrack;
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
/// attempt fails with `WhisperUnreachable`, which the queue worker retries with
/// backoff — so this bound only avoids a needless first failure, it's never a
/// correctness requirement.
const WHISPER_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Restores the configured whisper model when a one-job model override goes out
/// of scope. Dropping it pings the supervisor to swap the bundled server back,
/// so the override is undone on every pipeline exit path (success, transcribe
/// error, cancel) without each path having to remember to clear it. A no-op
/// (`inner` is `None`) when the job had no override or used a cloud backend (no
/// server to restore).
pub(crate) struct WhisperOverrideGuard {
    inner: Option<Arc<WhisperModelOverride>>,
}

impl Drop for WhisperOverrideGuard {
    fn drop(&mut self) {
        if let Some(o) = self.inner.take() {
            // Clearing the override makes the supervisor restart the bundled
            // server back onto the configured model for subsequent jobs.
            o.set(None);
        }
    }
}

/// Apply a recording's one-time whisper model override, scoped to this job,
/// returning the per-job [`WhisperConfig`] to build the provider from plus a
/// guard that restores the configured model on drop.
///
/// - No override (or a blank one): returns the configured config unchanged with
///   a no-op guard — the steady-state path.
/// - Local bundled backend: the override is a model file the single shared
///   server must load, so we publish it to the supervisor (which does one
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
pub(crate) async fn apply_model_override(
    state: &AppState,
    configured: &WhisperConfig,
    requested: Option<String>,
) -> (WhisperConfig, WhisperOverrideGuard) {
    // The main (final-transcription) server is the only server a re-transcribe /
    // queued pipeline override ever targets, so it keeps the historic behavior:
    // publish to `whisper_model_override`, wait on `main_generation`.
    apply_model_override_for_role(state, WhisperServerRole::Main, configured, requested).await
}

/// Role-aware sibling of [`apply_model_override`]: publish the one-job override to
/// the slot the chosen server's supervisor actually reads, and wait on that
/// server's spawn generation, so an in-place dictation routed through the preview
/// or dictation server loads its requested model on the *right* server instead of
/// silently swapping the main one. `role` selects the (override slot, generation)
/// pair; everything else (the blank-skip, the cloud branch, the per-job
/// `model_path` pin for labels, the readiness wait, the restore guard) is
/// identical to the main-role path. Preview2 never appears here — the 2nd
/// meeting-track server never carries a dictation (see
/// [`crate::app_state::in_place_override_role`]).
pub(crate) async fn apply_model_override_for_role(
    state: &AppState,
    role: WhisperServerRole,
    configured: &WhisperConfig,
    requested: Option<String>,
) -> (WhisperConfig, WhisperOverrideGuard) {
    let model = match requested {
        Some(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => return (configured.clone(), WhisperOverrideGuard { inner: None }),
    };

    // The override slot the chosen server's supervisor reads, and that server's
    // spawn-generation getter — picked once so the publish, the readiness gate,
    // and the restore guard all target the same server.
    let override_slot: &Arc<WhisperModelOverride> = match role {
        WhisperServerRole::Preview => &state.preview_model_override,
        WhisperServerRole::InPlace => &state.dictation_model_override,
        // Main is the default; Preview2 never routes a dictation override (it's
        // meeting-only), so fall back to the main slot rather than no-op.
        _ => &state.whisper_model_override,
    };
    let generation = move |state: &AppState| -> u64 {
        match role {
            WhisperServerRole::Preview => state.whisper_ports.preview_generation(),
            WhisperServerRole::InPlace => state.whisper_ports.dictation_generation(),
            _ => state.whisper_ports.main_generation(),
        }
    };

    let mut whisper_cfg = configured.clone();
    match configured.provider {
        TranscriptionBackend::Local => {
            tracing::info!(model = %model, role = %role.label(), "re-transcribe: applying one-job whisper model override");
            // Capture the supervisor's spawn generation BEFORE requesting the swap,
            // so the readiness wait below can tell the freshly-respawned server
            // (loading our model) apart from the old one it replaces.
            let override_gen = generation(state);
            // Publish the override; the supervisor swaps the server's model.
            override_slot.set(Some(model.clone()));
            // Pin the per-job model_path so the activity label and the stored
            // model reflect the override (the local provider talks to the server
            // over HTTP and ignores model_path itself).
            whisper_cfg.model_path = model;
            // Only the bundled server is ours to wait on; External is a
            // user-managed endpoint we never restart. The URL is re-resolved on
            // every poll because the override restart re-runs the supervisor's
            // port probe — the server can come back on a different port than the
            // one it left (its preferred port freed up, or a fresh fallback was
            // assigned).
            if matches!(
                configured.mode,
                WhisperMode::BundledModel | WhisperMode::BundledDownload
            ) {
                let poll_state = state.clone();
                let poll_cfg = whisper_cfg.clone();
                let fresh_state = state.clone();
                wait_for_whisper_ready(
                    move || {
                        let cfg = poll_state.config.load();
                        poll_state
                            .whisper_ports
                            .apply(&cfg, &poll_cfg)
                            .server_base_url()
                    },
                    move || generation(&fresh_state) != override_gen,
                    WHISPER_READY_TIMEOUT,
                )
                .await;
            }
            (
                whisper_cfg,
                WhisperOverrideGuard {
                    inner: Some(override_slot.clone()),
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

/// Apply a recording's one-time [`crate::app_state::PendingRerun`] overrides onto
/// a per-job config clone: the hooks toggle, the post-processing opt-out, and the
/// Re-run → "All" cleanup/summary/title values. Pure — it touches no global state
/// — so `run` builds a private config for the job and the process-global config is
/// never mutated. Mutating the global here would race a concurrent `ReloadConfig`
/// and could leak the forced-on pipeline onto another queued job.
fn apply_rerun_overrides(
    mut cfg: phoneme_core::Config,
    rerun: crate::app_state::PendingRerun,
) -> phoneme_core::Config {
    if let Some(rh) = rerun.run_hooks {
        cfg.hook.run_on_transcribe = rh;
    }
    // Post-processing opt-out: under the Playbook executor a step runs iff it's
    // a member of the resolved recipe, so disabling the legacy flag isn't enough
    // — drop the cleanup Transform step from the per-job clone's default recipe
    // so no Transform runs and the run yields the raw machine transcript.
    // (Disabling the flag too keeps any non-recipe path honest; both are confined
    // to the clone — the persisted recipe is never touched.)
    if rerun.post_process == Some(false) {
        cfg.llm_post_process.enabled = false;
        if let Some(recipe) = cfg.recipes.iter_mut().find(|r| r.id == DEFAULT_RECIPE_ID) {
            recipe.steps.retain(|s| s != "cleanup");
        }
    }
    // Re-run → "All": force the whole pipeline on and layer in the per-step
    // values (applied after the opt-out so cleanup stays on for an "All" run).
    if let Some(ov) = rerun.all_overrides {
        cfg.llm_post_process.enabled = true;
        if let Some(p) = ov.cleanup_provider {
            cfg.llm_post_process.provider = p;
        }
        if let Some(m) = ov.cleanup_model {
            cfg.llm_post_process.model = m;
        }
        if let Some(p) = ov.cleanup_prompt {
            cfg.llm_post_process.prompt = p;
        }
        if let Some(u) = ov.cleanup_api_url {
            cfg.llm_post_process.api_url = u;
        }
        // The recipe executor reads each step's prompt/model/provider/url from
        // its Playbook entry, so mirror the one-shot overrides into the matching
        // entries on this per-job clone. Without this, a Re-run → "All" with a
        // custom cleanup/summary/title model or prompt would be silently ignored.
        // The clone is discarded after the run, so the persisted Playbook is
        // untouched; the api_key is never set here (it inherits each section).
        let (cp, cm, cpr, cu) = (
            cfg.llm_post_process.provider.clone(),
            cfg.llm_post_process.model.clone(),
            cfg.llm_post_process.prompt.clone(),
            cfg.llm_post_process.api_url.clone(),
        );
        if let Some(entry) = cfg.playbook.iter_mut().find(|e| e.id == "cleanup") {
            entry.llm.provider = cp;
            entry.llm.model = cm;
            entry.llm.prompt = cpr;
            entry.llm.api_url = cu;
        }
        cfg.summary.auto = true;
        if let Some(m) = ov.summary_model {
            cfg.summary.model = m;
        }
        if let Some(p) = ov.summary_prompt {
            cfg.summary.prompt = p;
        }
        // A chosen title model implies "run the title step with it" — enable the
        // step and the LLM path even if globally off.
        if let Some(m) = ov.title_model {
            cfg.title.enabled = true;
            cfg.title.use_llm = true;
            cfg.title.model = m;
        }
        // Mirror the (possibly forced/overridden) `[summary]` and `[title]`
        // sections onto their Playbook entries so the Enrichment steps run with
        // the "All" values. We copy the raw section fields (blank stays blank):
        // the executor re-applies the same inherit-on-blank overlay against
        // `[llm_post_process]` via `entry_llm_config` at resolve time, so the
        // resolved connection matches what `summary_llm_config` / `title_llm_config`
        // would have produced. Precompute against immutable borrows, then mutate
        // the entries (avoids aliasing `cfg`).
        let summary_entry = section_to_entry_llm(&cfg.summary);
        let title_entry = section_to_entry_llm(&cfg.title);
        if let Some(entry) = cfg.playbook.iter_mut().find(|e| e.id == "summary") {
            entry.llm = summary_entry;
        }
        if let Some(entry) = cfg.playbook.iter_mut().find(|e| e.id == "title") {
            entry.llm = title_entry;
        }
        // The executor gates each step on recipe membership, so forcing the
        // legacy flags on isn't enough — ensure cleanup/title/summary are members
        // of the per-job clone's default recipe (in canonical order). Tags
        // membership is left as-is: legacy "All" never forced auto-tagging on (it
        // only set `summary.auto`/`title.enabled`/cleanup), so tags still run only
        // if they were already a member. Confined to the clone.
        ensure_default_recipe_steps(&mut cfg, &["cleanup", "title", "summary"]);
    }
    cfg
}

/// The canonical order of the built-in default recipe's steps — used to slot a
/// forced-on Re-run "All" step back into its rightful position.
const CANONICAL_DEFAULT_STEPS: [&str; 4] = ["cleanup", "title", "summary", "auto_tag"];

/// Copy a per-step section (`[summary]` / `[title]`) into a `PlaybookLlm` entry
/// half, raw (blank stays blank) — the executor re-applies the inherit-on-blank
/// overlay against `[llm_post_process]` at resolve time, so the resolved
/// connection matches what the legacy `*_llm_config` builder would have produced.
/// The prompt is the section prompt; the key is the section key (downstream
/// inheritance fills a blank). `timeout_secs` is irrelevant (the entry overlay
/// doesn't read it). Used only by the Re-run "All" mirror, on the config clone.
fn section_to_entry_llm<S: PerStepLlmSection>(section: &S) -> phoneme_core::config::PlaybookLlm {
    let mut e = phoneme_core::config::PlaybookLlm {
        provider: section.provider().to_string(),
        model: section.model().to_string(),
        prompt: section.prompt().to_string(),
        api_url: section.api_url().to_string(),
        ..Default::default()
    };
    let key = section.api_key();
    if !key.trim().is_empty() {
        e.set_api_key(key.to_string());
    }
    e
}

/// The fields `section_to_entry_llm` reads off a per-step LLM section.
trait PerStepLlmSection {
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    fn prompt(&self) -> &str;
    fn api_url(&self) -> &str;
    fn api_key(&self) -> &str;
}

impl PerStepLlmSection for phoneme_core::config::SummaryConfig {
    fn provider(&self) -> &str {
        &self.provider
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn prompt(&self) -> &str {
        &self.prompt
    }
    fn api_url(&self) -> &str {
        &self.api_url
    }
    fn api_key(&self) -> &str {
        self.api_key_str()
    }
}

impl PerStepLlmSection for phoneme_core::config::TitleConfig {
    fn provider(&self) -> &str {
        &self.provider
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn prompt(&self) -> &str {
        &self.prompt
    }
    fn api_url(&self) -> &str {
        &self.api_url
    }
    fn api_key(&self) -> &str {
        self.api_key_str()
    }
}

/// Ensure each id in `want` is a member of the per-job clone's default recipe,
/// inserting any missing one at its canonical position (cleanup → title →
/// summary → auto_tag). A no-op for ids already present; never reorders or
/// duplicates. Used only by the Re-run "All" path, on the config clone.
fn ensure_default_recipe_steps(cfg: &mut phoneme_core::Config, want: &[&str]) {
    let Some(recipe) = cfg.recipes.iter_mut().find(|r| r.id == DEFAULT_RECIPE_ID) else {
        return;
    };
    for id in want {
        if recipe.steps.iter().any(|s| s == id) {
            continue;
        }
        // Insert at the position implied by the canonical order: before the
        // first existing step whose canonical rank is higher than this id's.
        let rank = |s: &str| CANONICAL_DEFAULT_STEPS.iter().position(|c| c == &s);
        let my_rank = rank(id);
        let at = recipe
            .steps
            .iter()
            .position(|s| match (rank(s), my_rank) {
                (Some(sr), Some(mr)) => sr > mr,
                _ => false,
            })
            .unwrap_or(recipe.steps.len());
        recipe.steps.insert(at, (*id).to_string());
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
async fn wait_for_whisper_ready(
    base_url: impl Fn() -> String,
    is_fresh: impl Fn() -> bool,
    timeout: Duration,
) {
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
        // Accept readiness only once the supervisor has actually (re)spawned the
        // server (`is_fresh` — its spawn generation advanced past the override) AND
        // it answers health. Without the freshness gate, the old server still up in
        // the swap gap could answer 200 and we'd transcribe with the wrong model.
        let health = format!("{}/health", base_url().trim_end_matches('/'));
        if is_fresh() {
            if let Ok(resp) = client.get(&health).send().await {
                if resp.status().is_success() {
                    return;
                }
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
/// verbatim inside `SummaryFailed.error`, where the toast layer matches on the
/// phrase to report "skipped" instead of a failure — keep
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
    // (1) Start event carrying the verbatim prompt. Kept in `exact` so the same
    // text can be persisted to the AI-activity log once the session ends.
    let exact = provider.exact_prompt(prompt, text);
    state.events.emit(DaemonEvent::LlmActivity {
        id: id.clone(),
        stage,
        prompt: exact.clone(),
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
            // `skip_stage` is a global broadcast: `notify_waiters()` wakes every
            // in-flight LLM stage at once. The user's ⏭ targets the queue's active
            // item only, so honor the wake solely for the recording currently in
            // `state.processing`; any other concurrent stage (an on-demand re-run,
            // which is never the processing item) re-arms and keeps streaming
            // instead of being collaterally aborted.
            _ = skip_active_queue_item(state, id) => {
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

    // Persist the completed session so the 🧠 AI-activity log survives an app
    // restart (the live event stream above is in-memory only). Best-effort: a
    // log-write failure must never fail the stage. A skipped/errored stage isn't
    // persisted — only completed prompt→response sessions are logged.
    if let Ok(ref out) = result {
        if let Err(e) = state
            .catalog
            .insert_ai_activity(id.as_str(), stage.as_str(), &exact, out)
            .await
        {
            tracing::warn!(error = %e, ?stage, "failed to persist AI activity");
        }
    }

    result
}

/// Resolve to a completed future only when the user's ⏭ ("skip current step") is
/// meant for `id` — i.e. `id` is the queue's currently-processing item.
///
/// `state.skip_stage` is a single global `Notify`: `notify_waiters()` wakes every
/// LLM stage that happens to be streaming. That over-fires when an on-demand
/// re-run (`rerun_cleanup` / `rerun_summary`) is in flight at the same time as a
/// queue stage — one ⏭ would abort both. The queue worker publishes the one
/// active item in `state.processing`, so we treat the wake as a skip only for
/// that item; for anything else we re-arm and wait again (re-runs are never the
/// processing item, so they never get skipped this way). Until the matching
/// item's skip arrives this future stays pending, so the `select!` arm parks and
/// the stream wins.
async fn skip_active_queue_item(state: &AppState, id: &RecordingId) {
    loop {
        state.skip_stage.notified().await;
        let is_active = match state.processing.lock() {
            Ok(slot) => matches!(slot.as_ref(), Some((pid, _)) if pid == id),
            // A poisoned lock shouldn't strand the user's skip on the real
            // active item, but we can't prove identity — don't skip.
            Err(_) => false,
        };
        if is_active {
            return;
        }
    }
}
use std::time::Duration;

/// Embed `transcript` for semantic search and persist both representations:
/// per-chunk vectors (the high-recall path that powers paraphrase matching) and
/// the legacy whole-recording vector (kept so anything that still reads the old
/// `embeddings` table — and the search fallback — stays consistent).
///
/// Shared by every place a transcript becomes final or changes (pipeline, manual
/// edit, cleanup re-run, retroactive backfill) so all paths embed identically.
/// Best-effort: a failure is logged, never fatal — search degrades rather than
/// failing the recording.
pub(crate) async fn embed_and_store(
    embedder: std::sync::Arc<Embedder>,
    catalog: &Catalog,
    id: &RecordingId,
    transcript: &str,
) {
    // ONNX inference is blocking CPU work, so run it on the blocking pool rather
    // than on an async worker (where it would stall concurrent IPC + event
    // delivery for the duration). The catalog writes stay on the async runtime.
    // Taking an owned `Arc<Embedder>` (cloned out of the read-lock by the caller)
    // is what lets the work move into `spawn_blocking`.
    let text = transcript.to_string();
    let emb = embedder.clone();
    match tokio::task::spawn_blocking(move || emb.embed_chunks(&text)).await {
        Ok(Ok(chunks)) => {
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
        Ok(Err(e)) => tracing::warn!(error = %e, "Failed to embed transcript chunks"),
        Err(e) => tracing::warn!(error = %e, "chunk-embed task failed"),
    }
    // Keep the whole-recording vector in sync too (cheap; one extra embed).
    let text = transcript.to_string();
    match tokio::task::spawn_blocking(move || embedder.embed(&text)).await {
        Ok(Ok(vec)) => {
            if let Err(e) = catalog.upsert_embedding(id, &vec).await {
                tracing::warn!(error = %e, "Failed to save embedding to catalog");
            }
        }
        Ok(Err(e)) => tracing::warn!(error = %e, "Failed to embed transcript"),
        Err(e) => tracing::warn!(error = %e, "embed task failed"),
    }
}

/// Mint the LLM provider for a step that is about to run, launching the local
/// Ollama first when the effective connection needs it (`ollama_launcher`).
/// Every LLM execution path (cleanup, summary, tags, titles, in-place polish,
/// the cleanup re-run) resolves its provider through this; validation and "is
/// this configured?" checks keep calling `LlmPostProcessor::provider` directly so
/// a settings probe can never spawn a process.
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
/// Each blank summary field inherits the cleanup value, so summaries can run on a
/// fully independent provider+model or just reuse the cleanup connection. Always
/// enabled — summaries have their own on/off gate (`summary.auto` / the explicit
/// on-demand request).
pub fn summary_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    let s = &cfg.summary;
    cfg.llm_post_process
        .resolve_step(&s.provider, &s.api_url, s.api_key_str(), &s.model)
}

/// Generate an LLM summary of `transcript`, returning `(summary, model)` on
/// success or a human-readable reason on failure — the reason reaches the UI
/// toast verbatim, so it must say what went wrong (a stale endpoint, an
/// unreachable provider, an empty reply), not just that something did. Non-fatal:
/// callers surface the error and continue.
///
/// Summaries reuse the `[llm_post_process]` provider connection (endpoint, API
/// key, provider type) wherever the `[summary]` fields are blank. The
/// post-processor's `enabled` flag is irrelevant here — summarization is gated by
/// its own switch — so we force a working config clone with the summary
/// model/prompt swapped in.
pub async fn generate_summary(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
    // `Result` here is std's two-arg form, not the crate's `error::Result` alias
    // that the rest of this module uses — the Err side is a plain user-facing
    // string, not a phoneme error.
) -> std::result::Result<(String, String), String> {
    // The legacy/IPC path: provider+prompt+endpoint-hint come from the
    // `[summary]`/`[llm_post_process]` sections. The recipe executor calls
    // `generate_summary_with` instead, passing the resolved Playbook entry.
    let endpoint_hint = cfg.summary.api_url.trim();
    let endpoint_hint = (!endpoint_hint.is_empty()).then(|| endpoint_hint.to_string());
    generate_summary_with(
        state,
        id,
        transcript,
        summary_llm_config(cfg),
        &cfg.summary.prompt,
        endpoint_hint.as_deref(),
    )
    .await
}

/// The summary generator's core, parameterized by an already-resolved LLM config
/// and prompt so both the legacy/IPC path ([`generate_summary`], which reads
/// `[summary]`) and the recipe executor (which reads the migrated `summary`
/// Playbook entry) share one implementation — same events, same
/// skip/empty/error classification, same recorded model. `endpoint_hint`, when
/// `Some`, names an overridden endpoint in a real-error message (the classic
/// stale-URL cause); a skip and "not configured" never carry it.
pub(crate) async fn generate_summary_with(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
    endpoint_hint: Option<&str>,
) -> std::result::Result<(String, String), String> {
    if transcript.trim().is_empty() {
        return Err("the transcript is empty — nothing to summarize".into());
    }
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
        prompt,
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
            // (e.g. left over from trying a different provider) is a common cause
            // and invisible in a generic message.
            match endpoint_hint {
                Some(url) => Err(format!("{e} (summary endpoint override: {url})")),
                None => Err(e.to_string()),
            }
        }
    }
}

/// Whether this pipeline run should type the transcript at the cursor.
///
/// Only in-place dictations type, and only when the text hasn't already landed.
/// The one subtlety is the recorder's "type-first" pass: with
/// `[in_place].full_pipeline` + `[in_place].type_first` the recorder typed the
/// quick transcription the moment it was ready, so this run owns everything else
/// (cleanup, summary, tags, hooks, the library copy) but must not type again, or
/// the text would land twice.
///
/// `recipe_routed` is true when a custom-hotkey in-place binding named a recipe
/// (the recording was routed to the full pipeline so the recipe could run). In
/// that case the recorder skips the type-first pass — the recipe reshapes the
/// text, so the quick raw transcription is the wrong thing to type — and this run
/// is the sole insertion of the recipe's result. So the suppression mirrors the
/// recorder exactly: a type-first pass ran iff `full_pipeline && type_first &&
/// !recipe_routed`, and this run types iff one did not. Pure, so the decision is
/// testable without an input simulator.
fn pipeline_should_type(
    in_place: &InPlaceConfig,
    rec_in_place: bool,
    recipe_routed: bool,
    transcript: &str,
) -> bool {
    rec_in_place
        && !transcript.is_empty()
        && !(in_place.full_pipeline && in_place.type_first && !recipe_routed)
}

/// Run the summary enrichment step for a recording the recipe says should
/// summarize. Recipe membership is the gate — the legacy `summary.auto` flag was
/// folded into membership by the migration, so it isn't re-checked here. The
/// provider/prompt/model come from the resolved `summary` Playbook entry
/// (`llm_cfg` + `prompt`), so editing that entry in the UI changes what the
/// summary step does. Same events / persistence / classification as the legacy
/// path.
///
/// Returns `Some(error)` only when the summary step actually failed (a user-skip
/// and "not configured" are non-failures) — the caller folds that into the
/// terminal status and persists the message. `None` means no failure.
async fn run_summary_step(
    state: &AppState,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Summarizing,
    });
    // The endpoint hint names an overridden URL in a real-error message — only
    // when the entry actually overrode the URL (a non-blank entry api_url that
    // differs from the inherited cleanup connection is the stale-URL culprit).
    // Hint with the resolved URL when set.
    let endpoint_hint = {
        let u = llm_cfg.api_url.trim();
        (!u.is_empty()).then(|| u.to_string())
    };
    match generate_summary_with(
        state,
        id,
        transcript,
        llm_cfg,
        prompt,
        endpoint_hint.as_deref(),
    )
    .await
    {
        Ok((summary, model)) => {
            if let Err(e) = state
                .catalog
                .update_summary(id, &summary, Some(&model))
                .await
            {
                tracing::warn!(error = %e, "failed to persist auto summary");
                let msg = e.to_string();
                state.events.emit(DaemonEvent::SummaryFailed {
                    id: id.clone(),
                    error: msg.clone(),
                });
                Some(msg)
            } else {
                tracing::info!(id = %id.as_str(), "auto summary saved");
                state
                    .events
                    .emit(DaemonEvent::SummaryUpdated { id: id.clone() });
                None
            }
        }
        Err(reason) => {
            // Auto-summary failed — surface the real reason (the transcript
            // itself is fine; only the optional summary step failed). A user-skip
            // carries the sentinel and isn't a failure for the terminal status.
            let skipped = reason == STAGE_SKIPPED_REASON;
            state.events.emit(DaemonEvent::SummaryFailed {
                id: id.clone(),
                error: reason.clone(),
            });
            if skipped {
                None
            } else {
                Some(reason)
            }
        }
    }
}

/// Build the effective LLM config for tag suggestions, mirroring
/// `summary_llm_config`: start from `[llm_post_process]` and overlay any
/// auto-tag-specific provider / URL / key / model. Always enabled — the auto-tag
/// step has its own gate (`auto_tag.auto` / the on-demand request).
pub fn auto_tag_llm_config(cfg: &Config) -> LlmPostProcessConfig {
    let t = &cfg.auto_tag;
    cfg.llm_post_process
        .resolve_step(&t.provider, &t.api_url, t.api_key_str(), &t.model)
}

/// Parse the tagger LLM's reply into clean tag names: prefer a JSON string
/// array anywhere in the output (models often wrap it in code fences); fall
/// back to comma/newline splitting. Trims quotes/hashes/bullets, drops empties
/// and case-insensitive duplicates, and caps the list at `max`.
fn parse_tag_names(raw: &str, max: usize) -> Vec<String> {
    let cleaned = raw.trim();
    // Find the first valid JSON string-array anywhere in the reply. We scan every
    // '[' rather than slicing first-'[' .. last-']' because chatty models wrap the
    // array in bracket-bearing prose — "[1] as cited" before it, "[hope that
    // helps]" after — and a greedy slice would span the prose, fail to parse, and
    // comma-split the whole reply into junk candidates. The stream deserializer
    // parses one value starting at each '[' and ignores what follows, so trailing
    // prose can't break a well-formed array; a non-string array (e.g. "[1]") fails
    // fast and the scan moves to the next bracket.
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
/// `TagSuggestionsUpdated` so the UI shows the approval chips. The existing tag
/// list is included in the prompt so the model prefers reusing tags. Non-fatal:
/// failures are logged and leave existing suggestions untouched. Returns
/// `Some(error)` only when the tag step actually failed (an LLM call error) — the
/// caller folds that into the terminal status and persists the message. An empty
/// transcript, a missing provider, a user-skip, or "nothing new to suggest" are
/// all non-failures (`None`).
pub async fn suggest_tags(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
) -> Option<String> {
    // The legacy/IPC path: the LLM config + base prompt come from the
    // `[auto_tag]` section. The recipe executor calls `suggest_tags_with`
    // instead, passing the resolved `auto_tag` Playbook entry. The `max_tags` /
    // `auto_accept_existing` behavior knobs stay in `[auto_tag]` either way.
    suggest_tags_with(
        state,
        cfg,
        id,
        transcript,
        auto_tag_llm_config(cfg),
        &cfg.auto_tag.prompt,
    )
    .await
}

/// The tag-suggester's core, parameterized by an already-resolved LLM config +
/// base prompt so the legacy/IPC path (reads `[auto_tag]`) and the recipe
/// executor (reads the migrated `auto_tag` entry) share one implementation —
/// same existing-tags guidance, canonicalization, auto-accept, events, and
/// `set_tag_model` write. `base_prompt` is the user's instruction; the runtime
/// existing-tags mix guidance is appended here. `max_tags` / `auto_accept_existing`
/// remain `[auto_tag]` behavior knobs (not part of the LLM entry).
pub(crate) async fn suggest_tags_with(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    base_prompt: &str,
) -> Option<String> {
    if transcript.trim().is_empty() {
        return None;
    }
    let llm = match llm_provider_for_run(state, &llm_cfg).await {
        Some(llm) => llm,
        None => {
            tracing::warn!(
                provider = %llm_cfg.provider,
                "tag suggestions requested but no usable LLM provider is configured"
            );
            return None;
        }
    };
    // Every existing tag (attached or not) — the model reuses these where they
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
        base_prompt,
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
            // Record which model ran the auto-tagger (the detail provenance line
            // names it), once per run and before the suggest-vs-auto-accept branch
            // — so it sticks even when every suggestion was auto-accepted and
            // nothing is left to approve.
            if let Err(e) = state.catalog.set_tag_model(id, &llm_cfg.model).await {
                tracing::warn!(error = %e, "failed to persist tag model");
            }
            let mut names = parse_tag_names(&reply, max);
            // Don't suggest tags the recording already has.
            if let Ok(Some(rec)) = state.catalog.get(id).await {
                let have: Vec<String> = rec.tags.iter().map(|t| t.name.to_lowercase()).collect();
                names.retain(|n| !have.contains(&n.to_lowercase()));
            }
            // Canonicalize against the existing tag set, case-insensitively: a
            // suggested "Code" when the library already has "code" becomes "code"
            // — so a chip can't read as a new tag when it isn't, and approving
            // can't mint a casing-duplicate. The same model emitting "Code" and
            // "code" collapses to one suggestion.
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
            // Auto-accept matches of existing tags when enabled: a suggestion
            // whose tag already exists (attached anywhere or not) is attached
            // right away; only names that would create a new tag stay behind as
            // approve/dismiss chips.
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
                return None;
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
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "tag suggestion LLM call failed");
            // Best-effort: no suggestions added; surface the failure for a toast +
            // the terminal status. A user-skip carries the sentinel and isn't a
            // failure.
            let skipped = stage_skipped(&e);
            let msg = e.to_string();
            state.events.emit(DaemonEvent::TagFailed {
                id: id.clone(),
                error: msg.clone(),
            });
            if skipped {
                None
            } else {
                Some(msg)
            }
        }
    }
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
    // Models tend to announce "Title: …" despite instructions, and quote the value
    // as often as the whole reply, so unwrap on both sides of the strip.
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

/// Generate and store the recording's auto title. Runs only when the recipe
/// contains a `title` step (membership is the gate — the legacy `title.enabled`
/// flag was folded into membership by the migration, so it isn't re-checked
/// here). The heuristic (first meaningful sentence) is computed from the clean
/// transcript, falling back to the raw one; when `[title].use_llm` is on and a
/// provider resolves, the LLM's title replaces it — and the heuristic stays the
/// fallback on any LLM problem (no provider, call error, unusable reply). The LLM
/// provider/model/prompt come from the resolved `title` Playbook entry (`llm_cfg`
/// and `prompt`), so editing that entry changes the title step. `use_llm` stays a
/// `[title]` behavior knob (not part of the LLM entry; the migration kept it in
/// `[title]`).
///
/// The write goes through `Catalog::set_title`'s auto-guard, so a title the user
/// typed is never overwritten — a retranscribe refreshes auto titles and silently
/// skips user-owned ones. Best-effort: a failure here costs only the title. No
/// status flip and no events — the title lands before `TranscriptionDone`, whose
/// refresh paints it.
/// Returns `Some(error)` only when the title step actually failed (an LLM call
/// error) — the caller folds that into the recording's terminal status and
/// persists the message. A heuristic title, "nothing usable" (heuristic kept),
/// or a user-owned title are all non-failures (`None`).
async fn run_title_step(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
    raw_transcript: &str,
    title_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    let heuristic = phoneme_core::title::heuristic_title(transcript)
        .or_else(|| phoneme_core::title::heuristic_title(raw_transcript));
    let mut title = heuristic;
    // The model that produced the title, recorded for the provenance line — only
    // set when an LLM title is actually accepted; a heuristic title has none.
    let mut title_model: Option<String> = None;
    let mut failure: Option<String> = None;
    if cfg.title.use_llm && !transcript.trim().is_empty() {
        if let Some(llm) = llm_provider_for_run(state, &title_cfg).await {
            match llm.process(prompt, transcript).await {
                Ok(reply) => match sanitize_llm_title(&reply) {
                    Some(t) => {
                        title = Some(t);
                        title_model = Some(title_cfg.model.clone());
                    }
                    None => {
                        tracing::warn!("title LLM returned nothing usable; keeping the heuristic")
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "title LLM call failed; keeping the heuristic");
                    // Best-effort: the heuristic title (or none) is kept; surface
                    // the LLM failure for a toast + the terminal status.
                    let msg = e.to_string();
                    state.events.emit(DaemonEvent::TitleFailed {
                        id: id.clone(),
                        error: msg.clone(),
                    });
                    failure = Some(msg);
                }
            }
        }
    }
    let Some(title) = title else {
        // Nothing usable in the transcript either — leave any stored title be.
        return failure;
    };
    match state
        .catalog
        .set_title(id, Some(&title), true, title_model.as_deref())
        .await
    {
        Ok(true) => tracing::info!(id = %id.as_str(), title = %title, "auto title saved"),
        Ok(false) => {
            tracing::debug!(id = %id.as_str(), "auto title skipped — the user owns this title")
        }
        Err(e) => tracing::warn!(error = %e, "failed to persist auto title"),
    }
    failure
}

/// Run the auto-tag step when enabled (`auto_tag.auto`). Best-effort and
/// quiet: the transcript is already saved; only the optional suggestions step
/// is affected by a failure.
async fn run_tags_step(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    transcript: &str,
    llm_cfg: LlmPostProcessConfig,
    prompt: &str,
) -> Option<String> {
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Tagging,
    });
    suggest_tags_with(state, cfg, id, transcript, llm_cfg, prompt).await
}

/// Write a recording's terminal status at the end of the pipeline: `Done` on a
/// clean run, or the earliest failed optional step's status — and in that case
/// persist its message on the row (`error_kind` = the status string,
/// `error_message` = the reason) so the failed panel and `phoneme list` show why,
/// surviving a restart. Runs after `update_transcript` (which clears any stale
/// error from a prior run), so a recording that re-runs cleanly ends with no error
/// and a fresh failure overwrites an old one. The status write propagates its
/// error (it's the pipeline's final commit); the error write is best-effort
/// logging on top.
async fn finalize_step_status(
    state: &AppState,
    id: &RecordingId,
    failure: Option<(RecordingStatus, String)>,
) -> Result<()> {
    match failure {
        Some((status, message)) => {
            state.catalog.update_status(id, status).await?;
            if let Err(e) = state
                .catalog
                .update_error(id, status.as_str(), &message)
                .await
            {
                tracing::warn!(error = %e, "failed to persist step-failure error");
            }
        }
        None => {
            state
                .catalog
                .update_status(id, RecordingStatus::Done)
                .await?;
        }
    }
    Ok(())
}

/// Finalize an in-flight item canceled by the user: move the inbox file out of
/// `processing/`, mark the recording `Cancelled`, and emit the cancel events.
/// Best-effort — logs (but doesn't propagate) errors so a cancel always settles.
/// `Cancelled` is terminal like the failed states, but it's the user's own action
/// — it never shows up as a failure in the list or the failed panel.
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

// ── Recipe executor ──────────────────────────────────────────────────────────
// The pipeline's cleanup → title → summary → tags interior is driven by a
// resolved Playbook recipe instead of a hardcoded sequence. The executor is a
// thin dispatcher over the existing streaming/persistence primitives
// (`run_llm_stage`, `generate_summary_with`, `suggest_tags_with`, the title + tag
// persistence) — it never reimplements their event/persistence logic, so parity
// (and IPC-re-run identicality) comes for free. Each built-in step reads its
// migrated Playbook entry: membership gates whether it runs, the entry's
// `PlaybookLlm` (overlaid on `[llm_post_process]`) gives its provider/model/
// prompt. Hooks stay outside the recipe loop (`cfg.hook`).

/// The recipe id the normal recording pipeline runs.
const DEFAULT_RECIPE_ID: &str = "default";

/// One resolved built-in step, ready to dispatch. The schema's `custom:<key>`
/// enrichment has no persistence path yet, so it is carried as a forward-compat
/// no-op (warn-only) rather than dropped silently or treated as a failure.
enum ResolvedStep {
    /// A Transform (cleanup-style) step: rewrite the running transcript in place.
    /// Carries the entry-derived, inheritance-resolved LLM config + its prompt.
    Transform {
        llm_cfg: LlmPostProcessConfig,
        prompt: String,
        /// Which transcript this step reads: the running text (default, chaining)
        /// or the raw base transcription.
        input: phoneme_core::config::StepInput,
    },
    /// A deterministic transform (`PlaybookKind::FillerRemoval`): rewrite the
    /// running transcript by stripping filler words in pure Rust — no provider,
    /// no network. Runs in the same in-memory rewrite phase as `Transform`,
    /// carrying the `[filler]` config it reads.
    FillerRemoval {
        cfg: phoneme_core::config::FillerConfig,
    },
    /// Enrichment writing the recording title. Carries the entry-resolved LLM
    /// config + prompt so the title step reads the migrated `title` Playbook
    /// entry, not the legacy `[title]` section.
    Title {
        llm_cfg: LlmPostProcessConfig,
        prompt: String,
    },
    /// Enrichment writing the summary. Carries the entry-resolved LLM config +
    /// prompt from the migrated `summary` Playbook entry.
    Summary {
        llm_cfg: LlmPostProcessConfig,
        prompt: String,
    },
    /// Enrichment writing tag suggestions. Carries the entry-resolved LLM config
    /// and prompt from the migrated `auto_tag` Playbook entry.
    Tags {
        llm_cfg: LlmPostProcessConfig,
        prompt: String,
    },
    /// An enrichment whose target has no backing store yet (`custom:<key>` or an
    /// unrecognized target). No-op + warn; never fails the recording.
    UnsupportedEnrichment { target: String },
    /// A side-effect step (Playbook entry of `kind: Hook`): run a shell command
    /// and/or POST a webhook, gated by the entry's keyword trigger. Honors the
    /// entry's `required` flag (fail the recording vs. surface-and-continue).
    Hook {
        hook: phoneme_core::config::PlaybookHook,
    },
}

/// Overlay a Playbook entry's LLM half onto a clone of `[llm_post_process]`,
/// mirroring the daemon's per-step `*_llm_config` builders exactly: `enabled`
/// forced true, each blank field inheriting the cleanup value (inherit-on-blank),
/// and the API key inherited from the cleanup section unless the entry carries its
/// own non-blank key. Built-in migrated entries carry no key, so they inherit just
/// as the legacy pipeline did.
fn entry_llm_config(
    cfg: &Config,
    entry: &phoneme_core::config::PlaybookLlm,
) -> LlmPostProcessConfig {
    cfg.llm_post_process.resolve_step(
        &entry.provider,
        &entry.api_url,
        entry.api_key_str(),
        &entry.model,
    )
}

/// Resolve the migrated Enrichment entry for a given `target` ("summary" /
/// "tags" / "title") into the same `(LlmPostProcessConfig, prompt)` pair the
/// recipe executor dispatches with — so on-demand re-runs (SuggestTags,
/// rerun_summary) read the same Playbook entry the auto-pipeline does, not the
/// legacy `[summary]` / `[auto_tag]` sections. The first Enrichment whose trimmed
/// `target` matches wins; returns `None` when no such entry exists (a user deleted
/// it), letting callers fall back to the legacy path so behavior is never worse
/// than today.
pub(crate) fn entry_config_for_target(
    cfg: &Config,
    target: &str,
) -> Option<(LlmPostProcessConfig, String)> {
    use phoneme_core::config::PlaybookKind;
    cfg.playbook
        .iter()
        .find(|e| e.kind == PlaybookKind::Enrichment && e.target.trim() == target)
        .map(|e| (entry_llm_config(cfg, &e.llm), e.llm.prompt.clone()))
}

/// Resolve the base `(LlmPostProcessConfig, prompt)` for an on-demand Re-run
/// Cleanup from the migrated `cleanup` Playbook entry — so editing the Cleanup
/// entry in the Playbook changes what a Re-run Cleanup does, just like
/// `entry_config_for_target` does for the summary/tags re-runs. Cleanup is a
/// `Transform` (it rewrites the running text), so it has no Enrichment target and
/// `entry_config_for_target` can't find it; this resolver matches by `id ==
/// "cleanup"` and `kind == Transform` instead.
///
/// Falls back to the legacy `[llm_post_process]` config + prompt when no such
/// entry exists (a user deleted it), so behavior is never worse than today. The
/// one-shot Re-run overrides layer on top of whichever base this returns.
pub(crate) fn cleanup_entry_config(cfg: &Config) -> (LlmPostProcessConfig, String) {
    use phoneme_core::config::PlaybookKind;
    cfg.playbook
        .iter()
        .find(|e| e.id == "cleanup" && e.kind == PlaybookKind::Transform)
        .map(|e| (entry_llm_config(cfg, &e.llm), e.llm.prompt.clone()))
        .unwrap_or_else(|| {
            let llm = cfg.llm_post_process.clone();
            let prompt = llm.prompt.clone();
            (llm, prompt)
        })
}

/// Layer a Re-run modal's one-shot `model` / `prompt` overrides on top of a base
/// `(LlmPostProcessConfig, prompt)`. A non-empty (after trimming) override
/// replaces the corresponding base value; `None` or a whitespace-only override is
/// ignored so the modal's empty fields never clobber the entry's configured
/// model/prompt. The single source of truth for this layering, shared by
/// `rerun_summary`, `rerun_cleanup`, and their tests so no test re-implements the
/// production rule. Only model+prompt layer here; provider/api_url/api_key
/// overrides (cleanup-only) are applied by the caller around this base.
pub(crate) fn apply_oneshot_overrides(
    base_llm: LlmPostProcessConfig,
    base_prompt: String,
    model: Option<&str>,
    prompt: Option<&str>,
) -> (LlmPostProcessConfig, String) {
    let mut llm = base_llm;
    let mut resolved_prompt = base_prompt;
    if let Some(m) = model {
        let m = m.trim();
        if !m.is_empty() {
            llm.model = m.to_string();
        }
    }
    if let Some(p) = prompt {
        if !p.trim().is_empty() {
            resolved_prompt = p.to_string();
        }
    }
    (llm, resolved_prompt)
}

/// Resolve `recipe_id` into an ordered list of dispatchable steps.
///
/// `recipe_id` is `default` for every normal recording and the firing custom
/// hotkey's `recipe_id` otherwise. Picks that recipe from `cfg.recipes`; if it's
/// missing (a user deleted a custom recipe a binding still points at, or the
/// requested id was `default` on an empty config) it falls back to the `default`
/// recipe — first from `cfg.recipes`, then the seeded `default_recipes()` — so a
/// stale binding degrades to the standard pipeline rather than panicking or
/// running nothing. Each step id then maps to its `PlaybookEntry`; a dangling id
/// (entry deleted) or a `Hook` entry (hooks run outside the loop) is skipped with
/// a warning. An empty recipe yields an empty list: a bare transcribe-only run.
fn resolve_recipe(cfg: &Config, recipe_id: &str) -> Vec<ResolvedStep> {
    use phoneme_core::config::PlaybookKind;

    let seeded = phoneme_core::config::default_recipes();
    // The requested recipe, falling back to the `default` recipe (config then
    // seed) when it's missing — a deleted custom recipe a binding still names
    // must never run nothing or panic.
    let find_in = |id: &str| {
        cfg.recipes
            .iter()
            .find(|r| r.id == id)
            .or_else(|| seeded.iter().find(|r| r.id == id))
    };
    let recipe = find_in(recipe_id).or_else(|| {
        if recipe_id != DEFAULT_RECIPE_ID {
            tracing::warn!(
                recipe = %recipe_id,
                "custom-hotkey recipe not found; falling back to the `default` recipe"
            );
            find_in(DEFAULT_RECIPE_ID)
        } else {
            None
        }
    });
    let Some(recipe) = recipe else {
        tracing::warn!(recipe = %recipe_id, "no matching recipe found; running transcribe-only");
        return Vec::new();
    };

    let mut steps = Vec::with_capacity(recipe.steps.len());
    for step_id in &recipe.steps {
        let Some(entry) = cfg.playbook.iter().find(|e| &e.id == step_id) else {
            tracing::warn!(step = %step_id, "recipe references a missing Playbook entry; skipping");
            continue;
        };
        match entry.kind {
            PlaybookKind::Transform => steps.push(ResolvedStep::Transform {
                llm_cfg: entry_llm_config(cfg, &entry.llm),
                prompt: entry.llm.prompt.clone(),
                input: entry.input,
            }),
            PlaybookKind::FillerRemoval => steps.push(ResolvedStep::FillerRemoval {
                cfg: cfg.filler.clone(),
            }),
            PlaybookKind::Enrichment => {
                let target = entry.target.trim();
                match target {
                    "title" => steps.push(ResolvedStep::Title {
                        llm_cfg: entry_llm_config(cfg, &entry.llm),
                        prompt: entry.llm.prompt.clone(),
                    }),
                    "summary" => steps.push(ResolvedStep::Summary {
                        llm_cfg: entry_llm_config(cfg, &entry.llm),
                        prompt: entry.llm.prompt.clone(),
                    }),
                    "tags" => steps.push(ResolvedStep::Tags {
                        llm_cfg: entry_llm_config(cfg, &entry.llm),
                        prompt: entry.llm.prompt.clone(),
                    }),
                    other => steps.push(ResolvedStep::UnsupportedEnrichment {
                        target: other.to_string(),
                    }),
                }
            }
            PlaybookKind::Hook => {
                // A Hook entry runs as a recipe step (shell command and/or webhook
                // POST), gated by its keyword trigger — see `run_hook_steps`. An
                // entry with neither a command nor a URL is a no-op, so skip it
                // rather than push an empty step.
                if entry.hook.command.trim().is_empty() && entry.hook.webhook_url.trim().is_empty()
                {
                    tracing::warn!(step = %step_id, "Hook entry has no command or webhook; skipping");
                } else {
                    steps.push(ResolvedStep::Hook {
                        hook: entry.hook.clone(),
                    });
                }
            }
        }
    }
    steps
}

/// Run the recipe's Transform steps (cleanup) — the in-memory text rewrites that
/// must happen before the transcript-commit writes. Returns the (possibly
/// rewritten) transcript and the model that produced the last transform (the
/// `cleanup_model` recorded in processing meta).
///
/// Each Transform: best-effort `CleaningUp` status + `PipelineStageChanged`, then
/// a cancel-raced `run_llm_stage`. On success the running transcript is replaced
/// and feeds the next Transform; on a real failure (not a user-skip) the earliest
/// `CleanupFailed` is recorded in `step_failure`. Cancellation inside a Transform
/// finalizes and returns `None` via the `Cancelled` signal — the caller treats
/// that as a return from `run`.
///
/// The default recipe has exactly one Transform (cleanup); the loop also supports
/// chained Transforms (each rewrites the text), with only the last transform's
/// model recorded as `cleanup_model`.
async fn run_transform_steps(
    state: &AppState,
    id: &RecordingId,
    cancel: &tokio_util::sync::CancellationToken,
    steps: &[ResolvedStep],
    transcript: String,
    step_failure: &mut Option<(RecordingStatus, String)>,
) -> std::result::Result<
    (
        String,
        Option<String>,
        Vec<phoneme_core::catalog::TranscriptVersion>,
    ),
    Canceled,
> {
    use phoneme_core::catalog::TranscriptVersion;
    use phoneme_core::config::StepInput;
    // The immutable raw transcription, for steps configured to read `Base` rather
    // than the running (chained) text.
    let base = transcript.clone();
    let mut transcript = transcript;
    let mut cleanup_model: Option<String> = None;
    // Record each step's output. The caller prepends the raw ASR as idx 0, so these
    // are the post-step versions (idx 1, 2, …) of the chain.
    let mut versions: Vec<TranscriptVersion> = Vec::new();
    for step in steps {
        // Deterministic filler removal: a pure, instant text rewrite — no provider,
        // no network, never fails. It rewrites the running transcript like an LLM
        // Transform and feeds the next step, so it chains with cleanup either way.
        // It doesn't set `cleanup_model` (no model ran).
        if let ResolvedStep::FillerRemoval { cfg } = step {
            transcript = phoneme_core::filler::strip_fillers(&transcript, cfg);
            versions.push(TranscriptVersion {
                idx: versions.len() as i64 + 1,
                step_id: Some("filler".to_string()),
                label: Some("Filler removal".to_string()),
                model: None,
                text: transcript.clone(),
            });
            continue;
        }
        let ResolvedStep::Transform {
            llm_cfg,
            prompt,
            input,
        } = step
        else {
            continue;
        };
        // Per-step input: `Base` reads the raw transcription, else the running
        // (chained) text so steps compound toward a better transcript.
        let source = if *input == StepInput::Base {
            &base
        } else {
            &transcript
        };
        let Some(llm) = llm_provider_for_run(state, llm_cfg).await else {
            // No usable provider for this Transform — same as the legacy gate
            // (`llm_provider_for_run` None): skip silently, keep the transcript.
            continue;
        };
        // The list/detail/activity views read the DB status, so it tracks the
        // stage events step for step. Best-effort: a status write failing must not
        // kill the stage itself.
        let _ = state
            .catalog
            .update_status(id, RecordingStatus::CleaningUp)
            .await;
        state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::CleaningUp,
        });
        let cleanup_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                finalize_canceled(state, id).await;
                return Err(Canceled);
            }
            r = run_llm_stage(state, id, PipelineStage::CleaningUp, &*llm, prompt, source) => r,
        };
        match cleanup_result {
            Ok(processed) => {
                tracing::info!("LLM post-processing succeeded");
                transcript = processed;
                cleanup_model = Some(llm_cfg.model.clone());
                versions.push(TranscriptVersion {
                    idx: versions.len() as i64 + 1,
                    step_id: Some("transform".to_string()),
                    label: Some(format!("Cleanup ({})", llm_cfg.model)),
                    model: Some(llm_cfg.model.clone()),
                    text: transcript.clone(),
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "LLM post-processing failed, falling back to raw transcript");
                // Best-effort step: the raw transcript is kept and the recording
                // stays usable — surface the failure for a toast without flipping to
                // a terminal status. (Carries the skip sentinel when skipped.)
                let msg = e.to_string();
                state.events.emit(DaemonEvent::CleanupFailed {
                    id: id.clone(),
                    error: msg.clone(),
                });
                if !stage_skipped(&e) {
                    step_failure.get_or_insert((RecordingStatus::CleanupFailed, msg));
                }
            }
        }
    }
    Ok((transcript, cleanup_model, versions))
}

/// Marker that a step was canceled mid-flight; the caller returns `Ok(())` from
/// `run` (the cancel has already been finalized via `finalize_canceled`).
struct Canceled;

/// Re-derive the cleaned timing variant: take the recording's raw machine words,
/// realign them to `cleaned_text`, and store the result in the `*_clean` tables so
/// the Timeline/Synced views can match the cleaned panel text. The raw
/// machine-truth timeline is never touched. Best-effort + gated by
/// `editor.resync_views_on_edit` (the same switch manual-edit re-flow uses); a
/// realign that can't map the text leaves the cleaned variant absent, and the
/// views fall back to raw. Shared by the pipeline and `rerun_cleanup`.
pub(crate) async fn reflow_cleaned_timing(state: &AppState, id: &RecordingId, cleaned_text: &str) {
    if !state.config.load().editor.resync_views_on_edit {
        return;
    }
    let raw_words = match state.catalog.words_for(id).await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(id = %id.as_str(), error = %e, "cleaned re-flow: could not load raw words");
            return;
        }
    };
    let Some(r) = phoneme_core::realign::realign_transcript(cleaned_text, &raw_words) else {
        return;
    };
    // Guard a TOCTOU: `rerun_cleanup` runs this off the inbox queue, so a
    // concurrent retranscribe (queue worker) can commit fresh raw words between
    // our read above and the writes below. Re-read and bail if they shifted — the
    // writer that changed them re-derives the cleaned variant itself, so writing
    // our now-stale alignment would clobber the correct one. (Best-effort: it
    // closes the wide LLM-call window down to these two adjacent awaits.)
    match state.catalog.words_for(id).await {
        Ok(current) if current == raw_words => {}
        Ok(_) => {
            tracing::debug!(id = %id.as_str(), "cleaned re-flow: raw words changed under us; skipping stale write");
            return;
        }
        Err(e) => {
            tracing::warn!(id = %id.as_str(), error = %e, "cleaned re-flow: re-read failed; skipping");
            return;
        }
    }
    if let Err(e) = state
        .catalog
        .replace_words_variant(id, "cleaned", &r.words)
        .await
    {
        tracing::warn!(id = %id.as_str(), error = %e, "cleaned re-flow: failed to store words");
    }
    if let Err(e) = state
        .catalog
        .replace_segments_variant(id, "cleaned", &r.segments)
        .await
    {
        tracing::warn!(id = %id.as_str(), error = %e, "cleaned re-flow: failed to store segments");
    }
}

/// Run the recipe's title enrichment (the only enrichment that lands before the
/// transcript-commit's downstream events — same position as the legacy title
/// call). Membership is the gate (a `title` step present == the old
/// `title.enabled`), so this isn't re-gated here; the title step emits no status /
/// PipelineStageChanged. The LLM provider/model/prompt come from the resolved
/// `title` Playbook entry. Folds a real failure into `step_failure`.
async fn run_title_steps(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    steps: &[ResolvedStep],
    transcript: &str,
    raw_transcript: &str,
    step_failure: &mut Option<(RecordingStatus, String)>,
) {
    for step in steps {
        let ResolvedStep::Title { llm_cfg, prompt } = step else {
            continue;
        };
        if let Some(msg) = run_title_step(
            state,
            cfg,
            id,
            transcript,
            raw_transcript,
            llm_cfg.clone(),
            prompt,
        )
        .await
        {
            step_failure.get_or_insert((RecordingStatus::TitleFailed, msg));
        }
    }
}

/// Run the recipe's summary + tags enrichments (the after-commit, after-hooks
/// enrichments), in recipe order. Membership is the gate — a `summary`/`tags` step
/// is present iff the migration found the legacy flag on, so the executor doesn't
/// re-check `summary.auto` / `auto_tag.auto`. Because membership means "this step
/// runs", the `Summarizing`/`Tagging` status is written exactly when the step runs
/// (so a disabled step — one absent from the recipe — never flashes in the UI).
/// The LLM provider/model/prompt for each step come from its resolved Playbook
/// entry. Unsupported `custom:` targets are a no-op + warn. Folds real failures
/// into `step_failure`.
async fn run_enrichment_steps(
    state: &AppState,
    cfg: &Config,
    id: &RecordingId,
    steps: &[ResolvedStep],
    transcript: &str,
    step_failure: &mut Option<(RecordingStatus, String)>,
) {
    for step in steps {
        match step {
            ResolvedStep::Summary { llm_cfg, prompt } => {
                let _ = state
                    .catalog
                    .update_status(id, RecordingStatus::Summarizing)
                    .await;
                if let Some(msg) =
                    run_summary_step(state, id, transcript, llm_cfg.clone(), prompt).await
                {
                    step_failure.get_or_insert((RecordingStatus::SummarizeFailed, msg));
                }
            }
            ResolvedStep::Tags { llm_cfg, prompt } => {
                let _ = state
                    .catalog
                    .update_status(id, RecordingStatus::Tagging)
                    .await;
                if let Some(msg) =
                    run_tags_step(state, cfg, id, transcript, llm_cfg.clone(), prompt).await
                {
                    step_failure.get_or_insert((RecordingStatus::TagFailed, msg));
                }
            }
            ResolvedStep::UnsupportedEnrichment { target } => {
                tracing::warn!(
                    target = %target,
                    "recipe enrichment target has no backing store yet; skipping"
                );
            }
            // Transform / FillerRemoval / Title are handled in their own phases
            // (before the commit), and Hook steps in run_hook_steps; ignore them
            // here.
            ResolvedStep::Transform { .. }
            | ResolvedStep::FillerRemoval { .. }
            | ResolvedStep::Title { .. }
            | ResolvedStep::Hook { .. } => {}
        }
    }
}

/// What the recipe's Hook steps did, folded into the recording's hook provenance
/// (`hook_exit_code` etc., read by the detail-pane Pipeline popover) so it shows
/// Playbook hooks instead of the retired `[hook]` commands.
#[derive(Default)]
struct HookOutcome {
    /// Whether at least one Hook step actually ran (a command or a webhook).
    ran: bool,
    /// A label for the last hook that ran (its command, or `webhook: <url>`).
    last_label: String,
    /// The worst non-zero exit code among command hooks (0 when all succeeded).
    exit_code: i32,
    /// Summed wall-clock of the command hooks.
    total_ms: i64,
}

/// Run the recipe's Hook steps (Playbook entries of `kind: Hook`): for each, gate
/// on its keyword trigger, then run its shell command (`HookRunner`) and/or POST
/// its webhook (`WebhookClient`, under the global `[webhook]` policy). Non-fatal by
/// default — a failure is logged and folded into `step_failure` (surfaced like a
/// failed cleanup/tag step, leaving the transcript intact). A hook marked
/// `required` short-circuits with `Err`, which the caller turns into a failed
/// recording (mirroring the legacy always-on command path).
async fn run_hook_steps(
    state: &AppState,
    cfg: &Config,
    steps: &[ResolvedStep],
    payload: &HookPayload,
    step_failure: &mut Option<(RecordingStatus, String)>,
) -> Result<HookOutcome> {
    let mut out = HookOutcome::default();
    for step in steps {
        let ResolvedStep::Hook { hook } = step else {
            continue;
        };
        if !hook.should_run(&payload.transcript) {
            continue;
        }
        let timeout = Duration::from_secs(hook.timeout_secs);

        // Shell command half. Expand the Phoneme path tokens (%APPDATA%, ~/) the
        // same way the legacy [hook] path does via cfg.expanded().
        let cmd = hook.command.trim();
        if !cmd.is_empty() {
            out.ran = true;
            out.last_label = cmd.to_string();
            let runner = HookRunner::new(phoneme_core::config::expand_cmd(cmd), timeout);
            match runner.run(payload).await {
                Ok(result) => {
                    out.total_ms += result.duration_ms;
                    if result.exit_code != 0 {
                        if hook.required {
                            return Err(phoneme_core::error::Error::HookFailed {
                                code: result.exit_code,
                                stderr_tail: String::new(),
                            });
                        }
                        out.exit_code = result.exit_code;
                        tracing::warn!(command = %cmd, exit_code = result.exit_code, "playbook hook exited non-zero");
                        step_failure.get_or_insert((
                            RecordingStatus::HookFailed,
                            format!("hook exited {}", result.exit_code),
                        ));
                    } else {
                        tracing::info!(command = %cmd, "playbook hook ran");
                    }
                }
                Err(e) => {
                    if hook.required {
                        return Err(e);
                    }
                    if out.exit_code == 0 {
                        out.exit_code = 1;
                    }
                    tracing::warn!(command = %cmd, error = %e, "playbook hook failed to run");
                    step_failure.get_or_insert((RecordingStatus::HookFailed, e.to_string()));
                }
            }
        }

        // Webhook half — same payload; the global [webhook] policy / SSRF guard.
        let url = hook.webhook_url.trim();
        if !url.is_empty() {
            out.ran = true;
            out.last_label = format!("webhook: {url}");
            if let Err(e) = state
                .webhook
                .post(url, timeout, payload, &cfg.webhook)
                .await
            {
                if hook.required {
                    return Err(e);
                }
                if out.exit_code == 0 {
                    out.exit_code = 1;
                }
                tracing::warn!(url = %url, error = %e, "playbook webhook failed");
                step_failure.get_or_insert((RecordingStatus::HookFailed, e.to_string()));
            }
        }
    }
    Ok(out)
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
    // The worker has now claimed this item — flip Queued → Transcribing. Enqueue
    // sites set Queued, so a recording waiting in the queue reads as "queued"
    // rather than mislabeled "transcribing". Best-effort: a status write failing
    // must not abort the run.
    let _ = state
        .catalog
        .update_status(&id, RecordingStatus::Transcribing)
        .await;

    // Transcribe — reuse the process-wide client (AppState) so the HTTP
    // connection pool to the local whisper-server stays warm across items.
    let cfg_guard = state.config.load();
    // Apply this job's one-time Re-run overrides (hooks toggle / post-processing
    // opt-out / the Re-run "All" cleanup+summary+title values) onto a per-job
    // clone of the config — they must never touch the process-global config, where
    // a concurrent ReloadConfig could clobber them or they could leak their
    // forced-on pipeline onto another queued job. Claimed (removed) here so a
    // daemon restart drops them. No override → run with the loaded config as-is
    // (no clone).
    let cfg_owned;
    let cfg: &phoneme_core::Config = match state
        .pending_all_overrides
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id)
    {
        Some(rerun) => {
            cfg_owned = apply_rerun_overrides(cfg_guard.as_ref().clone(), rerun);
            &cfg_owned
        }
        None => cfg_guard.as_ref(),
    };
    let audio_path = std::path::Path::new(&payload.audio_path).to_path_buf();
    // Filter empty string to None — frontend sends "" for "auto-detect"
    let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());

    // Hold the whisper-server permit for the whole final transcription so the
    // streaming preview backs off and can't starve it (it used to time out).
    // Acquiring waits for any in-flight preview tick to finish. A model-override
    // swap (below) happens under this permit too, so the preview and any other
    // final transcription never run while the bundled server is mid-restart for a
    // one-job model override.
    let _whisper_permit = state.whisper_sem.acquire().await;

    // Apply this recording's one-time model override (if any), scoped to this job.
    // `override_guard` restores the configured model on every exit path (success,
    // error, cancel) via Drop, so the override can't leak onto a later job or
    // persist in config. `whisper_cfg` is the per-job transcription config the
    // provider is built from.
    // Recover from a poisoned mutex (take the inner map) rather than panicking —
    // this runs on every pipeline job, so an `.unwrap()` here would turn one
    // unrelated panic-while-locked into a daemon-wide crash loop.
    let requested_override = state
        .pending_overrides
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    // Claim this recording's custom-hotkey recipe override (if any) at the same
    // early point as the model/all-overrides removals — before transcription — so
    // a transcribe failure / cancel can't leave a stale entry keyed by a dead id
    // (the `resolve_recipe` call is much later, past the failure paths). Empty /
    // a deleted id degrades to the `default` recipe inside `resolve_recipe`.
    let requested_recipe_id = state
        .pending_recipe
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    // Claim this recording's focused-app override (if any) at the same early point
    // — before transcription — so a transcribe failure / cancel can't leave a
    // stale entry keyed by a dead id (the end-of-run typing is much later, past
    // the failure paths). Only ever populated for a non-fast-lane in-place
    // dictation; `None` degrades to the global `type_mode` in `resolve_type_mode`.
    let focused_app = state
        .pending_focused_app
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    // A non-empty bound recipe means this recording was routed off the dictation
    // fast lane into the full pipeline because of its recipe (see the recorder's
    // `wants_fast_lane`). It governs the end-of-run typing decision: such an
    // in-place recording gets its single insertion here, never a type-first pass.
    let recipe_routed = requested_recipe_id
        .as_deref()
        .is_some_and(|r| !r.trim().is_empty());
    let (whisper_cfg, override_guard) =
        apply_model_override(state, &cfg.whisper, requested_override).await;
    // Dial the port the bundled server is actually listening on: the supervisor
    // falls back to a free port when the configured one is held by another app,
    // and publishes the live value in `whisper_ports`.
    let whisper_cfg = state.whisper_ports.apply(cfg, &whisper_cfg);
    let provider = state.transcription.provider(&whisper_cfg, &cfg.diarization);

    // Track-aware Meeting Mode: read this recording's track + meeting link before
    // transcribing (a narrow two-column read, not the full row + join). A meeting's
    // mic track is a single voice — the user's — so we label it as one fixed
    // speaker "You" instead of diarizing it: that halves a meeting's diarizer work
    // (only the system track runs speakrs) and kills the spurious multi-speaker
    // labels a single-mic track otherwise produces. The `FixedSpeaker` hint
    // applies only when the row is genuinely a meeting mic track (`meeting_id` set
    // and `track == "mic"`), so a stray `track` value on a non-meeting row can't
    // change behavior; everything else diarizes as before. Best-effort: a read
    // failure falls back to `Diarize`.
    let (track, meeting_id) = state
        .catalog
        .track_and_meeting(&id)
        .await
        .unwrap_or((None, None));
    let is_meeting_mic = meeting_id.is_some() && track.as_deref() == Some("mic");
    let diar_track = if is_meeting_mic {
        DiarizationTrack::FixedSpeaker("You")
    } else if meeting_id.is_none() && cfg.diarization.solo_one_speaker {
        // A solo (non-meeting) recording and the user opted in to "treat single
        // recordings as one speaker": skip diarization so one voice is never split
        // into phantom `[Speaker N]` turns. Meeting tracks are excluded (a
        // meeting's system track still diarizes its multiple participants).
        DiarizationTrack::Plain
    } else {
        DiarizationTrack::Diarize
    };

    // Report transcription to the unified AI-activity ("brain") popout via the
    // Transcribing stage of LlmActivity: a start event naming the model/file, then
    // a done event with timing + size once it finishes. This lets the same popout
    // that shows cleanup/summary also surface what the STT engine is up to.
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
        res = provider.transcribe_with_segments(&audio_path, language.as_deref(), diar_track) => match res {
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
                // A transient error (server down / restarting, request timed out)
                // must not bury the item in failed/ — the queue worker requeues it
                // and retries with backoff, so a whisper-server blip never costs a
                // recording. Only permanent errors (bad audio, 4xx, decode
                // failures) take the failed path.
                if !transient {
                    state
                        .catalog
                        .update_status(&id, RecordingStatus::TranscribeFailed)
                        .await?;
                    // Persist the reason on the row so it survives a restart —
                    // best-effort, since the status + quarantine below are what
                    // actually fail the recording. Same kind label the inbox
                    // quarantine uses.
                    if let Err(err) = state
                        .catalog
                        .update_error(&id, "whisper_error", &e.to_string())
                        .await
                    {
                        tracing::warn!(error = %err, "failed to persist transcribe error reason");
                    }
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

    // The segment + word timelines are machine truth (they describe the raw
    // whisper output, not the LLM-cleaned text), so split them off here — the rest
    // of the pipeline only transforms the text.
    let phoneme_core::transcription::Transcription {
        text: transcript,
        segments,
        words,
        // Did the local fixed-speaker labelling actually run (mic track with real
        // segments on the OpenAI-compatible path)? Carried to the "You"
        // speaker-name write below so a cloud STT backend (which ignores the hint)
        // or a silent/segment-less mic track never gets an orphan row.
        fixed_speaker_applied,
        // Per-speaker centroid voiceprints (local diarization only; empty for
        // cloud/plain paths), persisted below for cross-recording recognition.
        speaker_voiceprints,
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

    // Restore the configured whisper model (if this job overrode it) before
    // releasing the permit, so the bundled server is swapped back while the preview
    // is still gated — the resumed preview then runs the configured model, not this
    // job's one-time override. Dropping the guard pings the supervisor; for
    // non-override jobs it's a no-op. (Both early-return paths above — cancel and
    // transcribe error — drop this guard implicitly, so the model is always
    // restored.)
    drop(override_guard);

    // Release the whisper-server permit now that transcription is done — LLM
    // post-processing and hooks below don't touch the server, so the preview
    // can resume immediately.
    drop(_whisper_permit);

    // Preserve the raw Whisper output as the "original" transcript regardless
    // of whether LLM post-processing rewrites the live version. Users can
    // always restore to this via "View original transcript" in the detail pane.
    let raw_transcript = transcript.clone();

    // Resolve the recording's recipe into an ordered list of dispatchable steps.
    // The migration encoded the legacy enable flags into recipe membership, so a
    // step runs iff it's present here; the per-step on/off semantics (a provider
    // must resolve for cleanup, the legacy flag still self-gates inside each
    // enrichment helper) are preserved by the dispatchers below. A custom-hotkey
    // recording carries its binding's `recipe_id`; every normal recording carries
    // `None` and resolves the `default` recipe. A dangling step id, an empty
    // recipe, or a deleted/missing recipe id degrades to a bare transcribe-only
    // run (falling back to `default` inside `resolve_recipe`), never a panic.
    let recipe_id = requested_recipe_id.as_deref().unwrap_or(DEFAULT_RECIPE_ID);
    let recipe = resolve_recipe(cfg, recipe_id);

    // The earliest optional step (cleanup → title → summary → tag) that actually
    // failed (a user-skip doesn't count) becomes the recording's terminal status
    // instead of `Done`, so a failed enrichment is filterable/searchable — like
    // `HookFailed`, the transcript is still intact and usable. `get_or_insert`
    // keeps the first/most-upstream failure (a single status can't hold several);
    // the paired message is persisted as the row's error so the failed panel +
    // logs show why, surviving a restart (see `finalize_step_status`).
    let mut step_failure: Option<(RecordingStatus, String)> = None;

    // PHASE 1 — Transform steps (cleanup): in-memory text rewrites that must
    // happen before the transcript-commit writes below. Non-fatal: on any failure
    // the raw transcript is kept. Cancellation inside a Transform has already
    // finalized the recording, so we return `Ok(())` here.
    let (transcript, cleanup_model, step_versions) = match run_transform_steps(
        state,
        &id,
        &cancel,
        &recipe,
        transcript,
        &mut step_failure,
    )
    .await
    {
        Ok(out) => out,
        Err(Canceled) => return Ok(()),
    };

    payload.transcript = transcript.clone();
    // Record the model that actually ran, from the per-job whisper config so a
    // one-time model override is reflected. The local bundled backend talks to
    // whisper.cpp over HTTP and only knows its model as a file on disk, so its id
    // is the `model_path` stem; the cloud/custom backends send a model id in the
    // request, so theirs is the requested `whisper.model` (falling back to the
    // path stem only when no model was set, so a misconfigured cloud backend
    // records the path stem rather than an empty string).
    payload.model = {
        use phoneme_core::config::TranscriptionBackend as TB;
        let path_stem = || {
            std::path::Path::new(&whisper_cfg.model_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        };
        match whisper_cfg.provider {
            TB::Local => path_stem(),
            _ => {
                let requested = whisper_cfg.model.trim();
                if requested.is_empty() {
                    path_stem()
                } else {
                    requested.to_string()
                }
            }
        }
    };

    // `transcript` = LLM-processed (or raw if LLM is disabled/failed).
    // `raw_transcript` = raw Whisper output, always preserved as the original.
    state
        .catalog
        .update_transcript(&id, &transcript, &raw_transcript, &payload.model)
        .await?;

    // Record the compounding chain: raw ASR as idx 0, then each Transform step's
    // output. When at least one Transform ran we store the full chain; when none did
    // (a plain transcribe, or a retranscribe with cleanup opted out) we write an
    // empty set to clear any prior chain. Always rewriting it keeps the stored chain
    // in step with the live transcript this run just wrote — otherwise a retranscribe
    // could leave a stale chain behind and Revert could resurrect a previous run's
    // text. Best-effort: a failure costs only the chain view.
    let versions = if step_versions.is_empty() {
        Vec::new()
    } else {
        let mut versions = Vec::with_capacity(step_versions.len() + 1);
        versions.push(phoneme_core::catalog::TranscriptVersion {
            idx: 0,
            step_id: None,
            label: Some("Original (raw)".to_string()),
            model: None,
            text: raw_transcript.clone(),
        });
        versions.extend(step_versions);
        versions
    };
    if let Err(e) = state
        .catalog
        .replace_transcript_versions(&id, &versions)
        .await
    {
        tracing::warn!(id = %id.as_str(), "failed to persist transcript versions: {e}");
    }

    // Persist the provider's segment timeline (replacing any previous one —
    // a retranscribe describes a new machine output). Best-effort: a failure
    // here costs the timeline views, not the recording.
    if let Err(e) = state.catalog.replace_segments(&id, &segments).await {
        tracing::warn!(id = %id.as_str(), "failed to persist transcript segments: {e}");
    }

    // Persist the finer per-word timeline alongside the segments (same
    // machine-truth, replace-on-(re)transcribe semantics). Empty for providers
    // with no per-word data. Best-effort: a failure costs only the word-level
    // views (word seek, confidence highlighting), not the recording.
    if let Err(e) = state.catalog.replace_words(&id, &words).await {
        tracing::warn!(id = %id.as_str(), "failed to persist transcript words: {e}");
    }

    // When a Transform changed the transcript, re-derive a "cleaned" timing
    // variant (the raw words realigned to the cleaned text) so Timeline/Synced can
    // match the panel instead of showing the raw ASR. The raw timing just stored
    // above is untouched; this writes only the *_clean tables.
    if transcript != raw_transcript {
        reflow_cleaned_timing(state, &id, &transcript).await;
    }

    // Persist each speaker's centroid voiceprint (local diarization only; empty on
    // cloud/plain paths) keyed by the same label as the transcript, so naming a
    // speaker can enroll it into the cross-recording library and later recordings
    // can be matched against it. A re-transcribe refreshes the sample without
    // un-enrolling. Best-effort: a failure costs only recognition.
    //
    // Each capture carries that speaker's total speaking duration (sum of their
    // segment spans, in ms) as a duration-weight: a long, clean sample outvotes a
    // brief one when the named voice's centroid is recomputed. The segment
    // `speaker` field is the decimal label that matches `vp.label` (`[Speaker N]`);
    // segments with no/other label contribute 0. A speaker with no matching segment
    // falls back to 0, which the weighted mean treats as equal weight.
    let mut speaking_ms: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for seg in &segments {
        if let Some(label) = &seg.speaker {
            *speaking_ms.entry(label.clone()).or_insert(0) += (seg.end_ms - seg.start_ms).max(0);
        }
    }
    for vp in &speaker_voiceprints {
        let duration_ms = speaking_ms.get(&vp.label.to_string()).copied().unwrap_or(0);
        if let Err(e) = state
            .catalog
            .save_speaker_voiceprint(id.as_str(), vp.label as i64, &vp.centroid, duration_ms)
            .await
        {
            tracing::warn!(id = %id.as_str(), label = vp.label, "failed to persist voiceprint: {e}");
        }
    }

    // Track-aware Meeting Mode: seed label 1 → "You" only when the mic track was
    // actually labelled `[Speaker 1]` in place of a diarizer pass
    // (`fixed_speaker_applied` — the local OpenAI-compatible path with real
    // segments). Gating on the result flag, not just `is_meeting_mic`, avoids two
    // bugs: a cloud STT backend ignores the fixed-speaker hint (no `[Speaker 1]` to
    // attach to → an orphan/mislabelled row), and an empty/segment-less/silent mic
    // track produces no `[Speaker 1]` either.
    //
    // The write is `set_speaker_name_if_absent`, not the upsert: it seeds the
    // friendly default on the first transcribe but never clobbers an existing row,
    // so a user rename (e.g. Speaker 1 → "Alice") survives a Retranscribe / Re-run
    // / crash-recovery requeue that re-enters this branch. Like any speaker name,
    // it stays user-renamable and never rewrites the canonical `[Speaker 1]`
    // markers. Best-effort: a failure costs only the friendly label, not the
    // recording.
    if is_meeting_mic && fixed_speaker_applied {
        if let Err(e) = state
            .catalog
            .set_speaker_name_if_absent(&id, 1, "You")
            .await
        {
            tracing::warn!(id = %id.as_str(), "failed to seed meeting mic speaker 'You': {e}");
        }
    }

    // Record which post-processing model was used and whether diarization was
    // actually applied. Diarization is only meaningfully "applied" when it produced
    // multi-speaker labels — both the local diarizer (`assign_speakers`) and the
    // cloud providers emit a "[Speaker N]: " prefix and fall back to plain,
    // unlabeled text when diarization is off, fails, or finds ≤1 speaker. So a
    // recording is diarized iff its raw (pre-cleanup) transcript carries those
    // labels, not merely because the setting is enabled. We check the raw
    // transcript because LLM cleanup may strip the labels from the live version.
    let diarized = raw_transcript.contains("[Speaker ");
    // Name a cloud diarizer's model for the provenance line; the local speakrs
    // diarizer (and "off") has none, so the line shows a plain "diarized".
    let diarization_model = if diarized {
        cfg.diarization.model_label()
    } else {
        None
    };
    state
        .catalog
        .update_processing_meta(
            &id,
            cleanup_model.as_deref(),
            diarized,
            diarization_model.as_deref(),
        )
        .await?;

    // PHASE 2a — title enrichment, right after the transcript settles (cleanup
    // included) and before TranscriptionDone fires, so the refreshed list shows the
    // title together with the new text. A retranscribe re-runs this and refreshes
    // auto titles; user-set titles are protected inside. Runs only when the recipe
    // contains a `title` step (membership is the gate — the migration folded
    // `cfg.title.enabled` into membership); the title step reads its migrated
    // Playbook entry for provider/model/prompt.
    run_title_steps(
        state,
        cfg,
        &id,
        &recipe,
        &transcript,
        &raw_transcript,
        &mut step_failure,
    )
    .await;

    let recording = state.catalog.get(&id).await?;
    if let Some(rec) = recording {
        if pipeline_should_type(&cfg.in_place, rec.in_place, recipe_routed, &transcript) {
            // Resolve how the text lands for the focused app captured at start: a
            // per-app override ("type"/"paste"/"off") wins over the global
            // `type_mode`; an unlisted (or undetectable) app falls back to the
            // global mode. This mirrors the dictation fast lane — without it a
            // recipe-routed / full-pipeline in-place recording would type with the
            // bare global `type_mode` and ignore a per-app "off" (and any per-app
            // paste/type) the fast lane honors. `focused_app` is the side-channel
            // value claimed early above; `None` (no stash, or a daemon-restart
            // drop) degrades to the global mode.
            let type_mode = cfg.in_place.resolve_type_mode(focused_app.as_deref());
            // Did streaming-type type live this dictation? Reset at every record
            // start, so a non-empty value means this recording streamed —
            // independent of the current config. Taken (cleared) here regardless.
            let streamed = std::mem::take(&mut *state.stream_typed.lock().await);
            if !streamed.is_empty() {
                // This recording streamed text live, so there's already live-typed
                // text at the cursor — branch on that before the resolved
                // `type_mode`. If a mid-recording config change flipped the app to
                // paste/off, typing the transcript would land it on top of the
                // orphaned live text, so always reconcile the streamed text instead:
                // type-mode patches it to the pipeline result, paste/off first
                // backspaces it away (then paste re-delivers via the clipboard).
                tracing::info!(id = %id.as_str(), "in-place dictation: reconciling streamed text to pipeline result");
                let target = if type_mode == "type" {
                    transcript.as_str()
                } else {
                    ""
                };
                let (backspaces, insert) =
                    phoneme_core::dictation::reconcile_edit(&streamed, target);
                // Safety guard: only backspace while the same window the text
                // streamed into still owns the caret. If focus moved between live
                // streaming and this end-of-run reconcile, those backspaces would
                // chew through unrelated content in the wrong window. On a mismatch
                // skip the destructive part; for "type" still append the divergent
                // insert at the current caret.
                let focus_lost = backspaces > 0
                    && !crate::in_place::foreground_still_matches(focused_app.as_deref());
                if focus_lost {
                    tracing::warn!(
                        id = %id.as_str(),
                        "in-place dictation: foreground changed since streaming; skipping {backspaces} backspaces to avoid destroying other content"
                    );
                    if type_mode == "type" && !insert.is_empty() {
                        if let Err(e) = crate::in_place::type_at_cursor(&insert, "type").await {
                            tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to type appended insert after focus change");
                        }
                    }
                    state.events.emit(DaemonEvent::TranscriptionFailed {
                        id: id.clone(),
                        error: "dictation finished, but focus moved away from where you were typing — the live text wasn't corrected to the final transcript (left as-is to avoid deleting other content). The full transcript is in the library.".to_string(),
                    });
                } else if let Err(e) =
                    crate::in_place::reconcile_at_cursor(backspaces, &insert).await
                {
                    tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to reconcile streamed text");
                }
                // A paste-mode flip still owes the user the final text via the
                // clipboard once the orphaned live text is cleared. Skip on focus
                // loss (orphan not cleared) or an empty transcript.
                if type_mode == "paste" && !focus_lost && !transcript.trim().is_empty() {
                    if let Err(e) = crate::in_place::type_at_cursor(&transcript, "paste").await {
                        tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to paste after streamed reconcile");
                    }
                }
            } else if type_mode == "off" {
                // The user asked dictation not to auto-deliver for this app — the
                // transcript still rides the pipeline into the library, it just
                // doesn't land at the cursor. Same skip the fast lane does.
                tracing::info!(
                    id = %id.as_str(),
                    "in-place dictation: per-app override is \"off\" for the focused app; not typing"
                );
            } else {
                tracing::info!(id = %id.as_str(), "in-place dictation: typing transcript at cursor");
                // The full-pipeline dictation path: either [in_place].full_pipeline
                // without `type_first`, or a recipe-bearing in-place binding (always
                // full-pipeline). The text lands only after every configured step —
                // for a recipe binding that's the recipe's result, the single
                // insertion. (With plain `type_first` the recorder's type-only pass
                // already typed it, so `pipeline_should_type` keeps this run from
                // landing it twice; for a recipe binding the recorder skips
                // type-first, so this run owns the sole insertion.) The insertion
                // itself (typing vs clipboard-paste, input-simulator failure modes)
                // lives in `in_place::type_at_cursor`, shared with the fast lane.
                // Best-effort — a failure is logged loudly but never fails the
                // recording.
                if let Err(e) = crate::in_place::type_at_cursor(&transcript, type_mode).await {
                    tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to insert transcript");
                }
            }
        }
    }

    // Clone the Arc out and drop the read-lock immediately — the inference now
    // runs under spawn_blocking inside embed_and_store, so holding the guard
    // across it would needlessly serialize a config reload / ReembedAll.
    let embedder = state.embedder.read().await.as_ref().cloned();
    if let Some(embedder) = embedder {
        embed_and_store(embedder, &state.catalog, &id, &transcript).await;
    }

    // Hooks are optional. When `run_on_transcribe` is off, finalize the recording
    // right after transcription without firing hooks or the webhook; the user can
    // run them on demand later via "Re-fire hook". This is what lets a
    // re-transcription update the text without re-triggering side effects (e.g.
    // re-appending to an Obsidian daily note).
    if !cfg.hook.run_on_transcribe {
        // PHASE 2b — summary + tags enrichments (the recipe's remaining Enrichment
        // steps), in recipe order. They run after post-processing so they see the
        // text the user actually sees; tag suggestions are approve-to-apply
        // (side-effect free). Each self-gates on its legacy flag and writes its own
        // status only when it will actually run, so a disabled step never flashes in
        // the UI.
        run_enrichment_steps(state, cfg, &id, &recipe, &transcript, &mut step_failure).await;
        // A failed optional step (cleanup/title/summary/tag) becomes the terminal
        // status — like hook_failed, the transcript is intact and usable, but the
        // failure is filterable, with the reason persisted. No failure → Done.
        finalize_step_status(state, &id, step_failure).await?;
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
    // Note: `TranscriptionDone` is not emitted here. The UI re-fetches the
    // recording on that event, and the hook provenance (hook_command /
    // hook_exit_code / hook_duration_ms) isn't written until `update_hook_result`
    // runs after all hooks finish — so emitting now would hand the UI a recording
    // with null hook fields, and a client that doesn't also listen for `HookDone`
    // would see that stale state forever. The transcript is already persisted, so
    // nothing is lost by deferring the event to after the provenance write below.

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

    // Single-fire invariant: a hook fires exactly once per transcribe, never twice
    // — even though the legacy [hook] loops below and the recipe Hook executor
    // (`run_hook_steps`) both live on this path. `migrate_hooks` (run by
    // `load_config` before any pipeline run) moves the legacy
    // commands/keyword_rules/webhook into recipe Hook entries and clears the legacy
    // fields, so post-migration these loops iterate an empty list and the migrated
    // entries fire only via `run_hook_steps`. Pre-migration the default recipe has
    // no Hook step, so only these loops fire. The two paths never both fire the same
    // hook. Kept (not deleted) so a config that somehow reaches the pipeline
    // un-migrated still runs its legacy hooks. Locked by
    // `configured_hook_fires_exactly_once_per_transcribe`.
    //
    // Expand env vars / `~` in the legacy [hook].commands. If expansion fails we
    // fall back to the unexpanded config, but log it first: a silent fallback
    // leaves literal `%APPDATA%` / `~/` tokens in the command, so every hook then
    // fails with a confusing path error and no clue why.
    let expanded_cfg = cfg.expanded().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "config expansion failed; hook env vars may not expand correctly");
        cfg.clone()
    });

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
                // Persist the reason on the row so it survives a restart (see the
                // transcribe-failed path). Best-effort; the quarantine below is
                // what actually fails the recording.
                if let Err(err) = state
                    .catalog
                    .update_error(&id, "hook_failed", &e.to_string())
                    .await
                {
                    tracing::warn!(error = %err, "failed to persist hook error reason");
                }
                state
                    .inbox
                    .finish_failed(&id, "hook_failed", &e.to_string())
                    .await?;
                // The transcript is complete and persisted; the provenance-deferral
                // rationale (waiting for update_hook_result) doesn't apply on the
                // failure path — that write is never reached — so signal the
                // transcript to the UI before failing the recording.
                state.events.emit(DaemonEvent::TranscriptionDone {
                    id: id.clone(),
                    transcript: transcript.clone(),
                });
                state.events.emit(DaemonEvent::HookFailed {
                    id,
                    error: e.to_string(),
                });
                return Err(e);
            }
        }
    }

    // Conditional keyword-triggered hooks: run each rule whose pattern matches the
    // (post-processed) transcript. These are supplementary — a failure is logged
    // but doesn't fail the recording, since the always-on commands above already
    // succeeded.
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

    // PHASE 2a (recipe) — Playbook Hook steps: shell/webhook side-effects that live
    // in the recipe. Additive alongside the legacy [hook] config above (the
    // migration folds [hook] into Hook entries and removes the legacy path). A
    // `required` hook failing quarantines the recording like a failed command.
    let hook_outcome = match run_hook_steps(state, cfg, &recipe, &payload, &mut step_failure).await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            state
                .catalog
                .update_status(&id, RecordingStatus::HookFailed)
                .await?;
            if let Err(err) = state
                .catalog
                .update_error(&id, "hook_failed", &e.to_string())
                .await
            {
                tracing::warn!(error = %err, "failed to persist hook error reason");
            }
            state
                .inbox
                .finish_failed(&id, "hook_failed", &e.to_string())
                .await?;
            // The transcript is complete and persisted; the provenance-deferral
            // rationale (waiting for update_hook_result) doesn't apply on the
            // failure path — that write is never reached — so signal the
            // transcript to the UI before failing the recording.
            state.events.emit(DaemonEvent::TranscriptionDone {
                id: id.clone(),
                transcript: transcript.clone(),
            });
            state.events.emit(DaemonEvent::HookFailed {
                id: id.clone(),
                error: e.to_string(),
            });
            return Err(e);
        }
    };
    // Fold the recipe Hook steps into the per-recording hook provenance the
    // Pipeline popover reads (hook_exit_code) — the legacy command/keyword vars
    // above are empty post-migration, so this is what populates it.
    if hook_outcome.ran {
        if last_cmd.is_empty() {
            last_cmd = hook_outcome.last_label;
        }
        if final_exit_code == 0 {
            final_exit_code = hook_outcome.exit_code;
        }
        total_duration += hook_outcome.total_ms;
    }

    // PHASE 2b (hooks on) — summary + tags enrichments, after post-processing and
    // hooks so they see the text the user actually sees; tag suggestions are
    // approve-to-apply (side-effect free). Same recipe-driven dispatch as the
    // hooks-off arm, over the (post-processed) payload transcript.
    run_enrichment_steps(
        state,
        cfg,
        &id,
        &recipe,
        &payload.transcript,
        &mut step_failure,
    )
    .await;

    // Record hook provenance only when a hook actually ran (a legacy command or a
    // recipe Hook step) — otherwise leave hook_exit_code null so the Pipeline
    // popover doesn't show a phantom "Hook ✓" on recordings with no Hook steps.
    if !last_cmd.is_empty() {
        state
            .catalog
            .update_hook_result(&id, &last_cmd, final_exit_code, total_duration)
            .await?;
    }
    // Now that the hook provenance is on the row, announce the transcript. A UI
    // re-fetch on this event sees the complete hook state (not the null fields it
    // would have seen had we emitted before the hooks ran). `HookDone` below is
    // the hook-specific signal; this is the transcript-ready one.
    state.events.emit(DaemonEvent::TranscriptionDone {
        id: id.clone(),
        transcript: transcript.clone(),
    });
    // A failed optional step becomes the terminal status (filterable, reason
    // persisted), like hook_failed; otherwise Done.
    finalize_step_status(state, &id, step_failure).await?;
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
                // The [webhook] policy (SSRF guard) is read per-run so a config
                // reload takes effect without restarting the daemon.
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
