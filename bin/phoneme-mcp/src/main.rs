//! phoneme-mcp — a thin Model Context Protocol bridge over the Phoneme daemon.
//!
//! MCP is JSON-RPC 2.0 over stdio. This binary is a *translator*, not a brain
//! (per the roadmap): it reads framed JSON-RPC requests from **stdin**, maps
//! the exposed tools onto `phoneme-ipc` [`Request`](phoneme_ipc::Request)s
//! over the existing daemon `Transport`, and writes JSON-RPC responses to
//! **stdout**. All logging goes to **stderr** — stdout is the protocol channel
//! and must never carry anything but framed JSON-RPC.
//!
//! Layout:
//! - [`protocol`] — JSON-RPC envelopes + stdio framing (newline or
//!   `Content-Length`), all pure / unit-tested.
//! - [`tools`] — the tools, their JSON schemas, the pure
//!   `build_request` dispatch, and result rendering.
//! - [`server`] — the method dispatcher (`initialize`, `tools/list`,
//!   `tools/call`, notifications) over a [`server::DaemonCall`] backend.
//!
//! The daemon connection is **observe-only**: a tool call dials the existing
//! pipe and reports a clean error if no daemon is running — it never spawns one
//! (see [`server`] for why an externally-launched bridge shouldn't).
//!
//! ## Run it
//!
//! Point an MCP client at the built binary (`phoneme-mcp`); see
//! `docs/developer-guide/mcp_server.md` for a copy-paste client config. Smoke
//! test by hand:
//!
//! ```text
//! echo '{"jsonrpc":"2.0","id":1,"method":"initialize"}' | phoneme-mcp
//! ```

mod protocol;
mod server;
mod tools;

use protocol::{FramedReader, JsonRpcError, PARSE_ERROR};
use serde_json::Value;
use server::{Handled, PipeDaemon, Server};
use tokio::io::{AsyncWriteExt, BufReader, BufWriter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs to STDERR only — stdout is the JSON-RPC channel.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load the same config the daemon/CLI read so we dial the right pipe.
    let cfg = match phoneme_core::Config::load_resolved() {
        Ok(c) => c,
        Err(e) => {
            // A missing/invalid config is recoverable for the protocol layer —
            // fall back to defaults so initialize/tools-list still work; tool
            // calls will then fail cleanly if the (default) pipe has no daemon.
            tracing::warn!("config load failed ({e}); using defaults");
            phoneme_core::Config::default()
        }
    };

    let server = Server::new(PipeDaemon::new(&cfg), env!("CARGO_PKG_VERSION"));
    tracing::info!("phoneme-mcp {} ready on stdio", env!("CARGO_PKG_VERSION"));

    run_loop(
        server,
        BufReader::new(tokio::io::stdin()),
        BufWriter::new(tokio::io::stdout()),
    )
    .await
}

/// The stdio event loop: read framed messages, dispatch, write framed
/// responses. Returns on clean EOF (the client closed stdin).
///
/// Generic over the reader/writer and the daemon backend so an integration
/// test can drive it over in-memory pipes with a mock daemon.
async fn run_loop<R, W, D>(server: Server<D>, reader: R, mut writer: W) -> anyhow::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
    D: server::DaemonCall,
{
    let mut framed = FramedReader::new(reader);

    while let Some(message) = framed.next_message().await? {
        let value = match message {
            Ok(v) => v,
            Err(parse_err) => {
                // Malformed JSON: answer with a JSON-RPC parse error (null id)
                // and keep serving rather than tearing the connection down.
                let err = JsonRpcError::new(Value::Null, PARSE_ERROR, parse_err);
                write_message(&mut writer, &err).await?;
                continue;
            }
        };

        // Extract id/method/params leniently — a request missing `method` is a
        // protocol violation we answer with a parse-ish error.
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            let err = JsonRpcError::new(id, PARSE_ERROR, "request missing 'method'");
            write_message(&mut writer, &err).await?;
            continue;
        };
        let params = value.get("params").cloned().unwrap_or(Value::Null);

        match server.handle(method, &params, id).await {
            Handled::Response(resp) => write_message(&mut writer, &resp).await?,
            Handled::Error(err) => write_message(&mut writer, &err).await?,
            Handled::None => {} // notification: no reply
        }
    }

    Ok(())
}

/// Serialize a JSON-RPC message and write it as one newline-framed line to the
/// protocol channel, flushing so the client sees it immediately.
async fn write_message<W>(writer: &mut W, message: &impl serde::Serialize) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let line = protocol::to_line(message)?;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoneme_ipc::{Request, Response};
    use serde_json::json;
    use std::io::Cursor;

    /// A trivial in-process daemon for the end-to-end loop test.
    struct OkDaemon;

    #[async_trait::async_trait]
    impl server::DaemonCall for OkDaemon {
        async fn call(&self, _req: Request) -> Result<Response, String> {
            Ok(Response::Ok(json!([])))
        }
    }

    /// Drive the whole loop over an in-memory stdin/stdout: an `initialize`
    /// followed by `tools/list`, then EOF. Both must yield valid JSON-RPC
    /// responses, in order, framed newline-delimited.
    #[tokio::test]
    async fn loop_answers_initialize_then_tools_list() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let reader = BufReader::new(Cursor::new(input.as_bytes().to_vec()));
        let mut output: Vec<u8> = Vec::new();

        let server = Server::new(OkDaemon, "0.0.0");
        run_loop(server, reader, Cursor::new(&mut output))
            .await
            .unwrap();

        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // The notification produced no reply → exactly two response lines.
        assert_eq!(lines.len(), 2, "expected 2 responses, got: {text}");

        let init: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["serverInfo"]["name"], server::SERVER_NAME);

        let list: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(list["id"], 2);
        // Derive the count from the shared registry — the single source of truth
        // for the tool catalog — so this stays correct as tools are added.
        let expected = phoneme_agent_core::ToolRegistry::with_phoneme_tools()
            .specs()
            .len();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), expected);
        // The framed payload carries real catalog content, not just a count: each
        // entry has a string name + object inputSchema, and a known tool is there.
        let names: Vec<&str> = tools
            .iter()
            .map(|t| {
                assert!(
                    t["inputSchema"].is_object(),
                    "each tool needs an inputSchema object, got: {t}"
                );
                t["name"].as_str().expect("each tool needs a string name")
            })
            .collect();
        assert!(
            names.contains(&"start_recording"),
            "loop tools/list payload missing start_recording: {names:?}"
        );
    }

    /// A malformed JSON line is answered with a parse error and the loop keeps
    /// serving the next (valid) message.
    #[tokio::test]
    async fn loop_recovers_from_malformed_line() {
        let input = concat!(
            "{ this is not json }\n",
            "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/list\"}\n",
        );
        let reader = BufReader::new(Cursor::new(input.as_bytes().to_vec()));
        let mut output: Vec<u8> = Vec::new();
        run_loop(
            Server::new(OkDaemon, "0.0.0"),
            reader,
            Cursor::new(&mut output),
        )
        .await
        .unwrap();

        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let err: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(err["error"]["code"], PARSE_ERROR);
        let ok: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ok["id"], 5);
    }
}
