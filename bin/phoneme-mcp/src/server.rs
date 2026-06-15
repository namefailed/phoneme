//! The MCP method dispatcher: maps JSON-RPC methods onto handlers and
//! tool calls onto daemon requests.
//!
//! [`Server`] is generic over a [`DaemonCall`] — the one thing a tool call
//! needs from the outside world: "send this [`Request`], give me the
//! [`Response`]". The binary wires in [`PipeDaemon`] (a fresh
//! `NamedPipeTransport` connection per call); tests wire in a closure mock.
//! That keeps every method handler — including the full `tools/call` →
//! `Request` → MCP-content path — unit-testable without a live daemon.
//!
//! ## Connect strategy: observe, never spawn
//!
//! `bin/phoneme` splits its connect into a spawning path (commands that create
//! work) and an observe-only path (inspection). An MCP server is launched by an
//! external agent host (Claude Desktop, etc.) that has no relationship to the
//! Phoneme daemon's lifecycle, so silently starting a long-lived background
//! daemon from a tool call would be surprising and would outlive the agent.
//! Instead every tool — including `start_recording` — *observes*: it dials the
//! existing pipe and, if no daemon is listening, returns a clean tool error
//! ("Phoneme daemon is not running — start it with `phoneme daemon start` or
//! the tray app") rather than spawning one. The user keeps explicit control of
//! the daemon; the bridge only ever talks to a daemon they already chose to run.

use crate::protocol::{JsonRpcError, JsonRpcResponse, INVALID_PARAMS, METHOD_NOT_FOUND};
use crate::tools;
use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use serde_json::{json, Value};

/// The MCP protocol version this server advertises (matches the version most
/// current MCP clients negotiate; the client echoes its own in `initialize`).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The server name reported in `initialize`'s `serverInfo`.
pub const SERVER_NAME: &str = "phoneme-mcp";

/// One round-trip to the daemon: send a [`Request`], get a [`Response`].
///
/// Abstracted so the server logic is testable without a live pipe. The real
/// implementation ([`PipeDaemon`]) opens a fresh connection per call.
#[async_trait::async_trait]
pub trait DaemonCall: Send + Sync {
    /// Send one request and await its response. `Err` is a transport/connect
    /// failure (rendered to the caller as "daemon not reachable").
    async fn call(&self, req: Request) -> Result<Response, String>;
}

/// Production [`DaemonCall`]: observe-only, one short-lived named-pipe
/// connection per request. Never spawns a daemon (see the module docs).
pub struct PipeDaemon {
    pipe_name: String,
}

impl PipeDaemon {
    /// Resolve the pipe name from the loaded config and build the caller.
    pub fn new(cfg: &Config) -> Self {
        Self {
            pipe_name: cfg.daemon.pipe_name.clone(),
        }
    }
}

#[async_trait::async_trait]
impl DaemonCall for PipeDaemon {
    async fn call(&self, req: Request) -> Result<Response, String> {
        let mut transport = NamedPipeTransport::connect(&self.pipe_name)
            .await
            .map_err(|e| {
                format!(
                    "Phoneme daemon is not running ({e}). Start it with \
                     `phoneme daemon start` or the Phoneme tray app, then retry."
                )
            })?;
        transport
            .request(req)
            .await
            .map_err(|e| format!("daemon transport error: {e}"))
    }
}

/// What a handled JSON-RPC message produces.
pub enum Handled {
    /// A success response to write to stdout.
    Response(JsonRpcResponse),
    /// An error response to write to stdout.
    Error(JsonRpcError),
    /// Nothing to write (a notification was handled).
    None,
}

/// The MCP server: dispatches methods against a [`DaemonCall`] backend.
pub struct Server<D: DaemonCall> {
    daemon: D,
    version: String,
}

impl<D: DaemonCall> Server<D> {
    /// Build a server over the given daemon backend. `version` is reported in
    /// `serverInfo` (the binary passes its crate version).
    pub fn new(daemon: D, version: impl Into<String>) -> Self {
        Self {
            daemon,
            version: version.into(),
        }
    }

