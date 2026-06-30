//! IPC request executors split out of the dispatch in `super` (the
//! on-demand re-runs, import/export/edit, and disk re-import). Pure free fns
//! that the `handle_request` match delegates to.
use super::*;

/// Validate + normalize a one-time import recipe id. A `scope = Meeting` recipe is
/// rejected (a single import is not a meeting); `None`/empty is fine (the global
/// default), and an unknown id is left to `resolve_recipe`'s lenient fallback
/// (→ default), matching record/retranscribe. Returns the trimmed id to stash, or
/// a user-facing error message.
pub(super) fn validate_import_recipe(
    cfg: &phoneme_core::Config,
    recipe_id: Option<String>,
) -> Result<Option<String>, String> {
    let rid = recipe_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(ref id) = rid {
        if cfg
            .recipes
            .iter()
            .any(|r| &r.id == id && matches!(r.scope, phoneme_core::config::RecipeScope::Meeting))
        {
            return Err(format!(
                "recipe '{id}' is a meeting template (scope = Meeting); import a single recording with a recording-scope recipe"
            ));
        }
    }
    Ok(rid)
}

/// Stash a custom-hotkey recording's per-job overrides against its freshly minted
/// recording id, so `pipeline::run` resolves the binding's recipe and transcribes
/// with its model. Two ledgers, both already proven by `RetranscribeRecording`:
///
///  - `whisper_model` → `pending_overrides` (the existing per-job model override
///    map): the pipeline applies it via `apply_model_override` for one job, then
///    restores — the same #49-safe path a model-override retranscribe uses.
///  - `recipe_id` → `pending_recipe` (the parallel recipe ledger): the pipeline
///    passes it to `resolve_recipe`, falling back to the `default` recipe when the
///    id is empty or names a deleted recipe.
///
/// Both are written only when non-empty, so a normal (non-custom-hotkey) record,
/// which sends `None`, leaves the recording on the global default recipe and
/// configured model. The maps are ephemeral: a daemon restart between stash and
/// the pipeline claim drops the override and the job runs the default recipe and
/// configured model (the documented `pending_overrides` contract). A leftover
/// entry can't leak onto another recording (each `RecordingId` is unique), and the
/// entries are claimed-and-removed on every terminal path: `pipeline::run` removes
/// both early — alongside the model/all-overrides removals, before transcription —
/// so a permanently-failed recording leaves nothing, and `DaemonRecorder::cancel`
/// removes both in its single-recording arm so a recording canceled mid-capture
/// (which never reaches `pipeline::run`) leaves nothing either.
pub(super) fn stash_hotkey_overrides(
    state: &AppState,
    id: &phoneme_core::RecordingId,
    recipe_id: Option<String>,
    whisper_model: Option<String>,
) {
    if let Some(model) = whisper_model {
        let model = model.trim();
        if !model.is_empty() {
            state
                .pending_overrides
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.clone(), model.to_string());
        }
    }
    if let Some(recipe) = recipe_id {
        let recipe = recipe.trim();
        if !recipe.is_empty() {
            state
                .pending_recipe
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.clone(), recipe.to_string());
        }
    }
}

/// Hard cap on the on-disk size of an importable file. The Tauri file dialog is
/// the intended sole producer, but `ImportRecording` accepts an arbitrary client
/// path, so this bounds a bypass that could otherwise feed the decoder a
/// pathologically large file and exhaust memory (the decoder buffers the whole
/// file into a single `Vec<f32>`; see `phoneme-audio::decode`). 2 GiB is far
/// beyond any realistic voice note while still leaving the decode duration cap
/// (in `phoneme-audio`) as the real memory bound.
pub(super) const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Returns `true` if an on-disk file of `len` bytes exceeds the import size cap.
/// Factored out so the bound is unit-testable without a multi-GiB fixture file.
pub(super) fn exceeds_import_size_cap(len: u64) -> bool {
    len > MAX_IMPORT_BYTES
}

/// Whether `requested` matches a configured hook command (compared trimmed).
///
/// The IPC `RefireHook` request lets a caller pass a command to run; without this
/// check any process reaching the pipe could run an arbitrary command via the
/// daemon. Restricting to the already-configured hooks turns it into "re-run one
/// of my hooks" instead of an open exec channel. (audit S-C2)
pub(super) fn hook_command_allowed(requested: &str, configured: &[String]) -> bool {
    let requested = requested.trim();
    !requested.is_empty() && configured.iter().any(|c| c.trim() == requested)
}

/// Returns `true` if `audio_path` is a normal path located under `audio_dir`.
///
/// The path comes from our own catalog, so this is defense in depth: we reject any
/// `..` component (which could climb out of the audio directory) and require the
/// rest to be prefixed by `audio_dir` component-wise. Kept purely lexical so it's
/// unit-testable without touching the filesystem and never deletes the wrong file
/// just because canonicalization of an already-removed file failed.
///
/// Lexical-only means this does NOT resolve symlinks: a symlink stored under
/// `audio_dir` passes this check yet would unlink its target. We never write
/// symlinks into the audio dir, but the delete callers add a `symlink_metadata`
/// refusal on top of this guard so that residual case can't escape.
pub(super) fn audio_path_is_ours(audio_path: &str, audio_dir: &std::path::Path) -> bool {
    use std::path::Component;
    let p = std::path::Path::new(audio_path);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    p.starts_with(audio_dir)
}

/// `true` if `path` is itself a symlink (vs a regular file or missing). Used to
/// refuse deleting a symlinked audio entry: the lexical [`audio_path_is_ours`]
/// guard can't see through a link, so a symlink planted under the audio dir would
/// otherwise unlink its target. A missing path / stat error reads as "not a
/// symlink" — the subsequent best-effort remove just no-ops on the missing file.
pub(super) async fn is_symlink(path: &str) -> bool {
    tokio::fs::symlink_metadata(path)
        .await
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Doctor check for orphaned audio: `.wav` files on disk that have no catalog row.
/// They accumulate when recordings are deleted with "keep the audio file", and a
/// `--reimport` would resurrect them, so surface the count rather than let it grow
/// silently and surprise the user later. Reuses the re-import scan and `all_ids`,
/// so it counts exactly what "Re-import from disk" would re-link.
pub(super) async fn orphan_audio_check(state: &AppState) -> phoneme_core::doctor::CheckResult {
    let existing: std::collections::HashSet<phoneme_core::RecordingId> = state
        .catalog
        .all_ids()
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let audio_dir = state.paths.audio_dir.clone();
    let count = tokio::task::spawn_blocking(move || scan_audio_dir(&audio_dir))
        .await
        .map(|cands| {
            cands
                .into_iter()
                .filter(|c| !existing.contains(&c.id))
                .count()
        })
        .unwrap_or(0);
    phoneme_core::doctor::orphan_audio_check_result(count)
}

/// Re-run only the LLM post-processing ("cleanup") step on a recording's
/// already-stored transcript, without re-transcribing the audio.
///
/// Design mirrors `RefireHook`: validate up front on the IPC connection (the
/// recording must exist and have a transcript), then do the slow work — the LLM
/// call, which can take its full timeout — off the connection in a spawned task,
/// reporting progress via the same `DaemonEvent`s the UI already consumes. This
/// keeps the single-connection Tauri bridge responsive.
///
/// Input baseline: the preserved original (raw Whisper) transcript when one
/// exists, falling back to the live transcript for recordings predating that
/// column. Cleaning the original rather than the already-cleaned live text keeps
/// the operation idempotent (re-running with a different model re-cleans the same
/// source instead of compounding edits) and lets us reuse `update_transcript`,
/// which re-asserts the original alongside the new live text. So the original
/// column is preserved by construction.
///
/// An optional `model` overrides the configured cleanup model for this run only;
/// it's never written back to config (unlike `RetranscribeRecording`, which must
/// restart the whisper server). The post-processing provider is built from a
/// cloned config with just the model field swapped.
/// One-time, per-run overrides for [`rerun_cleanup`]. Each field falls back to
/// the configured `[llm_post_process]` value when `None` and is never persisted.
#[derive(Default)]
pub(super) struct CleanupOverrides {
    pub(super) model: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) prompt: Option<String>,
    pub(super) api_url: Option<String>,
    pub(super) api_key: Option<String>,
}

