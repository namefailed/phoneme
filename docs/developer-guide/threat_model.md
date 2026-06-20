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
  transcription / LLM post-processing, kept in `config.toml` (DPAPI-encrypted at
  rest — see the mitigations table).
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
5. **Files on disk** — `config.toml` (API-key secrets DPAPI-encrypted at rest), the
   audio dir, downloaded models/binaries.

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
- **Physical access / disk forensics** beyond at-rest secret encryption (API keys
  are DPAPI-encrypted; broader full-disk forensics is out of scope).
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
| WebView | API keys are **masked** before `read_config` crosses to the renderer and restored from disk on `write_config`, so secrets never reach the WebView | `phoneme-tray::commands` (`mask_config_secrets`/`unmask_config_secrets`) | S-H2 (in part) |
| Logging | `Debug` for config redacts API keys | `phoneme-core::config` | — |
| Files on disk | API keys are encrypted at rest with Windows DPAPI (`CryptProtectData`, per-user, `dpapi:v1:` prefix); decrypted only in-process on config load, and legacy plaintext migrates on the next save | `phoneme-core::secret_crypto` + `config.rs` | S-H2 |
| Outbound network | Webhook SSRF guard: loopback targets always allowed (local-first); non-loopback private ranges (RFC1918, v4 link-local `169.254/16`, CGNAT `100.64/10`, IPv6 ULA, IPv6 link-local) blocked unless `[webhook] allow_private_network = true`; public targets must be HTTPS unless `[webhook] allow_http = true`. Hostnames are resolved and every address classified (most restrictive wins), and the webhook client never follows redirects, so an allowed endpoint can't bounce the POST somewhere blocked | `phoneme-core::webhook` | S-H1 |
| Outbound network | Optional HMAC-SHA256 webhook signing: a non-empty `[webhook] hmac_secret` adds an `X-Phoneme-Signature: sha256=<hex>` header (HMAC over the exact body bytes) so the receiver can verify authenticity; the secret is DPAPI-encrypted at rest and masked in the UI | `phoneme-core::webhook` | S-H1 |
| IPC pipe | `HookTest` output is redacted before it crosses the pipe: credential-shaped tokens (`sk-`/`ghp_`/`AKIA`-style prefixes, `Bearer` values, `key=`-style assignments) are masked and the text is length-capped — on the failure path too, since `HookFailed` embeds the command's stderr in its message | `phoneme-core::hook::redact_secrets` + `phoneme-daemon::ipc_handler` | — |
| Tampered downloads | Every artifact the first-run wizard loads or extracts is pinned to an exact SHA-256: the whisper GGML models, the semantic-search ONNX model + tokenizer, and `whisper-bin-x64.zip` (verified **before** it's unzipped). A download whose bytes don't match its pin — or that has no pin — is deleted and the wizard surfaces an error rather than loading it. A unit test fails if a wizard URL has no pin. (The Ollama installer is intentionally unpinned — a third-party auto-updating installer the user launches themselves.) | `phoneme-tray::checksums` (`expected_sha256` / `file_sha256`) | S-H7 |

## Content Security Policy

The WebView ships a strict CSP (`app.security.csp` in `tauri.conf.json`) so a
content-injection bug in the UI can't load remote scripts or exfiltrate to an
arbitrary origin. A looser `devCsp` applies only under `tauri dev`, where Vite
serves the bundle from `http://localhost:5173` and pushes hot-reload updates over
a websocket. Each production directive and why it's there:

| Directive | Value | Why |
|-----------|-------|-----|
| `default-src` | `'self'` | Deny by default; only the bundled app origin is trusted unless a directive below widens it. |
| `script-src` | `'self'` | Only bundled JS runs — no inline scripts, no `eval`. Lit and wavesurfer ship as compiled modules, so nothing inline is needed. |
| `style-src` | `'self' 'unsafe-inline'` | Lit renders into the light DOM and injects `<style>` tags at runtime, and wavesurfer's timeline/hover plugins set inline styles, so inline CSS must be allowed. |
| `img-src` | `'self' data: asset: http://asset.localhost` | `data:` covers the inline SVG icons in CSS `background-image`; the asset scheme covers any image served from disk through `convertFileSrc`. |
| `font-src` | `'self'` | Fonts are bundled or system; nothing is fetched remotely or via `data:`. |
| `media-src` | `'self' asset: http://asset.localhost` | The waveform player streams recording audio through `convertFileSrc`, which resolves to the Tauri asset protocol (`asset:` / `http://asset.localhost` on Windows). |
| `connect-src` | `'self' ipc: http://ipc.localhost asset: http://asset.localhost http: https:` | `ipc:` / `http://ipc.localhost` is Tauri's command channel; the asset entries let wavesurfer fetch the audio it plays; `http:`/`https:` is required because the model-list picker (`fetchLlmModels`) runs **in the WebView** and calls whatever provider endpoint the user configured — a local Ollama, OpenAI, Anthropic, Groq, Gemini, or any OpenAI-compatible URL. Transcription and post-processing requests themselves go out from the daemon, not the WebView. |
| `object-src` | `'none'` | No `<object>`/`<embed>`/plugins. |
| `frame-src` | `'none'` | No iframes are embedded. |
| `worker-src` | `'none'` | No web workers are spawned. |
| `base-uri` | `'self'` | Stops an injected `<base>` tag from re-pointing relative URLs at an attacker origin. |
| `form-action` | `'none'` | The UI never submits HTML forms; navigation is JS-driven. |
| `frame-ancestors` | `'none'` | The app window can't be embedded in another frame. |

`connect-src` is the one directive that can't be locked to a fixed allowlist:
because the live model-list fetch targets user-configured provider URLs, it has to
permit `http:`/`https:`. `script-src`, `object-src`, `frame-src`, and `base-uri`
stay tight, so this does not open a script-execution path.

**Asset-protocol scope.** `app.security.assetProtocol.scope` is what the asset
scheme (and therefore `convertFileSrc`) may read. It was `["$DOCUMENT/**",
"$HOME/**"]` — the whole home directory. It's now narrowed to
`["$DOCUMENT/phoneme/**", "$APPLOCALDATA/**"]`, which still covers the default
audio directory (`~/Documents/phoneme/audio`) and the app-local data dir while no
longer exposing unrelated files. A user who relocates `recording.audio_dir`
outside these roots would need a matching scope entry; the default install and the
documented data location both work as-is.

## Open items (prioritized)

- **Same-user auth token on the IPC pipe** — defense-in-depth beyond the owner
  ACL: the daemon mints a random token at startup, stores it in the user-only
  data dir, and clients present it on connect. *(S-C1 follow-up.)*
- ~~**Encrypt secrets at rest (Windows DPAPI)**~~ *Done — API keys are encrypted
  per-user with `CryptProtectData` (a `dpapi:v1:` prefix) on write and decrypted on
  load; legacy plaintext migrates on the next save, and an undecryptable blob reads
  as unset. Both S-H2 halves (masked DTO + at-rest) are now in place. See the
  mitigations table above.*
- ~~**Webhook SSRF guard**~~ *Done — the webhook client classifies every target
  before POSTing: loopback always allowed (local automation is the point),
  non-loopback private ranges blocked unless `[webhook] allow_private_network =
  true`, public targets HTTPS-only unless `[webhook] allow_http = true`;
  hostnames resolve-and-classify, redirects are never followed. Optional
  HMAC-SHA256 body signing is implemented and tested (a non-empty
  `[webhook] hmac_secret` adds the `X-Phoneme-Signature` header). (S-H1 — see the
  mitigations table above.)*
- ~~**Baseline CSP + narrowed Tauri asset scope**~~ *Done — `tauri.conf.json`
  ships a real production `csp` (plus a `devCsp` for the Vite dev server), and the
  asset-protocol scope is narrowed from `$HOME/**` to the Phoneme audio subtree.
  See the **Content Security Policy** section below. (S-H4.)*
- ~~**Model/binary download checksums**~~ *Done — every wizard artifact (whisper
  GGML models, the semantic ONNX model + tokenizer, the whisper-server zip) is
  pinned to an exact SHA-256 and verified before it's loaded/extracted; an
  unpinned or mismatched download is deleted, and a test fails if a wizard URL
  has no pin. See the mitigations table above. (S-H7.)*
- ~~**Redact hook test stderr**~~ *Done — `phoneme-core::hook::redact_secrets`
  masks credential-shaped tokens, `Bearer` values, and `key=`-style assignments
  (and caps the length) before `HookTest` output returns over IPC; the failure
  path is covered too, since `HookFailed` embeds stderr in its message.*
- ~~**`cargo audit` + `pnpm audit` in CI** — dependency-vulnerability gate.~~ *Done — non-blocking advisory job added in PR #66.*

See [ROADMAP.md](../../ROADMAP.md) → *Security & privacy hardening* for status.
