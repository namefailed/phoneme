# phoneme-agent-core

The in-tree **tool seam** for Phoneme's (future) embedded agent panel — the
compiler-enforced tool layer over the daemon's `phoneme_ipc::Request` enum.

Each `Tool` declares a name + JSON schema and maps validated JSON args to a typed
`Request`. **Execution is the caller's job** (send the `Request` over a
`phoneme_ipc::Transport`, render the `Response`), which keeps this layer pure,
synchronous, dyn-safe, and trivially unit-testable — and keeps the tool list in
lockstep with the wire contract *at compile time*.

```rust
use phoneme_agent_core::ToolRegistry;
use serde_json::json;

let reg = ToolRegistry::with_phoneme_tools();
for spec in reg.specs() { println!("{}: {}", spec.name, spec.description); }
let req = reg.to_request("search_recordings", &json!({"query": "standup"}))?;
// hand `req` to your Transport to execute.
```

This crate is the **single source of truth** for the tool catalog. `phoneme-mcp`
depends on it: it builds its MCP `tools/list` from `specs()` and dispatches every
`tools/call` through `to_request(name, args)`, then executes the `Request` over
its own IPC transport. The in-app agent drives *this* registry directly — "same
registry, opposite direction": external agents reach the same capabilities via
`phoneme-mcp`, both off the one catalog, so the two surfaces can't drift.

It registers the canonical Phoneme toolset (in `phoneme-mcp`'s `tools/list`
order):

- **Read-only core** — `start_recording`, `stop_recording`, `get_transcript`,
  `search_recordings`, `list_recent`.
- **Act on it** — `set_title`, `set_favorite`, `set_pinned`, `suggest_tags`,
  `list_tags`, `summarize`, `rerun_cleanup`, `retranscribe`, `more_like_this`,
  `get_words`, `get_segments`, `approve_tag_suggestion`, `dismiss_tag_suggestion`.
- **Meetings** — `start_meeting`, `stop_meeting`, `list_meeting`.
- **Speakers** — `set_speaker_name`, `reassign_speaker_segment`,
  `merge_speakers`, `split_speaker`, `recognize_speakers`.
- **Named-voice library** — `list_named_voices`, `rename_named_voice`,
  `merge_named_voices`, `forget_named_voice`.
- **Destructive prune** (last, confirm first) — `delete_recording`,
  `delete_tag`.

See `docs/design/phoneme-agent-harness.md`.

**Status:** the tool registry + the Phoneme tools + tests, consumed by
`phoneme-mcp`. The in-app agent loop and the Lit chat panel build on top of this.
