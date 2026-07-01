//! phoneme-daemon — the headless brain of Phoneme.
//!
//! Every other surface (the tray GUI, the `phoneme` CLI, global hotkeys) is a
//! thin client of this process: it owns the microphone, the catalog database,
//! the durable inbox queue, the bundled whisper-server children, and the
//! event stream. Clients speak NDJSON over a named pipe (see `phoneme-ipc`).
//!
//! A recording's life, told by module:
//! 1. [`recorder`] captures audio (with optional pre-roll and live preview)
//!    and finalizes the WAV;
//! 2. the inbox queue (`phoneme_core::InboxQueue`, held by [`app_state`])
//!    durably stores the work item as a JSON file in `pending/`;
//! 3. [`queue_worker`] claims items one at a time;
//! 4. [`pipeline`] transcribes, cleans up, titles, embeds, runs hooks,
//!    summarizes, and tags — writing every result to the catalog;
//! 5. [`event_bus`] broadcasts progress so every subscribed client (tray,
//!    `phoneme watch`, blocking `phoneme record`) follows along.
//!
//! In-place dictations skip 2–4 through [`in_place`]'s fast lane; requests
//! and event subscriptions enter via [`ipc_server`] → [`ipc_handler`].
//!
//! `main` wires the pieces: load config → build [`app_state::AppState`] →
//! recover crash leftovers ([`reconcile`]) → spawn the queue worker, both
//! whisper supervisors ([`whisper_supervisor`]), the retention loop
//! ([`retention`]), and the chunk-embedding backfill → serve IPC until the
//! shutdown coordinator ([`shutdown`]) fires — then finalize any in-flight
//! recording through the normal stop path (so a quit mid-take never leaves a
//! corrupt WAV), await the workers, and stop a daemon-launched Ollama
//! ([`ollama_launcher`]).
//!
//! Crash discipline: the IPC serve loop selects on the queue-worker and
//! main-supervisor join handles, so a crashed critical task takes the whole
//! daemon down (children die with the kill-on-close job object) instead of
//! leaving a zombie that accepts requests it can never serve. The preview
//! supervisor is deliberately not in that select — a preview crash must not
//! kill the daemon — but its handle is awaited on shutdown so its server
//! never outlives us.

#![warn(missing_docs)]

use anyhow::Result;
use clap::Parser;

mod app_state;
mod ask;
mod event_bus;
mod first_run;
mod in_place;
mod ipc_handler;
mod ipc_server;
mod logging;
mod ollama_launcher;
mod pipeline;
mod queue_worker;
mod reconcile;
mod recorder;
mod retention;
mod shutdown;
mod whisper_supervisor;

use app_state::AppState;

#[derive(Debug, Parser)]
#[command(name = "phoneme-daemon", version)]
struct Args {
    /// Run in foreground (logs to stderr instead of file).
    #[arg(long)]
    foreground: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    // Load → reconcile(+persist migrations) → apply in-memory defaults. The three
    // steps are explicit (not folded into a "load") so the disk write is visible.
    // Every load path runs the same sequence (startup here + the `ReloadConfig`
    // IPC + the queue worker's post-run reload), so the in-memory config the
    // daemon runs on is always the migrated one and the on-disk seeds freeze in
    // their migrated form. Persist failure is non-fatal — the in-memory migration
    // still applies and the next load retries.
    let mut cfg = load_config()?;
    reconcile_and_persist_config(&mut cfg);
    apply_runtime_defaults(&mut cfg);

    // Install logging before building AppState so early warnings aren't dropped:
    // job-object creation, the embedder load, and the first config-apply all
    // happen inside `AppState::new`, and without a subscriber up yet their logs
    // would go nowhere. The log dir + level come straight from the freshly-loaded
    // config (the same resolution AppState uses), so we don't need a built state
    // to configure the appender. The only thing still ahead of the subscriber is
    // `load_config()`'s own one-time migration warning, which is inherently
    // pre-logging — we need a config before we can configure logging.
    let log_dir = app_state::ResolvedPaths::from_config_in(&cfg, None)?.log_dir;
    let _guard = logging::init(&cfg, &log_dir, args.foreground)?;