/// One-time, per-run summary/digest connection overrides for [`rerun_summary`]
/// and the two digest re-runs. Each field falls back to the configured summary /
/// `[llm_post_process]` connection when `None` and is never persisted — the
/// connection counterpart of the cleanup overrides (summary's `model`/`prompt`
/// are carried separately). Applied by baking onto a config clone's `[summary]`
/// section so `summary_llm_config`'s `resolve_step` does the inherit-from-cleanup
/// fallback exactly as the auto pipeline does.
#[derive(Default)]
pub(super) struct SummaryProviderOverrides {
    pub(super) provider: Option<String>,
    pub(super) api_url: Option<String>,
    pub(super) api_key: Option<String>,
}

impl SummaryProviderOverrides {
    /// Layer these overrides onto a config clone's `[summary]` section in place.
    /// A non-empty `provider`/key wins; an explicit (even empty) `api_url` is
    /// honored — an empty URL is meaningful ("use the provider default"), matching
    /// the cleanup override rule.
    fn apply_to(self, summary: &mut phoneme_core::config::SummaryConfig) {
        if let Some(p) = self.provider {
            let p = p.trim();
            if !p.is_empty() {
                summary.provider = p.to_string();
            }
        }
        if let Some(u) = self.api_url {
            summary.api_url = u;
        }
        if let Some(k) = self.api_key {
            let k = k.trim();
            if !k.is_empty() {
                summary.set_api_key(k.to_string());
            }
        }
    }

    /// The same override layering onto an already-resolved `LlmPostProcessConfig`
    /// (the migrated `summary` Playbook entry's connection), matching `rerun_cleanup`'s
    /// in-place provider/url/key handling exactly.
    fn apply_to_llm(self, llm_cfg: &mut phoneme_core::config::LlmPostProcessConfig) {
        if let Some(p) = self.provider {
            let p = p.trim();
            if !p.is_empty() {
                llm_cfg.provider = p.to_string();
            }
        }
        if let Some(u) = self.api_url {
            llm_cfg.api_url = u;
        }
        if let Some(k) = self.api_key {
            let k = k.trim();
            if !k.is_empty() {
                llm_cfg.set_api_key(k.to_string());
            }
        }
    }
}

