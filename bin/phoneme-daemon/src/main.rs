//! phoneme-daemon — the headless brain.

use anyhow::Result;
use clap::Parser;

mod app_state;
mod event_bus;
mod first_run;
mod ipc_handler;
mod ipc_server;
mod logging;
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

    // Background task to retroactively embed recordings that lack embeddings
    let retroactive_state = state.clone();
    tokio::spawn(async move {
        let embedder_guard = retroactive_state.embedder.read().await;
        if let Some(embedder) = embedder_guard.as_ref() {
            if let Ok(records) = retroactive_state
                .catalog
                .list_recordings_without_embeddings()
                .await
            {
                if !records.is_empty() {
                    tracing::info!(
                        "Found {} recordings without semantic embeddings, generating...",
                        records.len()
                    );
                    for r in records {
                        if let Some(transcript) = r.transcript.as_ref() {
                            if let Ok(vec) = embedder.embed(transcript) {
                                let _ = retroactive_state
                                    .catalog
                                    .upsert_embedding(&r.id, &vec)
                                    .await;
                            }
                        }
                    }
                    tracing::info!("Finished generating retroactive semantic embeddings.");
                }
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
    let _ = worker_handle.await;
    let _ = supervisor_handle.await;
    // Wait for the preview supervisor too, so its dedicated whisper-server (if
    // any) is killed before we exit — same cleanup guarantee as the main server.
    let _ = preview_supervisor_handle.await;

    server_result
}

pub fn load_config() -> anyhow::Result<phoneme_core::Config> {
    // Canonical loader shared with the CLI: honors PHONEME_CONFIG, else the
    // per-user default, else built-in defaults.
    Ok(phoneme_core::Config::load_resolved()?)
}