    /// Handle one decoded JSON-RPC request and produce what to write back.
    ///
    /// `id` is the raw request id (`null` for notifications). Unknown methods
    /// yield a `-32601` error; notifications yield [`Handled::None`].
    pub async fn handle(&self, method: &str, params: &Value, id: Value) -> Handled {
        match method {
            "initialize" => Handled::Response(JsonRpcResponse::ok(id, self.initialize_result())),
            "notifications/initialized" | "initialized" => Handled::None,
            "ping" => Handled::Response(JsonRpcResponse::ok(id, json!({}))),
            "tools/list" => Handled::Response(JsonRpcResponse::ok(id, tools::tools_list())),
            "tools/call" => self.tools_call(params, id).await,
            other => Handled::Error(JsonRpcError::new(
                id,
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    /// The `initialize` result: protocol version, capabilities, serverInfo.
    fn initialize_result(&self) -> Value {
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": self.version,
            }
        })
    }

    /// Handle `tools/call`: validate name+arguments into a [`Request`], call the
    /// daemon, and render the result as MCP text content. Tool-level failures
    /// (bad args, unreachable daemon, daemon `Err`) come back as a *successful*
    /// JSON-RPC response whose `result` has `isError: true` — that is the MCP
    /// contract, so the agent sees the message instead of a transport fault.
    async fn tools_call(&self, params: &Value, id: Value) -> Handled {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Handled::Error(JsonRpcError::new(
                id,
                INVALID_PARAMS,
                "tools/call requires a 'name'",
            ));
        };
        let empty = json!({});
        let arguments = params.get("arguments").unwrap_or(&empty);

        // Build the request (pure validation). A bad tool/args is a tool error.
        let req = match tools::build_request(name, arguments) {
            Ok(req) => req,
            Err(e) => return Handled::Response(tool_error_result(id, e.to_string())),
        };

        // Talk to the daemon. Connect failure → tool error, not JSON-RPC error.
        match self.daemon.call(req).await {
            Ok(Response::Ok(value)) => {
                let text = tools::render_result(name, &value);
                Handled::Response(JsonRpcResponse::ok(id, tool_text_result(&text, false)))
            }
            Ok(Response::Err(e)) => Handled::Response(tool_error_result(
                id,
                format!("daemon error ({:?}): {}", e.kind, e.message),
            )),
            Err(e) => Handled::Response(tool_error_result(id, e)),
        }
    }
}

/// Build the MCP `CallToolResult` body: a single text content block.
fn tool_text_result(text: &str, is_error: bool) -> Value {
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error,
    })
}