pub(super) async fn rerun_cleanup(
    state: &AppState,
    id: phoneme_core::RecordingId,
    overrides: CleanupOverrides,
) -> Response {
    let recording = match state.catalog.get(&id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return not_found(format!("recording {id} not found"));
        }
        Err(e) => {
            return err_response(&e);
        }
    };

    // Cleanup operates on text — there must be something to clean.
    if recording.transcript.is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcript to run cleanup on".into(),
        });
    }

    // Resolve the base (llm_cfg, prompt) from the migrated `cleanup` Playbook
    // entry so editing it in the Playbook changes what an on-demand Re-run Cleanup
    // does — the Playbook is the source of truth, exactly like the summary/tags
    // re-runs read their migrated entries. `cleanup_entry_config` falls back to the
    // legacy `[llm_post_process]` config and prompt when the entry is gone (a user
    // deleted it), so behavior is never worse than today. The Re-run modal's
    // one-time overrides then layer on top and still win; none of this is persisted
    // (the config is local to the spawned task).
    let CleanupOverrides {
        model,
        provider,
        prompt,
        api_url,
        api_key,
    } = overrides;
    let (base_llm, base_prompt) = crate::pipeline::cleanup_entry_config(&state.config.load());
    // Layer the one-shot model + prompt overrides via the shared helper that
    // `rerun_summary` (and the tests) use, so the layering rule lives in exactly
    // one place. A non-empty override wins; a blank/whitespace one is ignored,
    // since a blank prompt would strip the cleanup instructions.
    let (mut llm_cfg, resolved_prompt) = crate::pipeline::apply_oneshot_overrides(
        base_llm,
        base_prompt,
        model.as_deref(),
        prompt.as_deref(),
    );
    llm_cfg.prompt = resolved_prompt;
    // Provider / endpoint / key overrides are cleanup-only (the summary re-run has
    // no such fields), so they apply directly around the shared base.
    // `cleanup_entry_config` already forced the step enabled — the GUI disables the
    // Re-run Cleanup option when cleanup is off, and the provider check below still
    // blocks a `none`/blank provider.
    if let Some(p) = provider {
        let p = p.trim();
        if !p.is_empty() {
            llm_cfg.provider = p.to_string();
        }
    }
    // An explicit empty URL is meaningful (= "use the provider default"), so
    // honor any provided value rather than only non-empty ones.
    if let Some(u) = api_url {
        llm_cfg.api_url = u;
    }
    if let Some(k) = api_key {
        let k = k.trim();
        if !k.is_empty() {
            llm_cfg.set_api_key(k.to_string());
        }
    }
    // Audit trail: a one-time override can point this run's cleanup at a different
    // provider/endpoint. Log the resolved target (never the API key) so a
    // redirect is visible in the logs rather than silent.
    tracing::info!(
        id = %id,
        provider = %llm_cfg.provider,
        api_url = %llm_cfg.api_url,
        model = %llm_cfg.model,
        "re-run cleanup resolved (one-time overrides applied; API key never logged)"
    );

    // Require post-processing to actually be configured. `provider()` returns
    // None when disabled or the provider is `none`/unrecognized — in that case
    // there is nothing to run, so report it rather than silently no-op'ing.
    if state.llm.provider(&llm_cfg).is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::InvalidConfig,
            message: "LLM post-processing is not enabled (set [llm_post_process] provider)".into(),
        });
    }

    // Choose the cleanup input: prefer the preserved original (raw machine output)
    // so cleanup is idempotent; fall back to the current transcript for older rows
    // that have no original stored.
    let source = match state.catalog.get_original_transcript(&id).await {
        Ok(Some(original)) if !original.is_empty() => original,
        // No original (or empty): fall back to the live transcript. Safe to
        // unwrap — we returned above if it was None.
        _ => recording.transcript.clone().unwrap_or_default(),
    };

    let task_state = state.clone();
    tokio::spawn(async move {
        // Re-build the provider inside the task from the already-validated config
        // so the heavy work — the network call to the LLM — happens off the IPC
        // connection. We re-check `provider()` only to obtain the boxed provider;
        // the None branch is unreachable in practice but handled defensively rather
        // than unwrapped. Going through the run-resolver here (not at validation
        // above) keeps the Ollama auto-launch off the IPC connection too.
        let Some(provider) = crate::pipeline::llm_provider_for_run(&task_state, &llm_cfg).await
        else {
            return;
        };

        // Surface this re-run in the queue as an active "Cleaning up…" item.
        task_state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::CleaningUp,
        });

        match crate::pipeline::run_llm_stage(
            &task_state,
            &id,
            PipelineStage::CleaningUp,
            &*provider,
            &llm_cfg.prompt,
            &source,
        )
        .await
        {
            Ok(cleaned) => {
                // Re-assert the original alongside the freshly cleaned live text.
                // Reusing `update_transcript` (the same call the pipeline makes)
                // keeps `original_transcript` pinned to the raw source we cleaned.
                if let Err(e) = task_state
                    .catalog
                    .update_transcript(&id, &cleaned, &source, &llm_cfg.model)
                    .await
                {
                    tracing::error!(error = %e, "rerun_cleanup: failed to update transcript");
                    task_state.events.emit(DaemonEvent::TranscriptionFailed {
                        id,
                        error: e.to_string(),
                    });
                    return;
                }
                // Record which cleanup model ran (diarization state is unchanged
                // by a text-only re-clean, so preserve whatever was stored —
                // both the flag and the diarizer model).
                if let Err(e) = task_state
                    .catalog
                    .update_processing_meta(
                        &id,
                        Some(&llm_cfg.model),
                        recording.diarized,
                        recording.diarization_model.as_deref(),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "rerun_cleanup: failed to update processing meta");
                }

                // A re-run is often requested precisely because the prior cleanup
                // failed, so clear the terminal CleanupFailed status now that it
                // succeeded, otherwise the recording reads as failed forever even
                // though it cleaned fine. `update_transcript` above already cleared
                // the error_kind/error_message columns; only the status remained.
                // Best-effort and scoped to CleanupFailed so a re-run never masks an
                // unrelated terminal status (e.g. HookFailed).
                if recording.status == RecordingStatus::CleanupFailed {
                    if let Err(e) = task_state
                        .catalog
                        .update_status(&id, RecordingStatus::Done)
                        .await
                    {
                        tracing::warn!(error = %e, "rerun_cleanup: failed to clear CleanupFailed status");
                    }
                }

                // TL-CONSISTENCY: re-derive the cleaned timing variant from the raw
                // words realigned to the re-cleaned text, so Timeline/Synced match
                // the panel. Raw timing is untouched. Best-effort + gated inside.
                crate::pipeline::reflow_cleaned_timing(&task_state, &id, &cleaned).await;

                // PB-COMPOUND: the live transcript is now this cleanup's output, so
                // the version chain has to reflect it — otherwise Compare-versions
                // and Revert keep showing the prior run's cleanup text. Rebuild it
                // as raw (idx 0) + this cleanup (idx 1); a manual re-clean collapses
                // any earlier multi-step chain since the live text is now solely
                // this cleanup output. Best-effort.
                let versions = vec![
                    phoneme_core::catalog::TranscriptVersion {
                        idx: 0,
                        step_id: None,
                        label: Some("Original (raw)".to_string()),
                        model: None,
                        text: source.clone(),
                    },
                    phoneme_core::catalog::TranscriptVersion {
                        idx: 1,
                        step_id: Some("cleanup".to_string()),
                        label: Some(format!("Cleanup ({})", llm_cfg.model)),
                        model: Some(llm_cfg.model.clone()),
                        text: cleaned.clone(),
                    },
                ];
                if let Err(e) = task_state
                    .catalog
                    .replace_transcript_versions(&id, &versions)
                    .await
                {
                    tracing::warn!(error = %e, "rerun_cleanup: failed to refresh transcript versions");
                }

                // Re-embed the new text so semantic search stays consistent,
                // mirroring the pipeline and UpdateTranscript paths.
                let embedder = task_state.embedder.read().await.as_ref().cloned();
                if let Some(embedder) = embedder {
                    crate::pipeline::embed_and_store(embedder, &task_state.catalog, &id, &cleaned)
                        .await;
                }

                // Emit the same event the UI already listens for after a manual
                // transcript change so the detail/list views refresh in place.
                task_state
                    .events
                    .emit(DaemonEvent::TranscriptUpdated { id });
            }
            Err(e) => {
                tracing::error!(error = %e, "rerun_cleanup: LLM post-processing failed");
                task_state.events.emit(DaemonEvent::TranscriptionFailed {
                    id,
                    error: e.to_string(),
                });
            }
        }
    });

    ok_null()
}

