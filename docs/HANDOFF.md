# Phoneme — handoff guide

A working developer's tour of the codebase: where it stands, how it's being
built, what's left, and the context you'd otherwise have to reverse-engineer.

Read this once. Keep the spec
([`superpowers/specs/2026-05-19-phoneme-design.md`](superpowers/specs/2026-05-19-phoneme-design.md))
next to you for the *why* behind product decisions.

**Master tip:** `7fb6363` — Plan 5 merge. Last updated 2026-05-21.

---

## 1. Where we are

Phoneme is a local-first Windows voice-notes app: press a hotkey, speak, get
a transcript from a locally-running LLM, delivered to a user-owned hook
script. The build is organised as 9 sequential plans. **Plans 1–5 are done
and merged; a full code review has been completed and all findings fixed.**

| Plan | Scope | Status |
|---|---|---|
| 1 | `phoneme-core` — types, config, catalog, queue, transcription, hook | ✅ merged |
| 2 | `phoneme-audio` + `phoneme-ipc` | ✅ merged |
| 3a | `phoneme-daemon` binary | ✅ merged |
| 3b | `phoneme` CLI | ✅ merged |
| 4 | `phoneme-tray` Tauri shell + frontend (recordings view) | ✅ merged |
| — | Code review 2026-05-21 — 16 findings, all fixed | ✅ merged |
| 5 | Settings + Doctor view + first-run wizard | ✅ merged |
| 6 | Hooks library / CI / MSI distribution | ✅ merged |
| 7 | v1.1 features (webhook hook type, etc.) | ✅ merged |
| — | Product Audit 2026-05-21 — Pre-launch full codebase review | ✅ completed |
| 8 | Cross-platform (macOS, Linux) | ⏳ planned |
| 9 | Mobile v2 | ⏳ outline only |

What works today:

- **Full pipeline** end-to-end via the CLI: mic → WAV → llama-server →
  hook → catalog.
- **The daemon** owns recording, the queue, the catalog, llama-server
  supervision, and the IPC server.
- **The CLI** (`phoneme`) drives every operation; auto-spawns the daemon.
- **The Tauri GUI** shows the recordings list + detail pane, a full
  Settings screen, a Doctor health screen, and a 7-step first-run wizard
  that auto-launches when no `config.toml` exists.

Gate state at master tip:

```
cargo test --workspace                     121 passing
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo fmt --all -- --check                 clean
cargo build --workspace --release          clean
frontend type-check (node tsc --noEmit)    clean
```

The code review lives at
[`docs/reviews/2026-05-21-code-review.md`](reviews/2026-05-21-code-review.md)
— all 16 findings are fixed; see its Resolution note for the two
intentional divergences from the suggested fixes.

---

## 2. How this project is built (decision-making)

This section is the part you can't reconstruct from the code. It's how the
work has been driven so far — keep doing it this way unless you have a
reason not to.

### Spec-first, plan-driven

There is one authoritative product spec and **9 per-plan build-order files**
in `docs/superpowers/plans/`. The plans were written upfront after a design
session. Each plan is a sequence of *tasks*; each task is a diff-sized unit
with concrete steps (often literal code). You execute a plan task-by-task,
top to bottom.

Don't freelance architecture. If something feels wrong, the spec is the
tie-breaker — and if the spec is genuinely wrong, fix the spec/plan, don't
silently diverge (see "Deviation handling" below).

### One plan = one branch = one `--no-ff` merge

Each plan is built on `feat/plan-<n>-<name>`, then merged into `master`
with `--no-ff` and a detailed merge-commit body, followed by an empty
`milestone: Plan N complete` commit on the branch before the merge.
`git log --graph master` shows every plan as its own subtree. The code
review got the same treatment: `fix/code-review-2026-05-21`.

### Verbatim vs. design — a deliberate speed call

Plan tasks come in two flavours:

- **Verbatim** — the task hands you literal Rust/TS. Transcribe it inline,
  build, test, commit. Fast. Most tasks are this.
- **Design** — the task describes an outcome and you make real decisions
  (concurrency, lifetimes, error flow). Slow down here; this is where the
  spec's bugs hide.