/// A `tools/call` result flagged `isError: true` carrying `message`.
fn tool_error_result(id: Value, message: String) -> JsonRpcResponse {
    JsonRpcResponse::ok(id, tool_text_result(&message, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoneme_ipc::{IpcError, IpcErrorKind};
    use std::sync::{Arc, Mutex};

    /// A mock [`DaemonCall`] that records the requests it sees and replies with
    /// a fixed function — no pipe, no daemon.
    struct MockDaemon<F: Fn(&Request) -> Result<Response, String> + Send + Sync> {
        seen: Arc<Mutex<Vec<Request>>>,
        responder: F,
    }

    #[async_trait::async_trait]
    impl<F> DaemonCall for MockDaemon<F>
    where
        F: Fn(&Request) -> Result<Response, String> + Send + Sync,
    {
        async fn call(&self, req: Request) -> Result<Response, String> {
            self.seen.lock().unwrap().push(req.clone());
            (self.responder)(&req)
        }
    }

    fn server_with<F>(responder: F) -> (Server<MockDaemon<F>>, Arc<Mutex<Vec<Request>>>)
    where
        F: Fn(&Request) -> Result<Response, String> + Send + Sync,
    {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let daemon = MockDaemon {
            seen: seen.clone(),
            responder,
        };
        (Server::new(daemon, "9.9.9"), seen)
    }

    fn ok_null() -> Result<Response, String> {
        Ok(Response::Ok(Value::Null))
    }

    fn unwrap_response(h: Handled) -> JsonRpcResponse {
        match h {
            Handled::Response(r) => r,
            Handled::Error(e) => panic!("expected Response, got Error: {:?}", e.error),
            Handled::None => panic!("expected Response, got None"),
        }
    }

    #[tokio::test]
    async fn initialize_returns_expected_shape() {
        let (srv, _) = server_with(|_| ok_null());
        let r = unwrap_response(srv.handle("initialize", &json!({}), json!(1)).await);
        assert_eq!(r.id, json!(1));
        assert_eq!(r.result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(r.result["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(r.result["serverInfo"]["version"], "9.9.9");
        assert!(r.result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_returns_all_tools() {
        let (srv, _) = server_with(|_| ok_null());
        let r = unwrap_response(srv.handle("tools/list", &json!({}), json!(2)).await);
        assert_eq!(r.result["tools"].as_array().unwrap().len(), 14);
    }

    #[tokio::test]
    async fn notifications_initialized_is_a_noop() {
        let (srv, _) = server_with(|_| ok_null());
        assert!(matches!(
            srv.handle("notifications/initialized", &json!({}), Value::Null)
                .await,
            Handled::None
        ));
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let (srv, _) = server_with(|_| ok_null());
        let h = srv.handle("does/not/exist", &json!({}), json!(7)).await;
        match h {
            Handled::Error(e) => {
                assert_eq!(e.error.code, METHOD_NOT_FOUND);
                assert_eq!(e.id, json!(7));
            }
            _ => panic!("expected an error"),
        }
    }

    #[tokio::test]
    async fn tools_call_dispatches_to_record_start() {
        let (srv, seen) = server_with(|_| Ok(Response::Ok(json!({"id": "rec-1"}))));
        let params = json!({"name": "start_recording", "arguments": {"mode": "hold"}});
        let r = unwrap_response(srv.handle("tools/call", &params, json!(3)).await);
        assert_eq!(r.result["isError"], false);
        assert!(r.result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("rec-1"));
        // The exact request reached the daemon.
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            &[Request::RecordStart {
                mode: phoneme_core::RecordMode::Hold,
                in_place: false
            }]
        );
    }

    #[tokio::test]
    async fn tools_call_dispatches_set_title_with_some() {
        let (srv, seen) = server_with(|_| ok_null());
        let id = phoneme_core::RecordingId::new();
        let params = json!({
            "name": "set_title",
            "arguments": { "id": id.as_str(), "title": "Budget call" }
        });
        let r = unwrap_response(srv.handle("tools/call", &params, json!(10)).await);
        assert_eq!(r.result["isError"], false);
        assert!(r.result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Title updated"));
        // The exact request reached the daemon.
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            &[Request::SetRecordingTitle {
                id,
                title: Some("Budget call".to_string())
            }]
        );
    }

    #[tokio::test]
    async fn tools_call_retranscribe_passes_model_override() {
        let (srv, seen) = server_with(|_| ok_null());
        let id = phoneme_core::RecordingId::new();
        let params = json!({
            "name": "retranscribe",
            "arguments": { "id": id.as_str(), "model": "large-v3" }
        });
        let r = unwrap_response(srv.handle("tools/call", &params, json!(11)).await);
        assert_eq!(r.result["isError"], false);
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            &[Request::RetranscribeRecording {
                id,
                model: Some("large-v3".to_string()),
                run_hooks: None,
                post_process: None,
                all_overrides: None,
            }]
        );
    }

    #[tokio::test]
    async fn tools_call_invalid_id_is_tool_error_not_daemon_call() {
        let (srv, seen) = server_with(|_| ok_null());
        let params = json!({ "name": "get_words", "arguments": { "id": "not-an-id" } });
        let r = unwrap_response(srv.handle("tools/call", &params, json!(12)).await);
        // A bad id is a tool error, surfaced as isError, never reaching the daemon.
        assert_eq!(r.result["isError"], true);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tools_call_bad_args_is_tool_error_not_jsonrpc_error() {
        let (srv, seen) = server_with(|_| ok_null());
        let params = json!({"name": "search_recordings", "arguments": {}});
        let r = unwrap_response(srv.handle("tools/call", &params, json!(4)).await);
        // isError true, but still a normal JSON-RPC *response*.
        assert_eq!(r.result["isError"], true);
        // Never reached the daemon.
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tools_call_unreachable_daemon_is_clean_tool_error() {
        let (srv, _) = server_with(|_| Err("daemon down".to_string()));
        let params = json!({"name": "stop_recording", "arguments": {}});
        let r = unwrap_response(srv.handle("tools/call", &params, json!(5)).await);
        assert_eq!(r.result["isError"], true);
        assert!(r.result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("daemon down"));
    }

    #[tokio::test]
    async fn tools_call_daemon_err_response_becomes_tool_error() {
        let (srv, _) = server_with(|_| {
            Ok(Response::Err(IpcError {
                kind: IpcErrorKind::NotRecording,
                message: "no active recording".to_string(),
            }))
        });
        let params = json!({"name": "stop_recording", "arguments": {}});
        let r = unwrap_response(srv.handle("tools/call", &params, json!(6)).await);
        assert_eq!(r.result["isError"], true);
        let text = r.result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("no active recording"), "got: {text}");
    }

    #[tokio::test]
    async fn tools_call_missing_name_is_invalid_params() {
        let (srv, _) = server_with(|_| ok_null());
        let h = srv.handle("tools/call", &json!({}), json!(8)).await;
        match h {
            Handled::Error(e) => assert_eq!(e.error.code, INVALID_PARAMS),
            _ => panic!("expected invalid params error"),
        }
    }
}
