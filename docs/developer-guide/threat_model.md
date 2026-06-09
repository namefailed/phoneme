# Threat model

Phoneme is a **local-first, single-user** desktop app for Windows: a background
daemon captures and transcribes audio, a Tauri tray/WebView is the UI, and a CLI
is a scriptable client. Everything runs as the logged-in user on one machine; no
multi-tenant server is involved. This document captures the trust boundaries,
what we defend, what we explicitly do *not*, and where each mitigation lives.

It is deliberately short. When you change anything that crosses a boundary below
(IPC, hook execution, config secrets, outbound network, the WebView surface),
update this file in the same PR.

## Assets worth protecting

- **Transcripts and audio.** The point of the app; often sensitive (meetings,
  personal notes). Stored under `%LOCALAPPDATA%\phoneme\data\`.
- **API keys.** `whisper.api_key` and `llm_post_process.api_key` for cloud
  transcription / LLM post-processing, kept in `config.toml`.
- **The daemon's command surface.** Record/stop/delete, hook execution,
  shutdown, and the event stream (live transcripts) — all reachable over IPC.

## Trust boundaries

1. **IPC named pipe** (`\\.\pipe\phoneme-daemon`) — daemon (server) ↔ tray/CLI
   (clients). The daemon trusts whatever connects unless the pipe ACL says
   otherwise. This is the primary boundary.
2. **Hook execution** — the daemon spawns user-configured shell commands after a
   transcription. Anything that can choose the command runs code as the user.
3. **WebView ↔ Rust commands** — the Tauri frontend is web content (HTML/JS) that
   invokes Rust `#[tauri::command]`s. Treat the renderer as the lower-trust side:
   a content-injection bug there must not become arbitrary file/process access.
4. **Outbound network** — cloud transcription/LLM providers and the optional
   webhook. URLs and request bodies leave the machine.
5. **Files on disk** — `config.toml` (plaintext secrets today), the audio dir,
   downloaded models/binaries.

## Adversaries we defend against

- **Other users / other logon sessions on the same machine.** Must not read the
  transcript event stream or drive the daemon.
- **A compromised WebView** (malicious/injected web content in the UI). Must not
  read arbitrary files, run arbitrary processes, or exfiltrate secrets beyond
  what the UI legitimately handles.
- **A malformed or hostile IPC peer.** Must not crash/OOM the daemon.
- **Tampered downloads** (model/binary fetches over the network).

## Out of scope (accepted, by design)

- **Same-user malware.** A process running as the user can already read the data
  directory and execute code as the user; it does not need Phoneme to do so. The
  pipe ACL (below) raises the bar to "same user," and an optional per-session
  **auth token** (tracked, not yet implemented) would further bind clients — but
  full same-user isolation is not a goal of a single-user local app.
- **Physical access / disk forensics** beyond at-rest secret encryption (see
  DPAPI, open).
- **Supply-chain integrity of crates/npm packages** (covered separately by
  dependency auditing in CI, not this document).

## Mitigations in place

| Boundary | Mitigation | Where | Audit |
|----------|------------|-------|-------|
| IPC pipe | Owner-only SDDL (`D:P(A;;GA;;;OW)(A;;GA;;;SY)`) on every pipe instance — removes the default cross-session `GENERIC_READ` that exposed the event stream | `phoneme-ipc::named_pipe` | S-C1 |
| IPC pipe | NDJSON frame cap (8 MiB) — an unterminated/oversized frame errors instead of growing the buffer unbounded | `phoneme-ipc::codec` | S-H6 |
| Hook exec | `RefireHook` only runs a command already present in the configured hook allowlist; arbitrary caller commands are rejected | `phoneme-daemon::ipc_handler` | S-C2 |
| WebView | `reveal_file` is restricted to the configured audio directory; `read_file_string` only serves the configured vimrc (both canonicalize + fail closed) | `phoneme-tray::commands` | S-H3 |
| WebView | Daemon-side audio deletion rejects `..` and paths outside the audio dir | `phoneme-daemon::ipc_handler` | S-H5 |
| WebView | Recording IDs from the renderer are validated before reaching ID slicing accessors | `phoneme-tray::commands::parse_id` | — |
| WebView | Error text rendered into the DOM is HTML-escaped | `frontend RecordingDetail` | S-med |
| Logging | `Debug` for config redacts API keys | `phoneme-core::config` | — |

## Open items (prioritized)

- **Same-user auth token on the IPC pipe** — defense-in-depth beyond the owner
  ACL: the daemon mints a random token at startup, stores it in the user-only
  data dir, and clients present it on connect. *(S-C1 follow-up.)*
- **Stop sending API keys to the WebView (masked config DTO)** — `read_config`
  currently serializes the real keys to the renderer. Plan: return the config
  with the two `api_key` fields blanked plus "is set" flags; `write_config`
  merges (a blank incoming key preserves the stored one); move the renderer's
  key-using calls (provider model-list / connection test) **daemon-side** so the
  WebView never needs the raw secret. *(S-H2.)*
- **Encrypt secrets at rest (Windows DPAPI)** — keys live in plaintext
  `config.toml`. Encrypt per-user via DPAPI; decrypt in the daemon only. *(S-H2.)*
- **Webhook SSRF guard** — the webhook POST target is user-configured; enforce
  HTTPS and consider blocking non-loopback private ranges (loopback is allowed on
  purpose for local automation), plus optional HMAC signing. *(S-H1.)*
- **Baseline CSP + narrowed Tauri asset/fs scope** — `tauri.conf.json` ships
  `csp: null` and a broad `$HOME/**` fs scope. *(S-H4.)*
- **Model/binary download checksums** — pin and verify SHA-256 before extracting
  the whisper-server zip and model files. *(S-H7.)*
- **Redact hook test stderr** — `HookTest` output may echo secrets from the
  command's environment; redact before returning to the UI.
- ~~**`cargo audit` + `pnpm audit` in CI** — dependency-vulnerability gate.~~ *Done — non-blocking advisory job added in PR #66.*

See [ROADMAP.md](../../ROADMAP.md) → *Security & privacy hardening* for status.