Telling them apart and not over-ceremonialising the verbatim ones is the
single biggest throughput lever. (Early on, formal review sub-agents were
dispatched per task; that was dropped for verbatim tasks once it was clear
they just transcribe.)

### Deviation handling — the core discipline

The spec is correct in *shape* but has had real bugs that only surface when
the code actually compiles and runs. When you must deviate:

1. **Fix the code.**
2. **Patch the plan file in the same commit** — so a future reader can read
   the plan and the code together with no surprises.
3. **Explain the *why* in the commit body.**
4. **Add it to §6 of this doc** (the canonical deviation log).

A subtler case: when the spec — *or a code review* — suggests a fix that
itself has a flaw. Then you deviate from the *suggestion*, and document why.
Worked example: code-review finding #9 said to fix `RefireHook`'s
IPC-blocking by re-enqueueing it. But the queue pipeline always
re-transcribes, which would clobber a user's manual transcript edit. So
`RefireHook` runs the hook in a detached task instead — fixes the blocking,
keeps the semantics. The review doc's Resolution note records the divergence.

### Test gates before every merge

No plan merges without all of:

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `cargo build --workspace --release`
- frontend type-check, for plans that touch TS

These are non-negotiable. A plan isn't "done" until they're all green.

### Periodic independent review

After Plan 4, a full independent code review was run against the snapshot,
*before* starting new feature work. 16 findings (5 critical) — all fixed on
a dedicated branch and merged before Plan 5 began. Plan a review like this
again after Plan 6 or 7; reviewing a moving target is harder.

### Scope discipline

When a task balloons past its diff-sized intent, scope it down *on purpose*
and document the deferral — don't half-do it. Example: Plan 3a Task 14
specified 9 integration scenarios, but 6 of them need a whole `test-mode`
synthetic-audio harness. Three landed; the rest are a documented, deliberate
follow-up (§6) rather than a silent gap.

---

## 3. Five-minute architecture

```
┌─────────────┐          ┌──────────────────┐         ┌─────────────────┐
│  phoneme    │  IPC     │  phoneme-daemon  │  HTTP   │   llama-server  │
│   (CLI)     │ ─────►   │   (orchestrator) │ ─────►  │ /v1/audio/...   │
└─────────────┘          │                  │         └─────────────────┘
                         │  • CPAL capture  │
┌─────────────┐  IPC     │  • Inbox queue   │  spawn  ┌─────────────────┐
│ phoneme-tray│ ─────►   │  • SQLite catalog│ ─────►  │   hook script   │
│   (Tauri)   │          │  • IPC server    │  (JSON  │  (PowerShell)   │
└─────────────┘          │  • Event bus     │  stdin) └─────────────────┘
                         └──────────────────┘
```

The daemon owns all I/O and state. CLI and Tauri are thin clients. They
never touch SQLite, never touch CPAL, never spawn llama-server. Everything
flows through `phoneme-ipc`'s newline-delimited JSON over a Windows named
pipe.

### Architectural decisions that are load-bearing

These were settled during the design session. Don't relitigate them
casually — but here's the reasoning so you *can* if you must:

- **Split daemon, not tray-as-daemon.** Recording/transcription must work
  with the GUI closed (CLI, hotkey daemons like Kanata/AHK). The daemon is
  the always-on process; the tray is optional. If you find yourself wanting
  the tray to read the catalog directly, you've misunderstood a use case.
- **JSON inbox *and* SQLite catalog — both.** The inbox (`pending/`,
  `processing/`, `done/`, `failed/` dirs, one JSON file per item) gives
  atomic-rename crash safety. The SQLite catalog gives queryable history +
  FTS5 search. Neither alone is enough.
- **Named-pipe IPC, transport-agnostic schema.** `phoneme-ipc` defines the
  wire types behind a `Transport` trait so a future HTTP transport (mobile,
  Plan 9) drops in without touching the schema.
- **Single-instance via the pipe bind**, not a PID lockfile. Windows
  recycles PIDs; a stale lockfile would false-positive. `first_pipe_instance(true)`
  is atomic and self-cleaning.
