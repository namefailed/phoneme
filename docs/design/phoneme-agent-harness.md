# Phoneme Agent — harness decision record

**Status:** accepted · **Date:** 2026-06-15 · **Scope:** which agent harness powers
the Phoneme Agent, and how it relates to the app.

## Context

Phoneme should grow a *phoneme-aware agent*: not just "answer from my archive"
but *act* on it — search recordings, read transcripts, then tag, title,
summarize, re-run steps, export captions, start/stop recording — and, as a
standalone tool, reach beyond Phoneme to the filesystem. Two surfaces are wanted:

1. **A standalone, powerful agent** ("as powerful as any other agent") that can
   drive Phoneme *and* manipulate the filesystem.
2. **An embedded, app-native agent** — a future in-app chat panel that is tightly
   integrated and sandboxed to Phoneme's own IPC.

These have opposite trust models and lifecycles, so they get different answers.

## Options considered

| Harness | Lang | Shape | Tools / MCP | License | Fit |
|---|---|---|---|---|---|
| **opencode** | Go TUI + Bun/JS HTTP server | Full terminal/serverable agent: loop, permission gating, sessions, LSP, 75+ providers incl. local Ollama | **MCP-native** (configured MCP servers' tools appear automatically); registry-pattern tools incl. filesystem/shell | MIT | Standalone power agent |
| **Rig** (`rig-core`) | Rust | In-process library: provider + vector-store abstractions, tool use, structured output | Bring-your-own tools; no built-in MCP host | MIT/Apache | In-tree daemon/agent loop |
| **Vercel AI SDK** | TypeScript | Library: `generateText` + tools + `stopWhen`/`ToolLoopAgent` (`maxSteps`) | Bring-your-own tools; MCP via community adapters | Apache-2.0 | A Lit/TS embedded panel |

Sources: [opencode](https://github.com/sst/opencode) · [Inside OpenCode](https://blog.openreplay.com/opencode-ai-coding-agent/) · [rig-core](https://docs.rs/rig-core) · [Rig GitHub](https://github.com/0xPlaygrounds/rig) · [AI SDK 5](https://vercel.com/blog/ai-sdk-5) · [AI SDK 6](https://vercel.com/blog/ai-sdk-6).

## Decision

**Build both surfaces, with different harnesses — don't write a loop from scratch.**

### 1. Standalone agent → a separate repo built **on opencode**
- New repo `phoneme-agent` (outside this workspace; local git, no remote yet).
- It runs/drives **opencode** (its server + loop + permission gating + provider
  matrix), and we point opencode's MCP config at our existing **`phoneme-mcp`**
  server. Phoneme's tools (`list_recent`, `search_recordings`, `get_transcript`,
  `start_recording`, `stop_recording`) become agent tools with **near-zero glue**,
  and opencode's own filesystem/shell tools satisfy "manipulate this *and* the
  filesystem." This is the "powerful as any other agent" deliverable.
- **Why opencode here, not Rig/AI-SDK:** it is the only option that already ships
  the *whole* agent (loop, permissions, sessions, multi-provider incl. local
  Ollama) **and** is MCP-native, so it consumes `phoneme-mcp` directly. Rebuilding
  that on Rig or the AI SDK would mean reimplementing a year of agent plumbing.
- **Why a separate repo is safe here:** the original worry was a separate repo
  version-skewing against the weekly-changing `Request` enum. But this client
  talks to Phoneme over **MCP**, which is the *stable* contract (the MCP tool
  surface, not the raw enum), so the skew risk is largely neutralized.

### 2. Embedded agent → **in-tree `crates/phoneme-agent-core`** (Rust seam)
- A thin, compiler-enforced **tool registry over the `Transport` trait** — the
  same typed surface the MCP server and REST API already translate. No opencode
  dependency; the in-app panel must stay sandboxed to Phoneme's IPC (no shell, no
  filesystem) with every tool call rendered as a visible, auditable step and
  destructive ops routed through the existing undo paths.
- The loop itself (when the panel is built) can use **Rig** (Rust-native, fits the
  daemon) or the existing provider stack; that choice is deferred to when the
  panel lands. The *seam* — the tool registry + a `Tool` trait + a dispatcher over
  `Transport` — is what we build now, because it's the shared foundation and it
  can't version-skew (it's in the same workspace as the enum).

## Consequences / sequencing

- **Now (prototype):** scaffold `crates/phoneme-agent-core` (the typed tool
  registry seam) **and** the standalone `phoneme-agent` repo wired to opencode +
  `phoneme-mcp`, with one capability working end-to-end.
- The two share the tool list *as code*: `phoneme-mcp` depends on
  `phoneme-agent-core` and builds its `tools/list` + `tools/call` dispatch from
  that registry, so external agents (in via MCP) and the in-app agent (out via the
  in-tree seam) work off one catalog — "same registry, opposite direction" as the
  roadmap puts it, now with no second hand-maintained list to drift.
- A future in-app chat panel (Lit) builds on `phoneme-agent-core`; if it ever
  wants a TS-side loop instead, the Vercel AI SDK is the fallback — but the tool
  contract stays the Rust seam.

## Revisit if
- opencode's license or maintenance posture changes (re-evaluate Rig-based
  standalone), or
- the embedded panel needs capabilities the IPC seam can't express (then widen
  the seam, not the trust model).