/// Generate (or regenerate) an LLM summary of a recording's current transcript
/// on demand. Like `rerun_cleanup`, the network call runs in a spawned task so
/// it doesn't block the IPC connection; the UI listens for `SummaryUpdated`.
/// `model`/`prompt` override the configured summary model/prompt for this run
/// only; `conn` carries the one-time provider/endpoint/key override (mirroring
/// `rerun_cleanup`). None of it is ever persisted.
pub(super) async fn rerun_summary(
    state: &AppState,
    id: phoneme_core::RecordingId,
    model: Option<String>,
    prompt: Option<String>,
    conn: SummaryProviderOverrides,
) -> Response {
    let recording = match state.catalog.get(&id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return not_found(format!("recording {id} not found"));
        }
        Err(e) => {
            return err_response(&e);
        }
    };

    let transcript = recording.transcript.clone().unwrap_or_default();
    if transcript.trim().is_empty() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcript to summarize".into(),
        });
    }

    // Resolve the base (llm_cfg, prompt) from the migrated `summary` Playbook entry
    // so editing it in the Playbook changes what an on-demand re-run does — the
    // Playbook is the source of truth. The Re-run modal's one-time overrides (a
    // non-empty model / prompt) then layer on top and still win. When no `summary`
    // entry exists (a user deleted it) fall back to the legacy
    // [summary]/[llm_post_process] path (`generate_summary`) so behavior is never
    // worse than today. `Resolution` carries whichever path we took to the probe
    // and the spawned task.
    let cfg = (**state.config.load()).clone();
    let model = model.filter(|m| !m.trim().is_empty());
    let prompt = prompt.filter(|p| !p.trim().is_empty());

    enum Resolution {
        /// The migrated `summary` entry drives this run; one-shot overrides
        /// already layered on. `generate_summary_with` dispatches it directly.
        Entry {
            llm_cfg: phoneme_core::config::LlmPostProcessConfig,
            prompt: String,
            endpoint_hint: Option<String>,
        },
        /// No `summary` entry — the legacy `[summary]` section drives this run via
        /// `generate_summary`, with the one-shot overrides baked into `cfg`. Boxed
        /// because `Config` is large and this is the rare path (clippy
        /// large_enum_variant — keep the common `Entry` arm cheap to move).
        Legacy {
            cfg: Box<phoneme_core::config::Config>,
        },
    }

    let resolution = match crate::pipeline::entry_config_for_target(&cfg, "summary") {
        Some((base_llm, base_prompt)) => {
            // Layer the one-shot model + prompt overrides via the shared helper
            // that `rerun_cleanup` (and the tests) use — the single source of truth
            // for "non-empty override wins, blank is ignored".
            let (mut llm_cfg, entry_prompt) = crate::pipeline::apply_oneshot_overrides(
                base_llm,
                base_prompt,
                model.as_deref(),
                prompt.as_deref(),
            );
            // Layer the one-shot connection override (provider/endpoint/key) on top,
            // exactly like `rerun_cleanup` — a non-empty provider/key wins, an
            // explicit (even empty) URL is honored. `LlmPostProcessConfig` is the
            // same connection shape `apply_to` writes onto `[summary]`, so reuse it.
            conn.apply_to_llm(&mut llm_cfg);
            // On-demand: a `summary` entry that pins `provider = "none"` falls back
            // to the global `[llm_post_process]` connection so a Re-run isn't blocked
            // just because the auto-summary step is off. The helper is a no-op when
            // the entry already resolves to a usable provider, so the common case
            // (entry inherits a working connection) keeps the entry's model/url/key.
            let mut llm_cfg = crate::pipeline::ondemand_connection(&state.llm, &cfg, llm_cfg);
            // A one-shot `--model` must still win even when we fell back to the
            // global connection above (which carries the global's own model).
            if let Some(m) = model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                llm_cfg.model = m.to_string();
            }
            // Name the endpoint in any real-error message (a stale per-step URL
            // is the classic cause and invisible in a generic message).
            let endpoint_hint = {
                let url = llm_cfg.api_url.trim();
                (!url.is_empty()).then(|| url.to_string())
            };
            Resolution::Entry {
                llm_cfg,
                prompt: entry_prompt,
                endpoint_hint,
            }
        }
        None => {
            // Bake the one-shot overrides into the [summary] section of a clone,
            // then let `generate_summary` resolve it exactly as it did before.
            let mut cfg_legacy = cfg.clone();
            if let Some(m) = &model {
                cfg_legacy.summary.model = m.clone();
            }
            if let Some(p) = &prompt {
                cfg_legacy.summary.prompt = p.clone();
            }
            // Connection override onto [summary] — `summary_llm_config` then resolves
            // it (inheriting the cleanup connection for any blank field) as ever.
            conn.apply_to(&mut cfg_legacy.summary);
            Resolution::Legacy {
                cfg: Box::new(cfg_legacy),
            }
        }
    };

    // Require a usable LLM provider up front so the user gets a clear error
    // rather than a silent no-op. The summary generators re-check defensively.
    // The Entry connection already routed through `ondemand_connection` above; the
    // Legacy path's `summary_llm_config` inherits `[summary]→[llm_post_process]`, so
    // the helper here only adds the final fallback when even that yields no provider.
    let probe = match &resolution {
        Resolution::Entry { llm_cfg, .. } => llm_cfg.clone(),
        Resolution::Legacy { cfg } => crate::pipeline::ondemand_connection(
            &state.llm,
            cfg,
            crate::pipeline::summary_llm_config(cfg),
        ),
    };
    if state.llm.provider(&probe).is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::InvalidConfig,
            message: "no LLM provider configured for summaries (set a summary or [llm_post_process] provider)"
                .into(),
        });
    }

    // Snapshot the status so the spawned task can clear a stale SummarizeFailed
    // on success without re-fetching (RecordingStatus is Copy).
    let prev_status = recording.status;
    let task_state = state.clone();
    tokio::spawn(async move {
        // Surface this re-run in the queue as an active "Summarizing…" item.
        task_state.events.emit(DaemonEvent::PipelineStageChanged {
            id: id.clone(),
            stage: PipelineStage::Summarizing,
        });
        let result = match resolution {
            Resolution::Entry {
                llm_cfg,
                prompt,
                endpoint_hint,
            } => {
                crate::pipeline::generate_summary_with(
                    &task_state,
                    &id,
                    &transcript,
                    llm_cfg,
                    &prompt,
                    endpoint_hint.as_deref(),
                )
                .await
            }
            Resolution::Legacy { cfg } => {
                crate::pipeline::generate_summary(&task_state, &cfg, &id, &transcript).await
            }
        };
        match result {
            Ok((summary, model)) => {
                if let Err(e) = task_state
                    .catalog
                    .update_summary(&id, &summary, Some(&model))
                    .await
                {
                    tracing::error!(error = %e, "rerun_summary: failed to persist summary");
                    task_state.events.emit(DaemonEvent::SummaryFailed {
                        id,
                        error: e.to_string(),
                    });
                    return;
                }
                // Clear a stale SummarizeFailed status now that the summary
                // succeeded — otherwise the recording reads as failed forever even
                // though the re-run worked. Best-effort and scoped to
                // SummarizeFailed so a re-run never masks an unrelated terminal
                // status. The error_kind/error_message columns are left as-is; the
                // list/detail "failed" state keys off `status`, which is now Done,
                // so the recording no longer surfaces as failed.
                if prev_status == RecordingStatus::SummarizeFailed {
                    if let Err(e) = task_state
                        .catalog
                        .update_status(&id, RecordingStatus::Done)
                        .await
                    {
                        tracing::warn!(error = %e, "rerun_summary: failed to clear SummarizeFailed status");
                    }
                }
                task_state.events.emit(DaemonEvent::SummaryUpdated { id });
            }
            Err(reason) => {
                task_state
                    .events
                    .emit(DaemonEvent::SummaryFailed { id, error: reason });
            }
        }
    });

    ok_null()
}