- **Three LLM modes:** `external` (BYO server), `bundled_model` (Phoneme
  runs llama-server, you supply a GGUF), `bundled_download` (v1.1).
- **Hook contract:** the daemon spawns a user script with the recording
  JSON on stdin; exit 0 = success. Phoneme transcribes; the *user* decides
  where the text goes.

---

## 4. Repo layout

```
phoneme/
├── crates/
│   ├── phoneme-core/        Types, config, catalog (SQLite+FTS5),
│   │                        inbox queue, transcription HTTP client, hook runner
│   ├── phoneme-audio/       CPAL capture, WAV (hound), rubato resample,
│   │                        silence detection, Recorder API
│   └── phoneme-ipc/         Request/Response/DaemonEvent schema,
│                            newline-delimited JSON codec, named-pipe transport
├── bin/
│   ├── phoneme-daemon/      The headless brain. Single binary, single instance.
│   │   src/                 app_state, ipc_server, ipc_handler, recorder,
│   │                        pipeline, queue_worker, event_bus, llm_supervisor,
│   │                        reconcile, shutdown, logging
│   └── phoneme/             User-facing CLI. Thin client; auto-spawns the daemon.
├── src-tauri/               Tauri 2 backend
│   src/                     bridge, commands, config_io, doctor, wizard,
│                            events, tray, lib.rs (entry), main.rs (thin)
├── frontend/                Vite + vanilla TypeScript UI
│   └── src/
│       ├── App.ts, router.ts, main.ts
│       ├── services/        ipc.ts, events.ts
│       ├── state/store.ts
│       ├── styles/          theme.css (Catppuccin Mocha), reset.css
│       └── components/
│           ├── HeaderBar.ts
│           ├── RecordingsView/   list + detail + waveform + splitter
│           ├── SettingsView/     7 sections + form helpers
│           ├── DoctorView/       health checklist
│           └── FirstRunWizard/   orchestrator + 7 steps/
└── docs/
    ├── HANDOFF.md           ← you are here
    ├── reviews/             code reviews (2026-05-21-code-review.md)
    └── superpowers/
        ├── specs/2026-05-19-phoneme-design.md      the product spec
        └── plans/*.md       per-plan build orders (1–9)
```

`target/`, `node_modules/`, `dist/`, `src-tauri/gen/` are gitignored.
`frontend/pnpm-lock.yaml` is checked in.

---

## 5. Run it

### One-time setup

```bash
# Rust toolchain comes from rust-toolchain.toml — rustup auto-installs.
# MSVC linker + Windows SDK required (Visual Studio Build Tools).

cargo install tauri-cli --version '^2' --locked   # one-time, ~10min compile

cd frontend && pnpm install                       # one-time
node node_modules/vite/bin/vite.js build          # produces frontend/dist
```

The bare `node …/vite.js build` invocation sidesteps pnpm's
postinstall-script approval gate. `pnpm build` works too once the gate is
satisfied (see §11).

### Smoke test (no llama-server needed)

```bash
cargo build --workspace --release

target/release/phoneme.exe daemon start    # auto-spawns the daemon
target/release/phoneme.exe daemon status
target/release/phoneme.exe list            # empty array on first run
target/release/phoneme.exe doctor          # green/red checklist
target/release/phoneme.exe daemon stop
```

### Full end-to-end (needs mic + a working llama-server)

```bash
# Terminal A — daemon in foreground so you can watch logs
cargo run -p phoneme-daemon -- --foreground

# Terminal B
target/release/phoneme.exe record --oneshot   # speak; stops on silence
target/release/phoneme.exe list
target/release/phoneme.exe show <id>
```

Default LLM endpoint: `http://127.0.0.1:5809/v1/audio/transcriptions`
(any OAI-compatible server). Configurable in `config.toml`.

### GUI

```bash
# Terminal A
cargo run -p phoneme-daemon -- --foreground
# Terminal B
cargo tauri dev
```

The tray icon appears; left-click toggles the window, right-click for the
menu. The window starts hidden. **If no `config.toml` exists the first-run
wizard launches automatically.** `cargo tauri build` produces an MSI in
`src-tauri/target/release/bundle/msi/`.