    let state = AppState::new(cfg).await?;

    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "Box<dyn Any>"
        };
        let location = info
            .location()
            .map(|loc| format!("{loc}"))
            .unwrap_or_else(|| "unknown".into());
        tracing::error!(panic = true, location = %location, "Thread panicked: {}", msg);
    }));

    // Sweep any crash-leaked preview temp WAVs from a previous run.
    // Each preview WAV is written to temp_dir/phoneme-preview-<ulid>.wav and
    // removed when the recording stops normally; a crash skips the cleanup.
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("phoneme-preview-") && name.ends_with(".wav") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    reconcile::run(&state).await?;

    // Optionally warm the local diarization models now (opt-in
    // `[diarization].preload_at_startup`, local backend only), so the first
    // diarized recording doesn't pay the multi-second, ~500 MB load inline.
    // Runs on the blocking pool so it never delays the rest of startup; a load
    // failure is logged and the next real run retries.
    {
        let cfg = state.config.load();
        if cfg.diarization.preload_at_startup {
            let cache = state.transcription.diarizer_cache().clone();
            let diar_cfg = cfg.diarization.clone();
            tokio::task::spawn_blocking(move || {
                phoneme_core::diarization::preload_local_diarizer(&cache, &diar_cfg);
            });
        }
    }

    // Background task to (re-)embed recordings that lack per-chunk embeddings.
    // This doubles as the migration from the legacy whole-recording `embeddings`
    // table to sentence-aware chunk vectors: any recording with a transcript but
    // no chunk rows — including ones that only have an old whole-recording vector
    // — is backfilled so paraphrase recall improves across the existing library
    // without the user re-recording or re-transcribing anything.
    let retroactive_state = state.clone();
    tokio::spawn(async move {
        // No embedder loaded → nothing to backfill (semantic search off or the
        // model failed to load); a silent no-op.
        if retroactive_state.embedder.read().await.is_none() {
            return;
        }
        if let Ok(records) = retroactive_state
            .catalog
            .list_recordings_without_chunk_embeddings()
            .await
        {
            if !records.is_empty() {
                tracing::info!(
                    "Found {} recordings without chunk embeddings, (re-)embedding...",
                    records.len()
                );
                for r in records {
                    let Some(transcript) = r.transcript.as_ref() else {
                        continue;
                    };
                    // Re-acquire the embedder per item rather than holding the
                    // read guard across the whole loop: a large-library backfill
                    // runs for minutes, and config reloads need the write lock,
                    // so clone the Arc and drop the guard to let writers interleave
                    // between items. If the embedder is gone mid-backfill (the
                    // user turned semantic search off), stop — same exit-when-
                    // unloaded behavior as the up-front check.
                    let embedder = retroactive_state.embedder.read().await.as_ref().cloned();
                    let Some(embedder) = embedder else {
                        tracing::info!("backfill stopped: embedding model unloaded");
                        return;
                    };
                    pipeline::embed_and_store(
                        embedder,
                        &retroactive_state.catalog,
                        &r.id,
                        transcript,
                    )
                    .await;
                }
                tracing::info!("Finished backfilling chunk embeddings.");
            }
        }

        // Optional ANN (approximate nearest-neighbour) vector index: load the
        // persisted sidecar if it's healthy, else rebuild from SQLite, now that
        // the chunk-embedding backfill has drained. A no-op unless the
        // `ann-usearch` feature is compiled AND `semantic_search.ann.enabled` is
        // on; the HNSW build is CPU-heavy, so it runs here (off the startup path)
        // and never blocks the first query — search uses the brute-force scan
        // until the index is warm. Any failure leaves the index `None` (brute
        // force), so this is best-effort.
        if retroactive_state.catalog.ann_enabled() {
            tracing::info!("ANN index enabled: loading or rebuilding in the background...");
            if let Err(e) = retroactive_state.catalog.load_or_rebuild_ann_index().await {
                tracing::warn!(error = %e, "ANN index load/rebuild failed; staying on brute-force search");
            }
        }
    });

    // Start idle pre-roll pre-capture if enabled (opt-in; no-op by default).
    state.recorder.ensure_preroll(&state).await;

    // Single shutdown coordinator, owned by AppState so the IPC `Shutdown`
    // handler triggers the same channel `main` waits on.
    state.shutdown.install_signals();

    let worker_state = state.clone();
    let worker_shutdown = state.shutdown.signal.clone_receiver();
    let mut worker_handle = tokio::spawn(async move {
        if let Err(e) = queue_worker::run(worker_state, worker_shutdown).await {
            tracing::error!(error = %e, "queue worker terminated");
        }
    });

    let supervisor_state = state.clone();
    let supervisor_signal = state.shutdown.signal.clone();
    let mut supervisor_handle = tokio::spawn(async move {
        if let Err(e) = whisper_supervisor::run(supervisor_state, supervisor_signal).await {
            tracing::error!(error = %e, "whisper supervisor terminated");
        }
    });

    // Second supervisor for the optional dedicated live-preview server. Idles
    // unless `preview_whisper` is configured as a local bundled model on its own
    // port; never touches the main server above.
    let preview_sup_state = state.clone();
    let preview_sup_signal = state.shutdown.signal.clone();
    // Keep the handle so shutdown can await it — otherwise the process could exit
    // before run_preview kills its child, orphaning the 2nd whisper-server. Not
    // in the crash-detection select below: a preview-server crash must not take
    // down the daemon (preview is non-critical).
    let preview_supervisor_handle = tokio::spawn(async move {
        if let Err(e) = whisper_supervisor::run_preview(preview_sup_state, preview_sup_signal).await
        {
            tracing::error!(error = %e, "preview whisper supervisor terminated");
        }
    });

    // Fourth supervisor for the optional second live-preview server (meeting
    // "both" mode opt-in). Idles unless `recording.meeting_preview_own_server` is
    // on with a dedicated local preview model; never touches the main or first
    // preview server. Kept (like the others) so shutdown awaits it and kills any
    // child it owns; never in the crash-detect select (preview is non-critical).
    let preview2_sup_state = state.clone();
    let preview2_sup_signal = state.shutdown.signal.clone();
    let preview2_supervisor_handle = tokio::spawn(async move {
        if let Err(e) =
            whisper_supervisor::run_preview2(preview2_sup_state, preview2_sup_signal).await
        {
            tracing::error!(error = %e, "2nd preview whisper supervisor terminated");
        }
    });

    // Third supervisor for the optional dedicated dictation server. Idles unless
    // the user opts in (`[in_place].stt` local bundled + use_own_bundled_server);
    // the default/weak-box config never spawns it. Like the preview handle, kept
    // so shutdown can await it and kill any child it owns — never in the
    // crash-detect select (dictation is non-critical).
    let dictation_sup_state = state.clone();
    let dictation_sup_signal = state.shutdown.signal.clone();
    let dictation_supervisor_handle = tokio::spawn(async move {
        if let Err(e) =
            whisper_supervisor::run_dictation(dictation_sup_state, dictation_sup_signal).await
        {
            tracing::error!(error = %e, "dictation whisper supervisor terminated");
        }
    });

    // Retention loop — enforces the `[retention]` auto-delete policy, so this is
    // privacy-relevant: if it dies, recordings the user expected pruned live on
    // forever. Unlike the whisper supervisors it isn't in the crash-detect select
    // (a retention hiccup must not take the whole daemon down), but a plain
    // detached spawn would let a panic kill the loop silently for the rest of the
    // process. So wrap it: respawn on panic, exit cleanly on a normal return
    // (`retention::run` returns only on shutdown). The handle is kept so shutdown
    // awaits it, like the preview/dictation supervisors above.
    let retention_supervisor_state = state.clone();
    let retention_supervisor_handle = tokio::spawn(async move {
        loop {
            let run_state = retention_supervisor_state.clone();
            let run_shutdown = retention_supervisor_state.shutdown.signal.clone_receiver();
            match tokio::spawn(async move {
                retention::run(run_state, run_shutdown).await;
            })
            .await
            {
                // Normal return: `retention::run` only exits on shutdown, so stop.
                Ok(()) => return,
                // Panic (already logged by the panic hook) — respawn so the
                // auto-delete policy keeps running.
                Err(e) => {
                    tracing::error!(error = %e, "retention loop panicked; restarting");
                }
            }
        }
    });

    tracing::info!(
        audio_dir = %state.paths.audio_dir.display(),
        "phoneme-daemon ready"
    );

    // Run the IPC server inline against the shutdown signal. Also select on the
    // background worker handles so that if one panics or crashes, the entire
    // daemon process crashes rather than continuing as a zombie.
    let server_state = state.clone();
    let mut server_signal = state.shutdown.signal.clone();
    // If a crash branch fires, that `&mut handle` has been driven to Ready and its
    // output consumed — the cleanup below must NOT await it again (tokio panics on
    // a JoinHandle polled after completion). Track which handle the select ate.
    let mut worker_consumed = false;
    let mut supervisor_consumed = false;
    let server_result: Result<()> = tokio::select! {
        r = ipc_server::serve(server_state) => r,
        _ = server_signal.wait() => {
            tracing::info!("ipc server shutdown signaled");
            Ok(())
        }
        res = &mut worker_handle => {
            worker_consumed = true;
            tracing::error!("queue worker handle unexpectedly exited: {:?}", res);
            Err(anyhow::anyhow!("queue worker crashed"))
        }
        res = &mut supervisor_handle => {
            supervisor_consumed = true;
            tracing::error!("supervisor handle unexpectedly exited: {:?}", res);
            Err(anyhow::anyhow!("whisper supervisor crashed"))
        }
    };

    tracing::info!("shutting down");
    // Make sure background tasks see the shutdown even if we got here via
    // a server failure rather than the Ctrl+C handler or an IPC Shutdown.
    state.shutdown.trigger();

    // Finalize any in-flight recording first, through the normal stop paths,
    // so a quit mid-recording never leaves a corrupt WAV: the file is closed
    // properly and enqueued in the durable inbox — the next daemon run picks
    // it up and transcribes it. (The queue worker is already winding down, so
    // the item simply waits.) A NotRecording error here is the common case.
    if state.recorder.meeting_active().await {
        match state.recorder.stop_meeting(&state).await {
            Ok(meeting_id) => {
                tracing::info!(%meeting_id, "shutdown: finalized the in-flight meeting recording")
            }
            Err(e) => tracing::warn!(error = %e, "shutdown: could not stop the active meeting"),
        }
    } else if state.recorder.current().await.is_some() {
        match state.recorder.stop(&state).await {
            Ok(id) => tracing::info!(id = %id, "shutdown: finalized the in-flight recording"),
            Err(e) => tracing::warn!(error = %e, "shutdown: could not stop the active recording"),
        }
    }

    // Skip a handle the select already drove to completion (crash path) — re-await
    // would panic; on every graceful path both are still pending and awaited once.
    if !worker_consumed {
        let _ = worker_handle.await;
    }
    if !supervisor_consumed {
        let _ = supervisor_handle.await;
    }
    // Wait for the preview supervisor too, so its dedicated whisper-server (if
    // any) is killed before we exit — same cleanup guarantee as the main server.
    let _ = preview_supervisor_handle.await;
    // And the 2nd preview supervisor — its optional concurrent-"both" server (if
    // the user opted in) must be killed before exit too.
    let _ = preview2_supervisor_handle.await;
    // And the dictation supervisor — its optional third server (if the user
    // opted in) must be killed before exit too.
    let _ = dictation_supervisor_handle.await;
    // And the retention supervisor — its inner loop returns on shutdown; awaiting
    // it keeps the loop from being abandoned mid-tick as the process winds down.
    let _ = retention_supervisor_handle.await;
    // Persist the optional ANN index so incremental adds since the last build
    // survive a restart (a warm-start then reloads the sidecar instead of
    // rebuilding). A no-op unless the `ann-usearch` feature is compiled,
    // `semantic_search.ann.enabled` is on, and an index is warm; the sidecar is
    // disposable, so a save failure only means the next start rebuilds.
    state.catalog.save_ann_index().await;

    // Stop the Ollama this daemon launched, if any — a user-started one is
    // NotOurs and stays untouched (see `ollama_launcher`).
    state.ollama.shutdown().await;

    server_result
}

