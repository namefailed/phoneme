//! IPC server — bind a NamedPipeListener and spawn handlers.

use crate::app_state::AppState;
use crate::ipc_handler::handle_connection;
use phoneme_ipc::NamedPipeListener;
use std::time::Duration;

/// Starting delay after the first accept failure.
const BACKOFF_INITIAL: Duration = Duration::from_millis(50);
/// Maximum delay between accept retries. Keeps the server responsive while
/// still backing off from a sustained transient error (e.g. a momentary
/// handle exhaustion burst).
const BACKOFF_MAX: Duration = Duration::from_secs(4);

pub async fn serve(state: AppState) -> anyhow::Result<()> {
    let pipe_name = state.config.load().daemon.pipe_name.clone();
    let mut listener = NamedPipeListener::bind(&pipe_name).map_err(|e| match e {
        phoneme_ipc::IpcTransportError::AlreadyInUse => anyhow::anyhow!(
            "another phoneme-daemon is already running. Stop it with `phoneme daemon --stop`."
        ),
        other => anyhow::anyhow!("bind named pipe '{pipe_name}': {other}"),
    })?;
    tracing::info!(
        pipe = %pipe_name,
        pid = std::process::id(),
        "IPC server listening — phoneme-daemon ready"
    );

    let mut backoff = BACKOFF_INITIAL;

    loop {
        let conn = match listener.accept().await {
            Ok(c) => {
                // Reset backoff on every successful accept so a burst of errors
                // doesn't permanently inflate the delay.
                backoff = BACKOFF_INITIAL;
                c
            }
            Err(e) => {
                tracing::warn!(error = %e, ?backoff, "accept failed; retrying after backoff");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            handle_connection(conn, state).await;
        });
    }
}