---

## 6. The deviations log

The spec was correct in shape but had real bugs that surfaced only under
implementation. These are fixed in the code AND synced into the plan files.
If something looks "wrong" relative to a plan, check here first.

### `RecordingId` is a `Mutex<u64>` counter, not the spec's `AtomicU16` swap

The spec's atomic-swap algorithm raced under parallel test load (5% failure).
A second attempt (`Mutex<IdState>` with a same-millisecond branch) *also*
raced. Final: pure `Mutex<u64>` monotonic counter; suffix is `counter % 1000`.
**The trailing three digits are not milliseconds** — they're a counter.
Don't "optimise" back to atomics without a property test pinning 0 failures
over 50 full-suite runs.

### `SilenceDetector::is_silent` clamps `sum_sq` at zero

f64 non-associativity drifts the running sum to ~−1e-13 after loud→silent
transitions; `sqrt` of a negative is NaN; `NaN < threshold` is always false;
silence is never detected and the recorder hangs forever. `sum_sq.max(0.0)`
is the fix. Surfaced by `oneshot_mode_stops_on_silence` hanging in CI.

### `CpalSource` lives on a dedicated `std::thread`

`cpal::Stream` is `!Send` on Windows (WASAPI COM apartment), but the
`Source` trait requires `Send`. The stream lives on its own OS thread that
blocks on `stop_rx.recv()` and drops the stream when signalled.

### `Response` uses adjacent tagging, not internal tagging

`#[serde(tag = "status")]` can't embed a tag into `Ok(Value::Null)` — it
round-trips as `Ok({})`. Switched to `#[serde(tag="status", content="value")]`;
wire shape `{"status":"ok","value":...}`.

### Daemon fails fast when the IPC bind fails

`ipc_server::serve` runs inline against the shutdown signal; if it errors
(e.g. another daemon owns the pipe) `main` returns Err and exits non-zero.
Previously it was a spawned task that logged-and-exited, leaving the daemon
alive with no IPC. Tested by `pipe_singleton.rs`.

### `ListFilter` gained `PartialEq`/`Eq` retroactively

The Plan 2 IPC `Request` enum derives `PartialEq`; `ListFilter` (Plan 1)
was missing it. Plan 1 was amended in place.

### `hold_mode_writes_wav_with_pushed_samples` uses `wait_for_finalize`

`stop_and_finalize` races the cmd-channel `Stop` against the source channel
in an unbiased `tokio::select!`. Harmless in production (CPAL never closes);
flaky in the synthetic-source test. The test closes the sink and awaits
natural completion instead.

### `pipeline.rs` model name comes from `cfg.llm.model_path`'s file stem

The spec used `cfg.llm.system_prompt` as the placeholder model name —
that's the prompt *text*, not a model id. Derived from `model_path`'s file
stem (or `"unknown"`) until the llama-server supervisor publishes the real
loaded-model name.

### Tauri 2 `lib.rs` / `main.rs` split

Tauri 2 wants the builder in `lib.rs` so `#[cfg(mobile)] mobile_entry_point`
attaches. `main.rs` is a thin entrypoint calling `lib::run()`. `run()` also
builds its own tokio runtime and `block_on`s the initial daemon connect,
because `tauri::Builder::run` is blocking.

### pnpm 10+ build-script approvals

pnpm 10+ refuses unapproved postinstall scripts (esbuild needs one).
`frontend/pnpm-workspace.yaml` (`onlyBuiltDependencies: [esbuild]`) and
`frontend/.npmrc` (`verify-deps-before-run=false`) keep `vite build` quiet.
pnpm occasionally rewrites `pnpm-workspace.yaml` with a placeholder line —
just fix it back.

### Code-review fixes (2026-05-21)

