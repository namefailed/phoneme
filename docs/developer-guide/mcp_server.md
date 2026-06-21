# 🔌 MCP Server (`phoneme-mcp`)

`phoneme-mcp` is a thin [Model Context Protocol](https://modelcontextprotocol.io)
bridge that exposes Phoneme to MCP-capable AI clients (Claude Desktop, the
Claude CLI, and any other MCP host). It lets an assistant **record, search and
read your library, and act on recordings** — set titles, star favorites, pin
recordings, suggest / approve / dismiss tags, summarize, re-run cleanup,
re-transcribe, find
similar recordings, pull word-level timings and segments, run meetings, and
correct / name / recognize speakers (including the named-voice library) —
through the same daemon the GUI and `phoneme` CLI talk to.

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

## 🧰 The tools

**Record, search & read** (capture + the core reads):

| Tool | Arguments | Maps to | Returns |
|------|-----------|---------|---------|
| `start_recording` | `mode?` (`"oneshot"` \| `"hold"`, default `oneshot`) | `RecordStart` | The new recording id |
| `stop_recording` | _(none)_ | `RecordStop` | The stopped recording id |
| `get_transcript` | `id` (required) | `GetRecording` | The transcript text (or a "not ready yet" note) |
| `search_recordings` | `query` (required), `limit?` (default 10) | `SemanticSearch` | Ranked hits: id, title, score, snippet |
| `list_recent` | `limit?` (default 10) | `ListRecordings` (newest first) | Recent rows: id, status, title, snippet |

**Act on it** (mutating and richer reads):

| Tool | Arguments | Maps to | Returns |
|------|-----------|---------|---------|
| `set_title` | `id` (required), `title?` | `SetRecordingTitle` | Confirmation (omit/blank `title` reverts to auto) |
| `set_favorite` | `id`, `favorite` (both required) | `SetFavorite` | Confirmation |
| `set_pinned` | `id`, `pinned` (both required) | `SetPinned` | Confirmation (pinned recordings sort to the top of the library) |
| `suggest_tags` | `id` (required) | `SuggestTags` | Confirmation (LLM suggestions land for approval; awaits the model) |
| `list_tags` | _(none)_ | `ListAllTags` | A bulleted list of every tag name |
| `summarize` | `id` (required) | `RerunSummary` | Confirmation (regenerates + stores the summary) |
| `rerun_cleanup` | `id` (required) | `RerunCleanup` | Confirmation (re-runs LLM cleanup on the preserved original transcript) |
| `retranscribe` | `id` (required), `model?` | `RetranscribeRecording` | Confirmation — **heavy**: re-runs the whole pipeline |
| `more_like_this` | `id` (required), `limit?` (default 10) | `MoreLikeThis` | Ranked similar recordings: id, title, score, snippet |
| `get_words` | `id` (required) | `GetWords` | A count of word-level timings (start/end offsets, e.g. for caption/SRT export) |
| `get_segments` | `id` (required) | `GetSegments` | A count of transcript segments (start/end offsets, text, speaker label per segment) |
| `approve_tag_suggestion` | `id`, `name` (both required) | `ApproveTagSuggestion` | Confirmation — creates the tag if needed, attaches it, drops the suggestion |
| `dismiss_tag_suggestion` | `id`, `name` (both required) | `DismissTagSuggestion` | Confirmation — drops the suggestion without attaching |

**Meetings** (two-track capture on a shared timeline):

| Tool | Arguments | Maps to | Returns |
|------|-----------|---------|---------|
| `start_meeting` | _(none)_ | `StartMeeting` | The new meeting id |
| `stop_meeting` | _(none)_ | `StopMeeting` | The stopped meeting id |
| `list_meeting` | `meeting_id` (required) | `ListMeeting` | The meeting's track rows (id, status, title, snippet) |

**Speakers** (diarization correction + named-speaker recognition):

| Tool | Arguments | Maps to | Returns |
|------|-----------|---------|---------|
| `set_speaker_name` | `id`, `speaker_label` (≥1), `name` (all required; blank `name` clears) | `SetSpeakerName` | Confirmation — names apply at display/export time, so it's reversible |
| `reassign_speaker_segment` | `id`, `idx` (≥0), `new_label` (≥1) | `ReassignSegmentSpeaker` | Confirmation — moves one `get_segments` segment to another speaker |
| `merge_speakers` | `id`, `from_label` (≥1), `into_label` (≥1) | `MergeSpeakers` | Confirmation — `from` is absorbed into `into` |
| `split_speaker` | `id`, `label` (≥1), `segment_idxs` (non-empty array of ≥0), `new_label` (≥1) | `SplitSpeaker` | Confirmation — moves the listed segments onto a fresh label |
| `recognize_speakers` | `id` (required) | `RecognizeSpeakers` | Named-speaker matches (Speaker N → name), or a "no matches" note |

**Named-voice library** (the enrolled voices recognition matches against):

| Tool | Arguments | Maps to | Returns |
|------|-----------|---------|---------|
| `list_named_voices` | _(none)_ | `ListNamedVoices` | A bulleted list of voices with sample counts |
| `rename_named_voice` | `id`, `name` (both required) | `RenameNamedVoice` | Confirmation |
| `merge_named_voices` | `from_id`, `into_id` (both required) | `MergeNamedVoices` | Confirmation — `from`'s samples move onto `into`, then `from` is removed |
| `forget_named_voice` | `id` (required) | `ForgetNamedVoice` | Confirmation — **reversible** in-app (raw voiceprints stay); confirm with the user first |

**Destructive prune** (irreversible — confirm with the user before calling):

| Tool | Arguments | Maps to | Returns |
|------|-----------|---------|---------|
| `delete_recording` | `id` (required), `keep_audio?` (default `false`) | `DeleteRecording` | Confirmation — **irreversible**; deletes the audio too unless `keep_audio` |
| `delete_tag` | `id` (required, integer) | `DeleteTag` | Confirmation — **irreversible**; detaches the tag from every recording |

`start_recording`'s `oneshot` mode auto-stops on silence; `hold` records until
an explicit `stop_recording`. The `id`-taking tools take the recording id
printed by `list_recent` / `search_recordings` (or by the `phoneme` CLI). To
tag a recording, run `suggest_tags` (the LLM proposes tags the user approves in
the app) and `list_tags` to see the existing tag vocabulary. `summarize`,
`rerun_cleanup` and `retranscribe` re-run pipeline steps; the model overrides
they carry are kept per-run and never written to config (and `retranscribe` is
the heavy one — it re-runs transcription **and** post-processing). Each tool
result is MCP **text content**.

This surface stays in lockstep with the in-tree `phoneme-agent-core` tool
registry (`crates/phoneme-agent-core`) — same tool names mapped to the same IPC
requests, pointed the opposite direction (an external agent calling in vs. the
embedded panel calling out).

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

### Claude Code

Claude Code reads a project-level `.mcp.json` at the repo root. Add a `phoneme`
entry whose `command` points at the built binary:

```json
{
  "mcpServers": {
    "phoneme": {
      "command": "C:\\path\\to\\phoneme\\target\\release\\phoneme-mcp.exe"
    }
  }
}
```

On a dev build the binary is under `target\\debug` instead of `target\\release`;
on macOS/Linux use the POSIX path (`/path/to/phoneme/target/release/phoneme-mcp`).
Like the Desktop entry it takes **no arguments**, and it is observe-only — make
sure the daemon is running (the tray app or `phoneme daemon start`) before
invoking a tool. Restart Claude Code and approve the new server so it connects.

Restart the client; "phoneme" appears in its tool list and the tools above
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
`serverInfo.name = "phoneme-mcp"`) and the tools with their input schemas.

