//! JSON-RPC 2.0 wire types and stdio framing for the MCP bridge.
//!
//! MCP rides JSON-RPC 2.0; this module owns the *transport* half of that — the
//! request/response/error envelopes and the two framings an MCP client may use
//! over stdio:
//!
//! - **newline-delimited** — one JSON object per line (`\n`), the simplest
//!   framing and what most MCP launchers (and our smoke test) speak;
//! - **`Content-Length`** — an LSP-style header block
//!   (`Content-Length: <n>\r\n\r\n<n bytes of JSON>`), which some clients use so
//!   a JSON object may contain raw newlines.
//!
//! [`FramedReader`] accepts either on the same stream by peeking the first
//! non-blank bytes: a `Content-Length:` header switches it into header mode for
//! that one message, otherwise the line is treated as a whole JSON value. All
//! framing helpers are deliberately free functions / small types with no daemon
//! dependency so they round-trip in unit tests without any I/O.
//!
//! Everything here writes to the caller's chosen sink; the binary points the
//! responses at **stdout** and all logging at **stderr** — stdout is the
//! protocol channel and must never carry anything but framed JSON-RPC.

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

/// The JSON-RPC version string every message carries.
pub const JSONRPC_VERSION: &str = "2.0";

// ── Standard JSON-RPC error codes (subset we emit) ──────────────────────────

/// The method does not exist / is not available.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Invalid JSON was received (parse error).
pub const PARSE_ERROR: i64 = -32700;
/// Invalid method parameters.
pub const INVALID_PARAMS: i64 = -32602;

/// A JSON-RPC success response: `{jsonrpc, id, result}`.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Echoes the request id.
    pub id: Value,
    /// The method's result payload.
    pub result: Value,
}

impl JsonRpcResponse {
    /// Build a success response echoing `id`.
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result,
        }
    }
}

/// A JSON-RPC error response: `{jsonrpc, id, error:{code,message}}`.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Echoes the request id (`null` when the id couldn't be parsed).
    pub id: Value,
    /// The error body.
    pub error: ErrorBody,
}

/// The `error` member of a JSON-RPC error response.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    /// A JSON-RPC error code (see the `*_ERROR` / `*_NOT_FOUND` constants).
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

impl JsonRpcError {
    /// Build an error response echoing `id`.
    pub fn new(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            error: ErrorBody {
                code,
                message: message.into(),
            },
        }
    }
}

/// Serialize a value to a single newline-framed JSON line (no trailing
/// newline added here — the writer appends it).
///
/// Kept separate from the I/O so tests can assert the exact bytes.
pub fn to_line(value: &impl Serialize) -> serde_json::Result<String> {
    serde_json::to_string(value)
}

/// Upper bound on a single framed message body / line, mirroring the daemon's
/// IPC frame cap (`phoneme_ipc`'s `MAX_FRAME_BYTES`, 8 MiB). A malicious or
/// buggy MCP client must never be able to make the bridge allocate without
/// limit: an oversize `Content-Length`, or a line/header stream that never
/// sends a newline, would otherwise grow memory until the process aborts or is
/// OOM-killed.
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Cap on the number of header lines consumed after a `Content-Length` header,
/// guarding against a peer that streams headers without a blank terminator.
const MAX_HEADER_LINES: usize = 64;

/// Read one line into `line`. If the line grows past [`MAX_FRAME_BYTES`]
/// without a terminating newline the reader drains until `\n` (or EOF) to
/// restore alignment, then returns a parse-level `Err` so the *session*
/// survives and the caller can answer with a JSON-RPC parse error rather than
/// tearing the loop down. Returns the byte count read (`0` = clean EOF),
/// matching [`AsyncBufReadExt::read_line`].
async fn read_capped_line<R: AsyncBufRead + Unpin>(
    inner: &mut R,
    line: &mut String,
) -> std::io::Result<usize> {
    let n = inner.take(MAX_FRAME_BYTES as u64).read_line(line).await?;
    if n == MAX_FRAME_BYTES && !line.ends_with('\n') {
        // The cap was hit before a newline arrived. Drain the remainder of this
        // oversized line so the next read_line starts on a clean message
        // boundary, then surface the problem as a parse error rather than an
        // io::Error (which would kill the session).
        drain_to_newline(inner).await?;
        // Return a sentinel: 1 byte so callers know "not EOF", but `line` will
        // be missing its newline — the caller's parse step will reject the
        // truncated content as invalid JSON, producing a JSON-RPC parse error.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("MCP line exceeds {MAX_FRAME_BYTES} bytes; line drained for realignment"),
        ));
    }
    Ok(n)
}

