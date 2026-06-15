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

Today it mirrors the five tools `phoneme-mcp` exposes externally
(`list_recent`, `search_recordings`, `get_transcript`, `start_recording`,
`stop_recording`) — "same registry, opposite direction": the in-app agent drives
*this*, external agents reach the same capabilities via `phoneme-mcp`. Richer
actions (tag, title, summarize, export, favorite) slot in here as their
`Request`s land. See `docs/design/phoneme-agent-harness.md`.

**Status:** bones — the tool registry + the five Phoneme tools + tests. The agent
loop and the Lit chat panel build on top of this.