/// Generate (or regenerate) a whole-meeting digest on demand — the meeting-scope
/// twin of [`rerun_summary`]. Loads every track of the meeting, assembles the
/// merged (source-labelled) transcript, and runs the configured meeting template
/// (a `scope = Meeting` recipe) over it via `run_meeting_recipe`. Like
/// `rerun_summary`, the LLM call runs in a spawned task so it doesn't block the IPC
/// connection; the result is stored keyed by `meeting_id` and the UI listens for
/// `MeetingDigestUpdated`. `model` overrides the configured summary model for this
/// run only (never persisted); `recipe_id`, when set, overrides the configured
/// `meeting_recipe_id` for this run only ("run with template X once") — a missing
/// or non-meeting-scope id falls back to the built-in digest inside the executor.
pub(super) async fn rerun_meeting_digest(
    state: &AppState,
    meeting_id: String,
    model: Option<String>,
    recipe_id: Option<String>,
    conn: SummaryProviderOverrides,
) -> Response {
    let tracks = match state.catalog.list_by_meeting(&meeting_id).await {
        Ok(rows) if !rows.is_empty() => rows,
        Ok(_) => return not_found(format!("meeting {meeting_id} not found")),
        Err(e) => return err_response(&e),
    };

    // Need at least one track with a transcript to have anything to digest. The
    // generator re-checks this defensively, but report it up front so the caller
    // gets a clear error instead of a silent no-op.
    let merged = crate::pipeline::assemble_meeting_transcript(&tracks);
    if merged.trim().is_empty() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcribed tracks yet — nothing to digest".into(),
        });
    }

    // Bake the one-shot overrides into a config clone, then let the meeting
    // executor resolve the connection exactly as the auto-digest does. The digest
    // reuses the summary provider/key/url; only the model can differ per-run
    // (mirroring `phoneme summarize --model`), and `recipe_id` can swap the meeting
    // template for this run only (`""`/`None` keeps the configured one).
    let mut cfg = (**state.config.load()).clone();
    if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
        cfg.summary.model = m;
    }
    if let Some(r) = recipe_id.filter(|r| !r.trim().is_empty()) {
        cfg.meeting_recipe_id = r;
    }
    // One-time connection override (provider/endpoint/key) onto [summary], mirroring
    // `rerun_summary` — `summary_llm_config` then resolves it for the digest run.
    conn.apply_to(&mut cfg.summary);

    // On-demand: the digest reuses the summary connection, which can pin
    // `provider = "none"` (auto-summary off) even with a working global LLM. Route
    // it through `ondemand_connection`; when it falls back to the global, write that
    // connection (provider/url/key) onto `[summary]` so both the gate below and the
    // executor's digest step (which re-resolves from `[summary]`) run on it. The
    // one-shot `--model` already on `cfg.summary.model` is left untouched so it
    // still wins; a no-op when the summary connection already yields a provider.
    let resolved = crate::pipeline::summary_llm_config(&cfg);
    let effective = crate::pipeline::ondemand_connection(&state.llm, &cfg, resolved.clone());
    if effective.provider != resolved.provider {
        cfg.summary.provider = effective.provider.clone();
        cfg.summary.api_url = effective.api_url.clone();
        cfg.summary.set_api_key(effective.api_key_str().to_string());
    }

    // Require a usable LLM provider up front so the user gets a clear error rather
    // than a silent failure inside the spawned task.
    if state.llm.provider(&effective).is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::InvalidConfig,
            message: "no LLM provider configured for summaries (set a summary or [llm_post_process] provider)".into(),
        });
    }

    // The LlmActivity/skip stream is keyed per-recording, so attribute this run to
    // the meeting's first track; the digest itself is stored against the meeting.
    let event_id = tracks[0].id.clone();
    let task_state = state.clone();
    tokio::spawn(async move {
        task_state.events.emit(DaemonEvent::PipelineStageChanged {
            id: event_id.clone(),
            stage: PipelineStage::Summarizing,
        });
        match crate::pipeline::run_meeting_recipe(&task_state, &cfg, &event_id, &tracks).await {
            Ok((digest, model)) => {
                if let Err(e) = task_state
                    .catalog
                    .update_meeting_digest(&meeting_id, &digest, Some(&model))
                    .await
                {
                    tracing::error!(error = %e, "rerun_meeting_digest: failed to persist digest");
                    task_state.events.emit(DaemonEvent::MeetingDigestFailed {
                        meeting_id,
                        error: e.to_string(),
                    });
                    return;
                }
                task_state
                    .events
                    .emit(DaemonEvent::MeetingDigestUpdated { meeting_id });
            }
            Err(reason) => {
                task_state.events.emit(DaemonEvent::MeetingDigestFailed {
                    meeting_id,
                    error: reason,
                });
            }
        }
    });

    ok_null()
}

/// Derive the stable storage key for a period digest from its canonical
/// (already-normalized) range bounds. Re-running the same window yields the same
/// key, so the upsert overwrites rather than accumulating near-duplicate rows.
/// Keyed on the range — never the human `label`, since two ranges can share one.
/// The bounds are serialized to RFC3339 so the key is deterministic across runs.
pub(super) fn period_digest_key(
    since: chrono::DateTime<chrono::Local>,
    until: chrono::DateTime<chrono::Local>,
) -> String {
    format!("{}|{}", since.to_rfc3339(), until.to_rfc3339())
}

/// Generate (or regenerate) a period digest on demand — the date-window twin of
/// [`rerun_meeting_digest`]. Selects every recording in `since..until` (oldest
/// first), assembles their transcripts into one chronological document, and runs
/// it through the configured summary provider with the period-scope prompt. Like
/// `rerun_meeting_digest`, the LLM call runs in a spawned task so it doesn't block
/// the IPC connection; the result is stored keyed by the range (see
/// [`period_digest_key`]) and the UI listens for `PeriodDigestUpdated`. `model`
/// overrides the configured summary model for this run only (never persisted).
pub(super) async fn rerun_period_digest(
    state: &AppState,
    since: chrono::DateTime<chrono::Local>,
    until: chrono::DateTime<chrono::Local>,
    label: String,
    model: Option<String>,
    conn: SummaryProviderOverrides,
) -> Response {
    // Select the window's recordings, oldest-first so the merged transcript reads
    // chronologically. This is the existing list query — no new SQL.
    let filter = phoneme_core::ListFilter {
        since: Some(since),
        until: Some(until),
        sort_desc: Some(false),
        ..Default::default()
    };
    let recordings = match state.catalog.list(&filter).await {
        Ok(rows) if !rows.is_empty() => rows,
        Ok(_) => return not_found("no recordings in that range".into()),
        Err(e) => return err_response(&e),
    };

    // Need at least one recording with a transcript to have anything to digest.
    // The generator re-checks this defensively, but report it up front so the
    // caller gets a clear error instead of a silent no-op.
    let merged = crate::pipeline::assemble_period_transcript(&recordings);
    if merged.trim().is_empty() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: "no transcribed recordings in that range — nothing to digest".into(),
        });
    }

    // Bake the one-shot model override into a [summary] clone, then let the digest
    // generator resolve the summary connection exactly as the meeting digest does.
    let mut cfg = (**state.config.load()).clone();
    if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
        cfg.summary.model = m;
    }
    // One-time connection override (provider/endpoint/key) onto [summary], mirroring
    // `rerun_summary` / the meeting digest.
    conn.apply_to(&mut cfg.summary);

    // On-demand: same pattern as `rerun_meeting_digest` — the period digest reuses
    // the summary connection, which can pin `provider = "none"` even with a working
    // global LLM. Route through `ondemand_connection`; if it falls back to the
    // global, write that connection onto `[summary]` so the spawned task sees it.
    let resolved = crate::pipeline::summary_llm_config(&cfg);
    let effective = crate::pipeline::ondemand_connection(&state.llm, &cfg, resolved.clone());
    if effective.provider != resolved.provider {
        cfg.summary.provider = effective.provider.clone();
        cfg.summary.api_url = effective.api_url.clone();
        cfg.summary.set_api_key(effective.api_key_str().to_string());
    }

    // Require a usable LLM provider up front so the user gets a clear error rather
    // than a silent failure inside the spawned task.
    if state.llm.provider(&effective).is_none() {
        return Response::Err(IpcError {
            kind: IpcErrorKind::InvalidConfig,
            message: "no LLM provider configured for summaries (set a summary or [llm_post_process] provider)".into(),
        });
    }

    // Derive the storage key from the (normalized) range before spawning, so the
    // result and any failure event both reference the same stable key.
    let key = period_digest_key(since, until);
    let source_count = recordings.len() as i64;

    // The LlmActivity/skip stream is keyed per-recording, so attribute this run to
    // the window's first recording; the digest itself is stored against the range.
    let event_id = recordings[0].id.clone();
    let task_state = state.clone();
    tokio::spawn(async move {
        task_state.events.emit(DaemonEvent::PipelineStageChanged {
            id: event_id.clone(),
            stage: PipelineStage::Summarizing,
        });
        match crate::pipeline::generate_period_digest(&task_state, &cfg, &event_id, &recordings)
            .await
        {
            Ok((digest, model)) => {
                if let Err(e) = task_state
                    .catalog
                    .update_period_digest(
                        &key,
                        &label,
                        since,
                        until,
                        &digest,
                        Some(&model),
                        source_count,
                    )
                    .await
                {
                    tracing::error!(error = %e, "rerun_period_digest: failed to persist digest");
                    task_state.events.emit(DaemonEvent::PeriodDigestFailed {
                        key,
                        error: e.to_string(),
                    });
                    return;
                }
                task_state
                    .events
                    .emit(DaemonEvent::PeriodDigestUpdated { key });
            }
            Err(reason) => {
                task_state
                    .events
                    .emit(DaemonEvent::PeriodDigestFailed { key, error: reason });
            }
        }
    });

    ok_null()
}

