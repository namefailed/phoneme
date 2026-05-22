//! IPC server — bind a NamedPipeListener and spawn handlers.

use crate::app_state::AppState;
use crate::ipc_handler::handle_connection;
use phoneme_ipc::NamedPipeListener;

pub async fn serve(state: AppState) -> anyhow::Result<()> {
    let pipe_name = state.config.load().daemon.pipe_name.clone();
    let mut listener = NamedPipeListener::bind(&pipe_name).map_err(|e| match e {
        phoneme_ipc::IpcTransportError::AlreadyInUse => anyhow::anyhow!(
            "another phoneme-daemon is already running. Stop it with `phoneme daemon --stop`."
        ),
        other => anyhow::anyhow!("bind named pipe '{pipe_name}': {other}"),
    })?;
    tracing::info!(pipe = %pipe_name, "IPC server listening");

    loop {
        let conn = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            handle_connection(conn, state).await;
        });
    }
}