/// Discard bytes from `inner` until a `\n` or EOF — used after hitting the
/// per-line cap to realign the stream to the next message boundary.
async fn drain_to_newline<R: AsyncBufRead + Unpin>(inner: &mut R) -> std::io::Result<()> {
    loop {
        let buf = inner.fill_buf().await?;
        if buf.is_empty() {
            return Ok(()); // EOF
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            // Consume up to and including the newline.
            let consume = pos + 1;
            inner.consume(consume);
            return Ok(());
        }
        let len = buf.len();
        inner.consume(len);
    }
}

/// Reads JSON-RPC messages off an async buffered stream, accepting either
/// newline-delimited or `Content-Length`-framed messages on the same channel.
pub struct FramedReader<R> {
    inner: R,
}

impl<R: AsyncBufRead + Unpin> FramedReader<R> {
    /// Wrap a buffered reader (e.g. `BufReader<Stdin>`).
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Read the next framed message and parse it into a raw [`Value`].
    ///
    /// Returns `Ok(None)` at clean end-of-stream. A line that is valid framing
    /// but invalid JSON returns the parse error so the caller can answer with a
    /// JSON-RPC parse error rather than tearing the loop down.
    pub async fn next_message(&mut self) -> std::io::Result<Option<Result<Value, String>>> {
        // Skip blank separator lines, then branch on whether the first
        // content line is a Content-Length header or a bare JSON line.
        loop {
            let mut line = String::new();
            let n = match read_capped_line(&mut self.inner, &mut line).await {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    // Oversize line: the stream has been drained to the next
                    // newline boundary; surface this as a parse error so the
                    // loop keeps serving rather than killing the session.
                    return Ok(Some(Err(e.to_string())));
                }
                Err(e) => return Err(e),
            };
            if n == 0 {
                return Ok(None); // EOF
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                continue; // blank line between messages
            }

            if let Some(len) = parse_content_length(trimmed) {
                // LSP-style header block: consume any further headers up to the
                // blank line, then read exactly `len` bytes of JSON body.
                self.consume_headers().await?;
                // Cap the body to the IPC frame limit: an oversize Content-Length
                // must not trigger an unbounded eager allocation. Unlike the
                // newline-framed oversize case (where we can drain to the next
                // `\n`), a Content-Length body could be arbitrarily large and
                // draining it is unbounded — so we close the stream here. This
                // is a hard protocol violation from the client; the session is
                // not recoverable.
                if len > MAX_FRAME_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Content-Length {len} exceeds maximum {MAX_FRAME_BYTES} bytes; \
                             stream closed to avoid unbounded drain"
                        ),
                    ));
                }
                let mut buf = vec![0u8; len];
                self.inner.read_exact(&mut buf).await?;
                let parsed = serde_json::from_slice::<Value>(&buf).map_err(|e| e.to_string());
                return Ok(Some(parsed));
            }

            // Newline-delimited: this whole line is one JSON value.
            let parsed = serde_json::from_str::<Value>(trimmed).map_err(|e| e.to_string());
            return Ok(Some(parsed));
        }
    }

    /// After a `Content-Length` header, consume remaining header lines through
    /// the terminating blank line.
    async fn consume_headers(&mut self) -> std::io::Result<()> {
        // Bound the header block: a peer that streams header lines without a
        // terminating blank line must not loop forever. Each line is itself
        // capped by `read_capped_line`, so neither the count nor any single
        // line can grow memory without limit.
        for _ in 0..MAX_HEADER_LINES {
            let mut line = String::new();
            let n = read_capped_line(&mut self.inner, &mut line).await?;
            if n == 0 {
                return Ok(()); // EOF mid-header; read_exact will then fail/short
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                return Ok(()); // end of header block
            }
            // Ignore any non-Content-Length headers (e.g. Content-Type).
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("MCP header block exceeds {MAX_HEADER_LINES} lines without a blank terminator"),
        ))
    }
}