/// Import an existing audio file: decode it to a canonical WAV under the audio
/// dir, insert a catalog row, and enqueue it for the normal transcription
/// pipeline. Mirrors `DaemonRecorder::stop` (catalog row at `Transcribing` +
/// `inbox.enqueue`) so an imported file is processed exactly like a mic
/// recording — the only difference is where the WAV came from.
pub(super) async fn import_recording(
    state: &AppState,
    path: String,
    recipe_id: Option<String>,
    ext_ref: Option<String>,
) -> Response {
    // One-time Playbook recipe for this import (mirrors RecordStart). Validate it
    // up front — before the (potentially slow) decode — so a meeting template is
    // rejected immediately rather than after the work. None/empty ⇒ the global
    // default; an unknown id is left to `resolve_recipe`'s lenient fallback
    // (→ default), matching record/retranscribe.
    let recipe_id = match validate_import_recipe(&state.config.load(), recipe_id) {
        Ok(rid) => rid,
        Err(message) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::InvalidConfig,
                message,
            })
        }
    };

    // Idempotent import: if the caller supplied an external-reference key and a
    // recording already carries it, return that one untouched instead of
    // importing a duplicate — checked BEFORE the decode so a re-import is cheap.
    // Lets a client (the youtube-note project) fire-and-forget and reconcile via
    // `phoneme list --json`'s `ext_ref` without its own dedup bookkeeping.
    let ext_ref = ext_ref
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(ref key) = ext_ref {
        match state.catalog.find_id_by_ext_ref(key).await {
            Ok(Some(existing)) => {
                tracing::info!(id = %existing, ext_ref = %key, "import deduped on ext_ref");
                return Response::Ok(
                    serde_json::json!({ "id": existing.to_string(), "reused": true }),
                );
            }
            Ok(None) => {}
            Err(e) => return err_response(&e),
        }
    }

    let requested = std::path::PathBuf::from(&path);

    // Canonicalize so the path we open is a fully-resolved, real filesystem
    // location (resolves `..`, symlinks, and relative components). The dialog hands
    // us absolute paths already; this hardens the arbitrary-client-path bypass by
    // ensuring we never act on a half-resolved or traversal path. It also checks
    // existence atomically, which avoids a TOCTOU window.
    let input = match std::fs::canonicalize(&requested) {
        Ok(p) => p,
        Err(e) => {
            return not_found(format!("could not resolve path {path}: {e}"));
        }
    };

    if !phoneme_audio::is_supported_extension(&input) {
        return Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: format!(
                "unsupported audio format (supported: {})",
                phoneme_audio::SUPPORTED_EXTENSIONS.join(", ")
            ),
        });
    }

    // Reject oversized files up front via metadata, before decoding allocates
    // anything. Doubles as the coarse memory bound for the import path.
    match std::fs::metadata(&input) {
        Ok(meta) => {
            if !meta.is_file() {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("not a regular file: {path}"),
                });
            }
            if exceeds_import_size_cap(meta.len()) {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!(
                        "file too large to import ({} bytes; max {} bytes / {} GiB)",
                        meta.len(),
                        MAX_IMPORT_BYTES,
                        MAX_IMPORT_BYTES / (1024 * 1024 * 1024)
                    ),
                });
            }
        }
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::Io,
                message: format!("could not stat {path}: {e}"),
            });
        }
    }

    let id = phoneme_core::RecordingId::new();
    let started_at = chrono::Local::now();
    let audio_path = state
        .paths
        .audio_dir
        .join(id.day_folder())
        .join(format!("{}.wav", id.file_stem()));

    // Decode is CPU-bound and blocking — run it off the async runtime so the
    // IPC connection (and the single-connection Tauri bridge) stays responsive.
    let decode_out = audio_path.clone();
    let decode_result = tokio::task::spawn_blocking(move || {
        phoneme_audio::decode_to_canonical_wav(&input, &decode_out)
    })
    .await;
    let duration_ms = match decode_result {
        Ok(Ok(ms)) => ms,
        Ok(Err(e)) => {
            return Response::Err(IpcError {
                kind: error_to_kind(&e),
                message: format!("failed to decode audio: {e}"),
            });
        }
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: format!("decode task panicked: {e}"),
            });
        }
    };

    let row = phoneme_core::Recording {
        id: id.clone(),
        started_at,
        duration_ms,
        audio_path: audio_path.to_string_lossy().into_owned(),
        in_place: false,
        transcript: None,
        model: None,
        // Queued, not Transcribing: the import rides the serial inbox; the
        // pipeline flips it to Transcribing when the worker claims it.
        status: RecordingStatus::Queued,
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
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        // Persisted by the base insert; drives future idempotent re-imports.
        ext_ref: ext_ref.clone(),
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    if let Err(e) = state.catalog.insert(&row).await {
        // Clean up the WAV we just wrote — no row means it's orphaned.
        let _ = tokio::fs::remove_file(&audio_path).await;
        return err_response(&e);
    }

    // Stash the one-time recipe against this id so `pipeline::run` resolves that
    // chain instead of the global default — the same per-job ledger custom hotkeys
    // and retranscribe use. Consumed-and-removed by the pipeline; must be in place
    // before the enqueue, since the queue worker can claim the job immediately.
    stash_hotkey_overrides(state, &id, recipe_id, None);

    let payload = HookPayload {
        id: id.clone(),
        timestamp: started_at,
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    if let Err(e) = state.inbox.enqueue(&payload).await {
        // No queue entry means this import would never be processed — roll the
        // catalog row and the canonical WAV back so it can't sit in the list
        // stuck on Queued forever. The caller can simply retry. Also drop the
        // recipe we just stashed so the never-processed id leaves nothing behind.
        let _ = state.catalog.delete(&id).await;
        let _ = tokio::fs::remove_file(&audio_path).await;
        state
            .pending_recipe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        return err_response(&e);
    }

    state.events.emit(DaemonEvent::RecordingStopped {
        id: id.clone(),
        duration_ms,
        audio_path: audio_path.to_string_lossy().into_owned(),
        meeting_id: None,
    });
    tracing::info!(id = %id, source = %path, ms = duration_ms, "imported recording");
    Response::Ok(serde_json::json!({ "id": id.to_string() }))
}

/// Export a `[start_ms, end_ms)` slice of a recording's audio to a new WAV (S7).
/// Looks up the recording's audio path in the catalog, picks the output path
/// (the caller's `out_path`, or a `_clip_<start>-<end>` sibling of the source),
/// then runs the pure `phoneme_audio::wav::clip_wav` helper on a blocking thread
/// (read + slice + write is CPU/IO-bound). Ok `{"path":"<written>"}`.
pub(super) async fn export_clip(
    state: &AppState,
    id: phoneme_core::RecordingId,
    start_ms: i64,
    end_ms: i64,
    out_path: Option<String>,
) -> Response {
    let rec = match state.catalog.get(&id).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(format!("recording {id} not found")),
        Err(e) => return err_response(&e),
    };

    let src = std::path::PathBuf::from(&rec.audio_path);
    // Default output: next to the source WAV with a `_clip_<start>-<end>` suffix
    // (milliseconds), so repeated clips of the same recording don't collide.
    let dest = match out_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let stem = src
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| id.to_string());
            let name = format!("{stem}_clip_{start_ms}-{end_ms}.wav");
            match src.parent() {
                Some(dir) => dir.join(name),
                None => std::path::PathBuf::from(name),
            }
        }
    };

    // Never write the clip over the recording's own source audio: `clip_wav` reads
    // the whole source into memory then atomically replaces `dest`, so dest==src
    // would truncate the original to the slice while the catalog row still points
    // here — irreversible data loss. Compare by resolved location so `./x.wav` vs.
    // the absolute path (and Windows separator/case differences) all match. When
    // dest already exists, canonicalize the FILE itself (not just its parent) so a
    // case- or short-name-spelled alias of the source folds to the same path; only
    // fall back to parent+join for a dest that doesn't exist yet (where it can't be
    // the source file anyway).
    let src_resolved = src.canonicalize().unwrap_or_else(|_| src.clone());
    let dest_resolved = dest.canonicalize().ok().or_else(|| {
        dest.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
            .or_else(|| std::env::current_dir().ok())
            .and_then(|dir| dest.file_name().map(|f| dir.join(f)))
    });
    if dest == src || dest_resolved.as_deref() == Some(src_resolved.as_path()) {
        return err_response(&phoneme_core::Error::InvalidConfig(
            "clip output path must differ from the recording's own audio file".into(),
        ));
    }

    // Read + slice + write is blocking — keep it off the async runtime so the
    // IPC connection (and the single-connection tray bridge) stays responsive.
    let dest_for_task = dest.clone();
    let result = tokio::task::spawn_blocking(move || {
        phoneme_audio::wav::clip_wav(&src, &dest_for_task, start_ms, end_ms)
    })
    .await;

    match result {
        Ok(Ok(frames)) => {
            let path = dest.to_string_lossy().into_owned();
            tracing::info!(id = %id, %path, start_ms, end_ms, frames, "exported audio clip");
            Response::Ok(serde_json::json!({ "path": path }))
        }
        Ok(Err(e)) => err_response(&e),
        Err(e) => Response::Err(IpcError {
            kind: IpcErrorKind::Internal,
            message: format!("clip task panicked: {e}"),
        }),
    }
}