## 🪞 The other direction: `phoneme-agent-core`

`phoneme-mcp` lets an **external** agent reach into Phoneme. Its source —
`crates/phoneme-agent-core` — is the in-tree **tool seam** that is also the
foundation for a future **embedded** agent (an in-app chat panel calling *out* to
the same daemon). Same tool registry, opposite direction — and now literally the
same code: `phoneme-mcp` depends on `phoneme-agent-core` and builds its
`tools/list` + `tools/call` dispatch *from* that registry, so there is no second
hand-maintained tool list to drift.

It is a small, dependency-light crate (`phoneme-core`, `phoneme-ipc`,
`serde_json`, `thiserror` — no async, no transport, no LLM) that holds exactly
the compiler-enforced "tool layer" over the daemon's `phoneme_ipc::Request`
enum:

| Type | Role |
|------|------|
| `ToolSpec` | What a tool advertises: `name`, one-line `description`, and a JSON-Schema `input_schema` object. |
| `Tool` (trait) | `spec()` + `to_request(args) -> Result<Request, ToolError>` — **pure**, synchronous, no I/O. |
| `ToolRegistry` | The set of tools; `specs()` is the `tools/list` surface, `to_request(name, args)` maps a named call to a `Request`. |
| `ToolError` | `Unknown(name)` or `BadArgs { tool, reason }` for the host to surface. |