Sixteen findings, all fixed — full detail in
[`docs/reviews/2026-05-21-code-review.md`](reviews/2026-05-21-code-review.md).
The structural ones worth knowing: the hook runner now `start_kill()`s a
timed-out child (Tokio's drop doesn't kill on Windows); the inbox
`claim_next` renames-before-parse so a corrupt file can't wedge the queue;
`RecordingId::parse` is a validating constructor (the Tauri commands use it);
`SubscribeEvents` sends no ACK; `TranscriptionClient` is built once and
reused. Two suggested fixes were *not* taken verbatim — see §2's deviation
discussion (#9 RefireHook) and the review's Resolution note (#14, #16).

### Plan 5 deviations

- `SettingsView/index.ts` gives each section its own child div — the spec
  wrote all sections into one element where each `innerHTML =` clobbered
  the previous.
- `DoctorView` imports `SettingsView`'s stylesheet (it reuses the
  `.settings-toolbar` / `.settings-body` chrome classes).
- `FirstRunWizard` uses an explicit `onFinish` callback instead of the
  spec's `done`+`next` special-case in `go()`.
- Wizard's Microphone step is a device picker only — the spec's "live
  level meter" is deferred.

---

## 7. What's next

### Plan 6 — Hooks library / CI / MSI distribution (the next plan)

`docs/superpowers/plans/2026-05-19-phoneme-hooks-ci-distribution.md`. Ships
the reference hook scripts (`to-stdout.ps1`, `to-org-journal.ps1`,
`to-markdown-daily.ps1`), a GitHub Actions CI workflow, and the MSI build.
This *also* closes the "hook scripts library" follow-up below — the default
config points at `to-stdout.ps1`, which doesn't exist yet.

### Plans 7–9

- **7** — v1.1 features (webhook hook type, etc.).
- **8** — cross-platform (macOS, Linux). The `!Send` CPAL handling and the
  named-pipe transport are the Windows-specific bits to generalise.
- **9** — mobile v2 (outline only — the HTTP transport hook is already
  designed into `phoneme-ipc`).

### Standing follow-ups (independent of the plans)

In rough order of value:

- **Daemon integration tests** — only 3 of Plan 3a's 9 scenarios landed.
  The rest need a `test-mode` cargo feature swapping `CpalSource` for
  `SyntheticSource` + a feature-gated `Request::TestPushAudio`. The
  record→transcribe→hook flow currently has no automated end-to-end test.
  Files: `bin/phoneme-daemon/src/recorder.rs`, `crates/phoneme-ipc/src/schema.rs`,
  `bin/phoneme-daemon/tests/common/mod.rs`.
- **Real tray + bundle icons** — currently 32×32 solid-colour placeholders.
  `cargo tauri icon <source.png>` generates the full set.
- **Frontend search input** — `HeaderBar`'s search box has an empty
  callback. The catalog already does FTS5 (`ListRecordings` with
  `filter.search`); thread the value through. ~10 lines.
- **`phoneme config set`** — CLI stub. The GUI Settings screen writes
  config fine; the CLI setter needs `toml_edit` to preserve comments.
- **`phoneme doctor --rebuild-catalog`** — flag recognised, not wired.
  Needs a `Request::RebuildCatalog` that walks `inbox/done/*.json` +
  `audio_dir`.