/// Edit a recording's audio (#262): keep only `keep_ranges` and concatenate them
/// (a trim is one range; a deleted inner section is the gap between two) via the
/// pure `phoneme_audio::wav::edit_wav`, on a blocking thread (read + cut + write
/// is CPU/IO-bound). Two save modes:
/// - `new_recording`: cut to a temp WAV, then run it through the normal import
///   path so it lands as a fresh recording (original untouched) + enqueued.
///   Ok `{"id":"<new id>"}`.
/// - in place: back the original up to a `.orig-<ts>.wav` sibling, cut over the
///   source, persist the new (shorter) duration, and re-enqueue for a fresh
///   transcription + pipeline. Ok `{"id":"<same id>","backup":"<path>"}`.
pub(super) async fn edit_recording(
    state: &AppState,
    id: phoneme_core::RecordingId,
    keep_ranges: Vec<(i64, i64)>,
    new_recording: bool,
) -> Response {
    if keep_ranges.is_empty() {
        return err_response(&phoneme_core::Error::InvalidConfig(
            "edit needs at least one range to keep".into(),
        ));
    }
    let rec = match state.catalog.get(&id).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(format!("recording {id} not found")),
        Err(e) => return err_response(&e),
    };
    let src = std::path::PathBuf::from(&rec.audio_path);

    if new_recording {
        // Cut to a temp WAV in the audio dir, then import it like a dropped file
        // (canonical re-encode, size cap, fresh catalog row + enqueue). The
        // original recording is never touched.
        let tmp = state
            .paths
            .audio_dir
            .join(format!("_edit-{}.wav", id.file_stem()));
        let tmp_task = tmp.clone();
        let src_task = src.clone();
        let ranges = keep_ranges.clone();
        let cut = tokio::task::spawn_blocking(move || {
            phoneme_audio::wav::edit_wav(&src_task, &tmp_task, &ranges)
        })
        .await;
        match cut {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return err_response(&e),
            Err(e) => {
                return Response::Err(IpcError {
                    kind: IpcErrorKind::Internal,
                    message: format!("edit task panicked: {e}"),
                })
            }
        }
        let resp = import_recording(state, tmp.to_string_lossy().into_owned(), None, None).await;
        let _ = tokio::fs::remove_file(&tmp).await; // import copied it in; drop the temp
        return resp;
    }

    // ── Replace in place ──────────────────────────────────────────────────────
    // Back the original up before overwriting it (recoverable on a bad edit),
    // then cut over the source and re-transcribe the edited audio.
    let backup = {
        let ts = chrono::Local::now().format("%Y%m%dT%H%M%S");
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.to_string());
        let name = format!("{stem}.orig-{ts}.wav");
        match src.parent() {
            Some(dir) => dir.join(name),
            None => std::path::PathBuf::from(name),
        }
    };
    let src_task = src.clone();
    let backup_task = backup.clone();
    let ranges = keep_ranges.clone();
    let result = tokio::task::spawn_blocking(move || -> phoneme_core::error::Result<i64> {
        std::fs::copy(&src_task, &backup_task).map_err(|e| {
            phoneme_core::Error::Internal(format!("backing up original audio: {e}"))
        })?;
        // edit_wav reads the whole source into memory before writing, so editing
        // over the source in place is safe; write_wav replaces it atomically.
        phoneme_audio::wav::edit_wav(&src_task, &src_task, &ranges)?;
        phoneme_audio::wav::duration_ms(&src_task)
    })
    .await;
    let new_duration = match result {
        Ok(Ok(ms)) => ms,
        Ok(Err(e)) => return err_response(&e),
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: format!("edit task panicked: {e}"),
            })
        }
    };

    // Persist the new (shorter) duration + flip to Queued, then re-enqueue so the
    // edited audio gets a fresh transcription + the full pipeline (same id). The
    // stale transcript stays visible until the new one lands (status = Queued),
    // mirroring a plain re-transcribe.
    if let Err(e) = state
        .catalog
        .update_status_and_duration(&id, RecordingStatus::Queued, new_duration)
        .await
    {
        return err_response(&e);
    }
    let payload = HookPayload {
        id: id.clone(),
        timestamp: rec.started_at,
        transcript: String::new(),
        audio_path: rec.audio_path.clone(),
        duration_ms: new_duration,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    match state.inbox.enqueue(&payload).await {
        Ok(()) => {
            tracing::info!(id = %id, backup = %backup.display(), new_duration, "edited recording in place");
            Response::Ok(serde_json::json!({
                "id": id.to_string(),
                "backup": backup.to_string_lossy(),
            }))
        }
        Err(e) => {
            // Roll the status back so the recording is not stuck as Queued with
            // no inbox entry to advance it.
            let _ = state
                .catalog
                .update_status_and_duration(&id, RecordingStatus::Done, new_duration)
                .await;
            err_response(&e)
        }
    }
}