`ToolRegistry::with_phoneme_tools()` (also `Default`) registers the **canonical
Phoneme toolset in the same order** `phoneme-mcp` exposes it: the read-only core
(`start_recording`, `stop_recording`, `get_transcript`, `search_recordings`,
`list_recent`), the "act on it" tools (`set_title`, `set_favorite`, `set_pinned`,
`suggest_tags`, `list_tags`, `summarize`, `rerun_cleanup`, `retranscribe`,
`more_like_this`, `get_words`, `get_segments`, `approve_tag_suggestion`,
`dismiss_tag_suggestion`), the meeting tools (`start_meeting`, `stop_meeting`,
`list_meeting`), the speaker tools (`set_speaker_name`,
`reassign_speaker_segment`, `merge_speakers`, `split_speaker`,
`recognize_speakers`), the named-voice library (`list_named_voices`,
`rename_named_voice`, `merge_named_voices`, `forget_named_voice`), and the
destructive prune tools (`delete_recording`, `delete_tag`). Because `phoneme-mcp`
builds its surface by iterating this registry, the two can't drift — and a
`phoneme-mcp` test asserts its exposed names equal the registry's, byte-for-byte.

```rust
use phoneme_agent_core::ToolRegistry;
use serde_json::json;

let reg = ToolRegistry::with_phoneme_tools();
for spec in reg.specs() {
    println!("{}: {}", spec.name, spec.description); // the tools/list surface
}
// Validate + map a call to a typed Request — then hand it to your Transport.
let req = reg.to_request("search_recordings", &json!({ "query": "standup" }))?;
```

The key boundary is the same one `phoneme-mcp` honors: **the crate builds the
`Request` but never executes it.** Sending it over a `phoneme_ipc::Transport`
and rendering the `Response` is the caller's job. That keeps the layer pure and
trivially unit-testable, and — because every tool maps to a real `Request`
variant — a renamed or removed variant breaks the build *here*, not at runtime.

> **Status — the single source of truth, consumed by `phoneme-mcp`.** The crate
> ships the registry, the tool set, and its tests, and `phoneme-mcp` now depends
> on it (its `tools.rs` is a thin adapter that re-shapes the registry into the MCP
> wire format and renders results). The in-app agent loop and chat panel that
> would also drive it are still on the roadmap. The harness decision — opencode
> for the standalone agent, a separate embedded harness for the panel, both
> reaching the same tool surface — lives in
> [`../design/phoneme-agent-harness.md`](../design/phoneme-agent-harness.md).

## 🗺️ Relationship to the rest of Phoneme

The MCP tool surface is intentionally a small, stable subset of the full IPC
`Request` enum. For the complete automation surface (every recording, library,
queue, tag, and config operation) drive the daemon directly over its named pipe
or use the `phoneme` CLI — see [`ipc_integration.md`](ipc_integration.md) and
[`cli_reference.md`](cli_reference.md). The roadmap's in-app **Phoneme Agent**
shares the same typed-wrapper idea, pointed the opposite direction (an in-app
agent calling out, rather than external agents calling in) — see
[`phoneme-agent-core`](#-the-other-direction-phoneme-agent-core) above and the
[harness decision record](../design/phoneme-agent-harness.md).