- **Log rotation** — `daemon.log_max_size_mb` / `log_max_files` config
  fields are parsed but unused; `logging.rs` uses an unbounded daily
  appender (code-review #16, deferred to Plan 6).

---

## 8. Conventions

### No AI attribution anywhere

No "Co-Authored-By", no "with Claude", no AI mention in commits, doc author
tags, READMEs, or code comments. Enforced for everything user-visible.

### Commit style

`<crate-or-area>: <imperative summary>` — body covers the *why* and any
deviation notes.

```
phoneme-core: add Catalog::update_duration method
phoneme-daemon: fix Send-safe CpalSource wrapper
frontend: add HeaderBar component + shared styles
milestone: Plan 3a complete (phoneme-daemon green)
Merge Plan 4: Tauri shell + Recordings view
```

Pass multi-line messages via a `git commit -F-` heredoc — inline `-m` with
backticks gets mangled by the shell.

### Git

Per-plan `feat/plan-<n>-<name>` branches, `--no-ff` merges, empty
`milestone:` commit per plan. Never amend; never force-push. Projects live
under `~/dev/`.

### Deviations sync back into the plan file

Covered in §2. The plan file and the code must always agree.

---

## 9. How to add a new IPC command

The most common extension — five layers:

1. **Schema** — `crates/phoneme-ipc/src/schema.rs`. Add a `Request` variant
   (and a `DaemonEvent` if it emits one). Roundtrip test in
   `crates/phoneme-ipc/tests/schema.rs`.
2. **Daemon handler** — `bin/phoneme-daemon/src/ipc_handler.rs`. Match it in
   `handle_request`; call `state.catalog.*` / `state.recorder.*`; return
   `Response::Ok(json!(...))` or `Response::Err`.
3. **CLI** — `bin/phoneme/src/commands/`. New file + a match arm in
   `main.rs`; `args.rs` definitions if needed.
4. **Tauri command** — `src-tauri/src/commands.rs`. A `#[tauri::command]`
   calling `forward(&bridge, Request::Whatever {...})`. Register it in the
   `invoke_handler!` macro in `src-tauri/src/lib.rs`.
5. **Frontend** — a typed wrapper in `frontend/src/services/ipc.ts`, then
   the caller component.

Reference: `Request::UpdateTranscript` exists at all five layers.

---

## 10. Quick map: "where do I find…"

| Looking for | File |
|---|---|
| On-disk paths Phoneme uses | `bin/phoneme-daemon/src/app_state.rs` (`ResolvedPaths`) |
| What `phoneme list` shows | `crates/phoneme-core/src/types.rs` (`Recording`) |
| The wire protocol | `crates/phoneme-ipc/src/schema.rs` |
| CPAL → 16kHz/i16 conversion | `crates/phoneme-audio/src/source.rs` + `convert.rs` |
| Default config values | `crates/phoneme-core/src/config.rs` (`Default for Config`) |
| Hook JSON payload contract | `crates/phoneme-core/src/types.rs` (`HookPayload`) |
| Config read/write (atomic) | `src-tauri/src/config_io.rs` |
| Doctor checks | `src-tauri/src/doctor.rs` + `bin/phoneme/src/commands/doctor.rs` |
| Tauri ↔ daemon plumbing | `src-tauri/src/bridge.rs` + `commands.rs` |
| Tray icon state machine | `src-tauri/src/events.rs` + `tray.rs` |
| Settings form helpers | `frontend/src/components/SettingsView/form.ts` |
| First-run wizard | `frontend/src/components/FirstRunWizard/` |
| Theme tokens | `frontend/src/styles/theme.css` |
| Per-plan specs | `docs/superpowers/plans/2026-05-19-phoneme-*.md` |
| Product spec | `docs/superpowers/specs/2026-05-19-phoneme-design.md` |
| Code review | `docs/reviews/2026-05-21-code-review.md` |

---

## 11. If something's wrong

**Tests fail after pulling:** `cargo clean`, then retry — stale incremental
compilation has bitten here.

**`cargo tauri dev` won't start:** `frontend/dist/` must exist. Regenerate
with `node frontend/node_modules/vite/bin/vite.js build`. The
`tauri::generate_context!` macro panics at compile time if it's missing.

**Daemon won't start (`another phoneme-daemon is already running`):**
`taskkill //F //IM phoneme-daemon.exe //T` (double-slash for Git Bash). No
lockfile to clean — the singleton check is the named-pipe bind.

**Recording works, transcript empty:** llama-server isn't reachable. The
recording still lands with status `transcribe_failed`; the WAV is intact —
`phoneme replay <id>` re-transcribes once the LLM is back.

**Hook step always fails:** the default `hook.command` points at
`%APPDATA%/phoneme/hooks/to-stdout.ps1`, which doesn't exist until Plan 6.
Create it (`Write-Output (Get-Content -Raw)`) or edit `config.toml`.

**`pnpm install` complains about build scripts:** `frontend/.npmrc` +
`pnpm-workspace.yaml` are the workaround; if pnpm rewrote the latter with a
placeholder line, restore `onlyBuiltDependencies: [esbuild]`.
`node node_modules/vite/bin/vite.js build` always bypasses the gate.

**Two cargo builds deadlock:** they share one build lock. Don't run
`cargo` twice concurrently — kill stale processes with
`taskkill //F //IM cargo.exe //T`.