/// A `.wav` on disk whose RecordingId has no catalog row — a candidate to
/// re-link in [`reimport_from_disk`].
pub(super) struct ReimportCandidate {
    id: phoneme_core::RecordingId,
    path: std::path::PathBuf,
    duration_ms: i64,
    started_at: chrono::DateTime<chrono::Local>,
}

/// Reconstruct a RecordingId from a day folder (`YYYY-MM-DD`) + a file stem
/// (`HHmmssNNN`) — the inverse of the `audio_dir/<day>/<stem>.wav` layout that
/// `RecordingId::day_folder()`/`file_stem()` produce. `None` for anything that
/// isn't a valid id (e.g. a user-dropped file with a different name).
pub(super) fn id_from_path_parts(day_name: &str, stem: &str) -> Option<phoneme_core::RecordingId> {
    let date_digits: String = day_name.chars().filter(|c| *c != '-').collect();
    phoneme_core::RecordingId::parse(format!("{date_digits}T{stem}"))
}

/// The original wall-clock time encoded in a RecordingId (`YYYYMMDDTHHmmssNNN`),
/// so a re-imported row keeps its real timestamp instead of "now". Falls back to
/// the current time only if the slices somehow don't parse (the id is already
/// shape-validated by `parse`).
pub(super) fn started_at_from_id(id: &phoneme_core::RecordingId) -> chrono::DateTime<chrono::Local> {
    use chrono::{Local, NaiveDate, NaiveTime, TimeZone};
    let s = id.as_str();
    let build = || -> Option<chrono::DateTime<Local>> {
        let y: i32 = s.get(0..4)?.parse().ok()?;
        let mo: u32 = s.get(4..6)?.parse().ok()?;
        let d: u32 = s.get(6..8)?.parse().ok()?;
        let h: u32 = s.get(9..11)?.parse().ok()?;
        let mi: u32 = s.get(11..13)?.parse().ok()?;
        let se: u32 = s.get(13..15)?.parse().ok()?;
        let dt = NaiveDate::from_ymd_opt(y, mo, d)?.and_time(NaiveTime::from_hms_opt(h, mi, se)?);
        Local.from_local_datetime(&dt).single()
    };
    build().unwrap_or_else(Local::now)
}

/// Synchronously walk `audio_dir/<YYYY-MM-DD>/<HHmmssNNN>.wav`, collecting every
/// `.wav` whose path reconstructs to a valid RecordingId. Blocking std::fs (the
/// caller runs it off the runtime); no new crate dependency. Unreadable dirs are
/// skipped rather than failing the whole scan.
pub(super) fn scan_audio_dir(audio_dir: &std::path::Path) -> Vec<ReimportCandidate> {
    let mut out = Vec::new();
    let Ok(days) = std::fs::read_dir(audio_dir) else {
        return out;
    };
    for day in days.flatten() {
        let day_path = day.path();
        if !day_path.is_dir() {
            continue;
        }
        let Some(day_name) = day_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(&day_path) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(id) = id_from_path_parts(day_name, stem) else {
                continue;
            };
            let duration_ms = phoneme_audio::wav::duration_ms(&p).unwrap_or(0);
            let started_at = started_at_from_id(&id);
            out.push(ReimportCandidate {
                id,
                path: p,
                duration_ms,
                started_at,
            });
        }
    }
    out
}

/// Re-link audio files that have no catalog row — the safe counterpart to the
/// destructive `doctor --rebuild-catalog`. Scans the audio dir, and for every
/// `.wav` whose RecordingId isn't already in the catalog inserts a `Queued` row
/// pointing at the existing file (no copy, original id and timestamp preserved)
/// and enqueues it for the normal pipeline. Never deletes or mutates existing
/// rows. `dry_run` returns the count and paths without writing anything.
pub(super) async fn reimport_from_disk(state: &AppState, dry_run: bool) -> Response {
    let existing: std::collections::HashSet<phoneme_core::RecordingId> =
        match state.catalog.all_ids().await {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => return err_response(&e),
        };

    let audio_dir = state.paths.audio_dir.clone();
    let candidates = match tokio::task::spawn_blocking(move || scan_audio_dir(&audio_dir)).await {
        Ok(c) => c,
        Err(e) => {
            return Response::Err(IpcError {
                kind: IpcErrorKind::Internal,
                message: format!("re-import scan task panicked: {e}"),
            });
        }
    };

    let orphans: Vec<ReimportCandidate> = candidates
        .into_iter()
        .filter(|c| !existing.contains(&c.id))
        .collect();

    if dry_run {
        let paths: Vec<String> = orphans
            .iter()
            .map(|c| c.path.to_string_lossy().into_owned())
            .collect();
        return Response::Ok(serde_json::json!({ "count": orphans.len(), "paths": paths }));
    }

    let mut count = 0usize;
    for c in orphans {
        let audio_path = c.path.to_string_lossy().into_owned();
        let row = phoneme_core::Recording {
            id: c.id.clone(),
            started_at: c.started_at,
            duration_ms: c.duration_ms,
            audio_path: audio_path.clone(),
            in_place: false,
            transcript: None,
            model: None,
            // Queued (not Transcribing): it rides the serial inbox; the worker
            // flips it to Transcribing when it claims the job — same as import.
            status: RecordingStatus::Queued,
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
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            pinned: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            entities_model: None,
            chapters_model: None,
            tasks_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            mean_confidence: None,
            detected_language: None,
            ext_ref: None,
            tags: vec![],
            entities: vec![],
            tasks: vec![],
            speaker_names: vec![],
        };
        if let Err(e) = state.catalog.insert(&row).await {
            tracing::warn!(id = %c.id, "re-import: failed to insert row, skipping: {e}");
            continue;
        }
        let payload = HookPayload {
            id: c.id.clone(),
            timestamp: c.started_at,
            transcript: String::new(),
            audio_path: audio_path.clone(),
            duration_ms: c.duration_ms,
            model: String::new(),
            metadata: HookMetadata::current(),
        };
        if let Err(e) = state.inbox.enqueue(&payload).await {
            // No queue entry means it'd sit stuck on Queued forever — roll the
            // row back (the file is untouched, so a later re-import retries it).
            let _ = state.catalog.delete(&c.id).await;
            tracing::warn!(id = %c.id, "re-import: failed to enqueue, rolled back: {e}");
            continue;
        }
        state.events.emit(DaemonEvent::RecordingStopped {
            id: c.id.clone(),
            duration_ms: c.duration_ms,
            audio_path,
            meeting_id: None,
        });
        count += 1;
    }
    tracing::info!(count, "re-imported orphaned recordings from disk");
    Response::Ok(serde_json::json!({ "count": count }))
}

