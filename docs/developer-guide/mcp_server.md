# 🔌 MCP Server (`phoneme-mcp`)

`phoneme-mcp` is a thin [Model Context Protocol](https://modelcontextprotocol.io)
bridge that exposes Phoneme to MCP-capable AI clients (Claude Desktop, the
Claude CLI, and any other MCP host). It lets an assistant **start and stop
recordings, fetch transcripts, and search your library** through the same
daemon the GUI and `phoneme` CLI talk to.

It is deliberately a *translator*, not a brain: MCP is JSON-RPC 2.0 over stdio,
and each tool call maps to exactly one `phoneme-ipc` `Request` over the existing
daemon transport — near-zero business logic of its own (see
[`ipc_integration.md`](ipc_integration.md) for the underlying wire contract).

## 🏛️ How it works

```
MCP client  ──stdin/stdout (JSON-RPC 2.0)──►  phoneme-mcp  ──named pipe──►  phoneme-daemon
(Claude Desktop)                              (this binary)                 (the engine)
```

- **stdout is the protocol channel.** `phoneme-mcp` reads framed JSON-RPC
  requests from **stdin** and writes responses to **stdout**; **all logging goes
  to stderr**. It accepts both newline-delimited and `Content-Length`-framed
  messages, so it works with either MCP client framing.
- **Observe, never spawn.** A tool call dials the daemon's existing named pipe.
  If no daemon is running it returns a **clean tool error** ("Phoneme daemon is
  not running — start it with `phoneme daemon start` or the Phoneme tray app")
  rather than silently launching a long-lived background daemon that would
  outlive the assistant session. You keep explicit control of the daemon; the
  bridge only ever talks to one you already chose to run. (This mirrors the
  `phoneme` CLI's observe-only inspection commands.)
- **Failures are tool errors, not crashes.** Bad arguments, an unreachable
  daemon, or a daemon-side error all come back as a normal MCP tool result with
  `isError: true` and a message — never a panic or a transport fault.

The JSON-RPC methods handled are the MCP lifecycle set: `initialize`,
`notifications/initialized` (no-op), `ping`, `tools/list`, and `tools/call`.
Any other method returns a JSON-RPC `-32601` (method not found).

## 🧰 The five tools

| Tool | Arguments | Maps to | Returns |
|------|-----------|---------|---------|
| `start_recording` | `mode?` (`"oneshot"` \| `"hold"`, default `oneshot`) | `RecordStart` | The new recording id |
| `stop_recording` | _(none)_ | `RecordStop` | The stopped recording id |
| `get_transcript` | `id` (required) | `GetRecording` | The transcript text (or a "not ready yet" note) |
| `search_recordings` | `query` (required), `limit?` (default 10) | `SemanticSearch` | Ranked hits: id, title, score, snippet |
| `list_recent` | `limit?` (default 10) | `ListRecordings` (newest first) | Recent rows: id, status, title, snippet |

`start_recording`'s `oneshot` mode auto-stops on silence; `hold` records until
an explicit `stop_recording`. `get_transcript` takes the recording id printed by
`list_recent` / `search_recordings` (or by the `phoneme` CLI). Each tool result
is MCP **text content**.

## 📋 Client configuration

`phoneme-mcp` is launched by the MCP client as a child process over stdio — you
point the client at the built binary. Build it with:

```bash
cargo build --release -p phoneme-mcp
# binary at: target/release/phoneme-mcp   (phoneme-mcp.exe on Windows)
```

### Claude Desktop

Add an entry to your `claude_desktop_config.json` (on Windows:
`%APPDATA%\Claude\claude_desktop_config.json`; on macOS:
`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "phoneme": {
      "command": "C:\\path\\to\\phoneme\\target\\release\\phoneme-mcp.exe"
    }
  }
}
```

On macOS/Linux use the POSIX path instead:

```json
{
  "mcpServers": {
    "phoneme": {
      "command": "/path/to/phoneme/target/release/phoneme-mcp"
    }
  }
}
```

Restart the client; "phoneme" appears in its tool list and the five tools above
become callable. The MCP server needs **no arguments** — it reads the same
config the daemon and CLI read (honoring `PHONEME_CONFIG`) to find the daemon's
pipe name. Make sure the daemon is running (the tray app or `phoneme daemon
start`) before invoking a tool.

> Tip: set `RUST_LOG=debug` in the server entry's `env` to get verbose logs on
> stderr while debugging — it never pollutes the stdout protocol channel.

## 🔎 Smoke test by hand

Because it speaks newline-framed JSON-RPC on stdio, you can drive it from a
shell. Pipe in an `initialize` and a `tools/list` and read the responses off
stdout:

```bash
printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | ./target/release/phoneme-mcp
```

You should see two JSON-RPC response lines: the `initialize` result (with
`serverInfo.name = "phoneme-mcp"`) and the five tools with their input schemas.

## 🗺️ Relationship to the rest of Phoneme

The MCP tool surface is intentionally a small, stable subset of the full IPC
`Request` enum. For the complete automation surface (every recording, library,
queue, tag, and config operation) drive the daemon directly over its named pipe
or use the `phoneme` CLI — see [`ipc_integration.md`](ipc_integration.md) and
[`cli_reference.md`](cli_reference.md). The roadmap's in-app **Phoneme Agent**
shares the same typed-wrapper idea, pointed the opposite direction (an in-app
agent calling out, rather than external agents calling in).
