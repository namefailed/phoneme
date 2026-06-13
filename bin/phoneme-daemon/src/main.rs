//! phoneme-daemon — the headless brain.

use anyhow::Result;
use clap::Parser;

mod app_state;
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
    let cfg = load_config()?;
    let state = AppState::new(cfg).await?;
    let _guard = logging::init(&state.config.load(), &state.paths.log_dir, args.foreground)?;

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

    reconcile::run(&state).await?;

    // Background task to (re-)embed recordings that lack per-chunk embeddings.
    // This doubles as the migration from the legacy whole-recording `embeddings`
    // table to sentence-aware chunk vectors: any recording with a transcript but
    // no chunk rows — including ones that only have an old whole-recording vector
    // — is backfilled so paraphrase recall improves across the existing library
    // without the user re-recording or re-transcribing anything.
    let retroactive_state = state.clone();
    tokio::spawn(async move {
        // No embedder loaded → nothing to backfill (semantic search off or the
        // model failed to load); same silent no-op as before.
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
                    // Re-acquire the embedder PER ITEM rather than holding the
                    // read guard across the whole loop: a large-library backfill
                    // runs for minutes, and config reloads need the write lock —
                    // clone the Arc and drop the guard so writers interleave
                    // between items. If the embedder is gone mid-backfill (the
                    // user turned semantic search off), stop — the same
                    // exit-when-unloaded behavior the up-front check gave.
                    let embedder = retroactive_state.embedder.read().await.as_ref().cloned();
                    let Some(embedder) = embedder else {
                        tracing::info!("backfill stopped: embedding model unloaded");
                        return;
                    };
                    pipeline::embed_and_store(
                        &embedder,
                        &retroactive_state.catalog,
                        &r.id,
                        transcript,
                    )
                    .await;
                }
                tracing::info!("Finished backfilling chunk embeddings.");
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
    // Keep the handle so shutdown can AWAIT it — otherwise the process could exit
    // before run_preview kills its child, orphaning the 2nd whisper-server. Not
    // in the crash-detection select below: a preview-server crash must not take
    // down the daemon (preview is non-critical).
    let preview_supervisor_handle = tokio::spawn(async move {
        if let Err(e) = whisper_supervisor::run_preview(preview_sup_state, preview_sup_signal).await
        {
            tracing::error!(error = %e, "preview whisper supervisor terminated");
        }
    });

    let retention_state = state.clone();
    let retention_shutdown = state.shutdown.signal.clone_receiver();
    tokio::spawn(async move {
        retention::run(retention_state, retention_shutdown).await;
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
    let server_result: Result<()> = tokio::select! {
        r = ipc_server::serve(server_state) => r,
        _ = server_signal.wait() => {
            tracing::info!("ipc server shutdown signaled");
            Ok(())
        }
        res = &mut worker_handle => {
            tracing::error!("queue worker handle unexpectedly exited: {:?}", res);
            Err(anyhow::anyhow!("queue worker crashed"))
        }
        res = &mut supervisor_handle => {
            tracing::error!("supervisor handle unexpectedly exited: {:?}", res);
            Err(anyhow::anyhow!("whisper supervisor crashed"))
        }
    };

    tracing::info!("shutting down");
    // Make sure background tasks see the shutdown even if we got here via
    // a server failure rather than the Ctrl+C handler or an IPC Shutdown.
    state.shutdown.trigger();

    // Finalize any in-flight recording FIRST, through the normal stop paths,
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

    let _ = worker_handle.await;
    let _ = supervisor_handle.await;
    // Wait for the preview supervisor too, so its dedicated whisper-server (if
    // any) is killed before we exit — same cleanup guarantee as the main server.
    let _ = preview_supervisor_handle.await;
    // Stop the Ollama this daemon launched, if any — a user-started one is
    // NotOurs and stays untouched (see `ollama_launcher`).
    state.ollama.shutdown().await;

    server_result
}

pub fn load_config() -> anyhow::Result<phoneme_core::Config> {
    // Canonical loader shared with the CLI: honors PHONEME_CONFIG, else the
    // per-user default, else built-in defaults.
    Ok(phoneme_core::Config::load_resolved()?)
}