/// Read the daemon's config from disk. Pure: no migration, no persist, no
/// in-memory synthesis — just the parsed config.
///
/// Canonical reader shared with the CLI: honors `PHONEME_CONFIG`, else the
/// per-user default path, else built-in defaults. Callers that want the daemon's
/// runtime config follow this with [`reconcile_and_persist_config`] (the one-time
/// migrations and the disk write they imply) then [`apply_runtime_defaults`]
/// (in-memory-only derivations). Keeping those steps explicit means a "load"
/// never silently writes to disk.
pub fn load_config() -> anyhow::Result<phoneme_core::Config> {
    Ok(phoneme_core::Config::load_resolved()?)
}

/// Run the one-time config migrations on `cfg` in place and, if either actually
/// changed something, persist the result so the on-disk config freezes in its
/// migrated form. This is the explicit disk side-effect, kept out of
/// `load_config`. Idempotent and self-healing: safe to call on every load path
/// (startup, `ReloadConfig`, the queue worker's post-run reload), and a no-op
/// once the file is already migrated. A persist failure is non-fatal — the
/// in-memory migration still applies and the next load retries.
///
/// Runs both migrations with a non-short-circuiting `|` so both always execute,
/// playbook first: it rebuilds the `default` recipe's step list from the legacy
/// enable flags, then `migrate_hooks` appends the migrated Hook steps.
pub fn reconcile_and_persist_config(cfg: &mut phoneme_core::Config) {
    if cfg.migrate_playbook() | cfg.migrate_hooks() {
        if let Err(e) = cfg.write_resolved() {
            tracing::warn!(error = %e, "failed to persist config migration; will retry next reload");
        }
    }
}

/// Apply in-memory-only runtime defaults that must never be written back to
/// `config.toml`. Currently: auto-default the live preview to the smallest local
/// whisper model when `[preview_whisper]` is unset and the main provider is a
/// local bundled model (P1). Call this after [`reconcile_and_persist_config`] so
/// the synthesized block is never caught by that persist. The model scan needs a
/// real absolute file, so it derives against an expanded view and copies the
/// absolute-path block onto `cfg` (`expanded()` later no-ops on an already-
/// absolute path). A no-op when the user set `preview_whisper`, the main is
/// cloud/external, or no smaller local model exists.
pub fn apply_runtime_defaults(cfg: &mut phoneme_core::Config) {
    if cfg.preview_whisper.is_none() {
        match cfg.expanded() {
            Ok(mut expanded) => {
                if expanded.materialize_auto_preview() {
                    if let Some(pv) = expanded.preview_whisper {
                        tracing::info!(
                            model = %pv.model_path,
                            port = pv.bundled_server_port,
                            "live preview auto-defaulted to the smallest local whisper model"
                        );
                        cfg.preview_whisper = Some(pv);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not expand config to auto-default the preview model; preview will reuse the main provider");
            }
        }
    }
}