/// Parse a `Content-Length: <n>` header line, case-insensitively on the key.
/// Returns `None` if the line is not a Content-Length header.
pub fn parse_content_length(line: &str) -> Option<usize> {
    let (key, value) = line.split_once(':')?;
    if !key.trim().eq_ignore_ascii_case("content-length") {
        return None;
    }
    value.trim().parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    use tokio::io::BufReader;

    #[test]
    fn parse_content_length_matches_case_insensitively() {
        assert_eq!(parse_content_length("Content-Length: 42"), Some(42));
        assert_eq!(parse_content_length("content-length:7"), Some(7));
        assert_eq!(parse_content_length("Content-Type: x"), None);
        assert_eq!(parse_content_length("not a header"), None);
    }

    #[test]
    fn error_and_response_envelopes_carry_version() {
        let ok = JsonRpcResponse::ok(json!(1), json!({"k": "v"}));
        let s = to_line(&ok).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["k"], "v");

        let err = JsonRpcError::new(json!(2), METHOD_NOT_FOUND, "nope");
        let v: Value = serde_json::from_str(&to_line(&err).unwrap()).unwrap();
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(v["error"]["message"], "nope");
        assert!(v.get("result").is_none());
    }

    #[tokio::test]
    async fn reads_newline_framed_message() {
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
        let mut reader = FramedReader::new(BufReader::new(Cursor::new(input.as_bytes().to_vec())));
        let msg = reader.next_message().await.unwrap().unwrap().unwrap();
        assert_eq!(msg["method"], "initialize");
        assert!(reader.next_message().await.unwrap().is_none()); // EOF
    }

    #[tokio::test]
    async fn reads_content_length_framed_message() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/list\"}";
        let framed = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut reader = FramedReader::new(BufReader::new(Cursor::new(framed.into_bytes())));
        let msg = reader.next_message().await.unwrap().unwrap().unwrap();
        assert_eq!(msg["method"], "tools/list");
        assert_eq!(msg["id"], 9);
    }

    #[tokio::test]
    async fn round_trips_two_newline_messages() {
        let a = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}";
        let b = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}";
        let input = format!("{a}\n{b}\n");
        let mut reader = FramedReader::new(BufReader::new(Cursor::new(input.into_bytes())));
        let first = reader.next_message().await.unwrap().unwrap().unwrap();
        assert_eq!(first["method"], "initialize");
        let second = reader.next_message().await.unwrap().unwrap().unwrap();
        assert_eq!(second["method"], "notifications/initialized");
        assert!(second.get("id").is_none());
        assert!(reader.next_message().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn invalid_json_line_surfaces_as_parse_error() {
        let input = "{not json}\n";
        let mut reader = FramedReader::new(BufReader::new(Cursor::new(input.as_bytes().to_vec())));
        let msg = reader.next_message().await.unwrap().unwrap();
        assert!(
            msg.is_err(),
            "malformed JSON must surface as an Err, not panic"
        );
    }

    #[tokio::test]
    async fn oversize_content_length_closes_stream_not_allocation() {
        // A wildly oversize Content-Length must NOT trigger a multi-terabyte
        // eager allocation. Because draining the body is unbounded, the stream
        // is closed (Err returned) rather than attempting to drain and continue.
        let framed = "Content-Length: 4000000000000\r\n\r\n";
        let mut reader = FramedReader::new(BufReader::new(Cursor::new(framed.as_bytes().to_vec())));
        let result = reader.next_message().await;
        assert!(
            result.is_err(),
            "oversize Content-Length must close the stream (Err), not allocate"
        );
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn oversize_line_surfaces_as_parse_error_and_session_continues() {
        // An oversize newline-framed line must NOT kill the session. The reader
        // drains to the newline boundary and returns Ok(Some(Err)) so the loop
        // can send a JSON-RPC parse error and keep serving.
        //
        // Build: MAX_FRAME_BYTES of 'x', NO newline (triggers the cap),
        // then a valid second message on its own line.
        let mut input: Vec<u8> = vec![b'x'; MAX_FRAME_BYTES];
        // Add a newline to terminate the oversize "line".
        input.push(b'\n');
        // Append a valid second JSON message.
        let second = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
        input.extend_from_slice(second.as_bytes());

        let mut reader = FramedReader::new(BufReader::new(Cursor::new(input)));
        // First read: the oversize line — should surface as Ok(Some(Err)).
        let first = reader.next_message().await.unwrap();
        assert!(
            first.unwrap().is_err(),
            "oversize line must surface as a parse Err, not kill the session"
        );
        // Second read: the valid message that follows — must still be readable.
        let second = reader.next_message().await.unwrap().unwrap().unwrap();
        assert_eq!(second["method"], "ping");
    }

    #[tokio::test]
    async fn runaway_header_block_is_rejected() {
        // A header block that never sends a blank terminator must error out
        // rather than looping/growing without bound.
        let mut framed = String::from("Content-Length: 5\r\n");
        for _ in 0..(MAX_HEADER_LINES + 10) {
            framed.push_str("X-Pad: y\r\n");
        }
        let mut reader = FramedReader::new(BufReader::new(Cursor::new(framed.into_bytes())));
        assert!(
            reader.next_message().await.is_err(),
            "a runaway header block (no blank terminator) must error"
        );
    }
}
