# Phoneme — handoff guide

A working developer's tour of the codebase. Read this once, keep the spec
([`superpowers/specs/2026-05-19-phoneme-design.md`](superpowers/specs/2026-05-19-phoneme-design.md))
next to you for the *why*, then dive in.

Last touched at master tip `8f944ff` — Plan 4 merge.

---

## 1. Where we are

Plans 1–4 are merged to master. The full pipeline (mic → WAV → llama-server →
hook → catalog) works end-to-end via the CLI today, and the Tauri GUI shows
the recordings list and detail pane. Plans 5–9 are written but unimplemented.

| Plan | Scope | Status |
|---|---|---|
| 1 | `phoneme-core` (catalog, queue, transcription, hook) | ✅ merged |
| 2 | `phoneme-audio` + `phoneme-ipc` | ✅ merged |
| 3a | `phoneme-daemon` binary | ✅ merged |
| 3b | `phoneme` CLI | ✅ merged |
| 4 | `phoneme-tray` Tauri shell + frontend | ✅ merged |
| 5 | Settings / Doctor view / first-run wizard | ⏳ planned |
| 6 | Hooks library / CI / MSI distribution | ⏳ planned |
| 7 | v1.1 features (webhook hook type, etc.) | ⏳ planned |
| 8 | Cross-platform (macOS, Linux) | ⏳ planned |
| 9 | Mobile v2 | ⏳ outline only |

A full code review was conducted against snapshot `8f944ff` and lives at
[`docs/reviews/2026-05-21-code-review.md`](reviews/2026-05-21-code-review.md).
It documented 5 critical bugs, 6 important issues, and 5 nitpicks. **All 16
findings are now fixed** (branch `fix/code-review-2026-05-21`, merged) — see
the review's Resolution note for the two intentional divergences from the
suggested fixes (#9 RefireHook, #14 exit-code sentinel) and the one
deferred nitpick (#16 log rotation, lands with Plan 5/6).

Workspace gate state at master tip:

```
cargo test --workspace        117 passing
cargo clippy --workspace -- -D warnings   clean
cargo fmt --all -- --check                clean
cargo build --workspace --release         clean
pnpm --dir frontend type-check            clean (via node tsc --noEmit)
```

---

## 2. Five-minute architecture

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

The daemon owns all I/O and state. CLI and Tauri are clients. They never
touch SQLite, never touch CPAL, never spawn llama-server. Everything flows
through `phoneme-ipc`'s newline-delimited JSON over a Windows named pipe.

**The split-daemon decision is load-bearing.** Don't dissolve it. If you ever
find yourself wanting the tray app to read the catalog directly, you've
probably misunderstood a use case. Re-read the spec.

---

## 3. Repo layout

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
│   └── phoneme/             User-facing CLI. Thin client; auto-spawns the daemon.
├── src-tauri/               Tauri 2 backend (tray, IPC bridge, commands, event bridge)
├── frontend/                Vite + vanilla TypeScript UI
│   └── src/
│       ├── App.ts
│       ├── services/        ipc.ts, events.ts  (typed Tauri invoke/listen wrappers)
│       ├── state/store.ts
│       ├── styles/
│       └── components/
│           ├── HeaderBar.ts
│           └── RecordingsView/
└── docs/
    ├── HANDOFF.md           ← you are here
    └── superpowers/
        ├── specs/2026-05-19-phoneme-design.md      The single source of truth
        └── plans/*.md       Per-plan build orders (1–9)
```

`target/`, `node_modules/`, `dist/`, `src-tauri/gen/` are all gitignored.
`frontend/pnpm-lock.yaml` is checked in.

---

## 4. Run it

### One-time setup

```bash
# Rust toolchain comes from rust-toolchain.toml — rustup auto-installs
# MSVC linker + Windows SDK already needed (you set those up already)

cargo install tauri-cli --version '^2' --locked   # one-time, ~10min compile

cd frontend && pnpm install                       # one-time
node node_modules/vite/bin/vite.js build          # produces frontend/dist
# (pnpm build also works — the bare `node` invocation bypasses pnpm's
#  postinstall-script gate which currently flags esbuild)
```

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

# Terminal B — record
target/release/phoneme.exe record --oneshot   # speak; stops on silence
target/release/phoneme.exe list               # see the new row
target/release/phoneme.exe show <id>          # see the transcript
```

The default config points at `http://127.0.0.1:5809/v1/audio/transcriptions`
(any OAI-compatible endpoint). Change it via `~/AppData/Roaming/phoneme/config.toml`.

### GUI

```bash
# Terminal A
cargo run -p phoneme-daemon -- --foreground

# Terminal B
cargo tauri dev
```

The tray icon appears. Left-click toggles the window; right-click for menu.
The window starts hidden by config — open it via the tray.

`cargo tauri build` produces an MSI in `src-tauri/target/release/bundle/msi/`
(several minutes; you can iterate on Rust changes with `cargo tauri dev`
which doesn't need a full bundle).

---

## 5. The deviations log

The spec was correct in shape but had real bugs that only surfaced under
implementation. These are fixed in the code AND synced back into the plan
files, but here's the canonical list. If something looks "wrong" relative
to the spec, check here first — it's probably intentional.

### `RecordingId` is a `Mutex<u64>` counter, not the spec's `AtomicU16` swap

The spec's atomic-swap algorithm raced under parallel test load (5% failure).
First fix used `Mutex<IdState>` with a same-millisecond branch — that *also*
raced. Final: pure `Mutex<u64>` monotonic counter; suffix is `counter % 1000`.
**The trailing three digits are not milliseconds anymore.** They're a
monotonic counter mod 1000.

If you're tempted to "optimize" this back to atomics: don't, unless you've
written a property test that pins 0 failures in 50 consecutive runs of the
full test suite.

### `SilenceDetector::is_silent` clamps `sum_sq` at zero

f64 non-associativity drifts the running sum to ~−1e-13 after symmetric
loud→silent transitions, then `sqrt` returns NaN, then `NaN < threshold` is
always false → silence never detected → recorder hangs forever. The
`sum_sq.max(0.0)` clamp is the fix.

Surfaced by `oneshot_mode_stops_on_silence` hanging in CI. Don't touch the
clamp without understanding why it's there.

### `CpalSource` lives on a dedicated `std::thread`

`cpal::Stream` is `!Send` on Windows (WASAPI's COM apartment requires
same-thread ownership), but the `Source` trait requires `Send`. The spec's
verbatim code wouldn't compile. We own the stream on its own OS thread that
blocks on a `stop_rx.recv()` and drops the stream when signaled.

### `Response` uses adjacent tagging, not internal tagging

`#[serde(tag = "status")]` is internally tagged. `Ok(Value::Null)` has no
object to embed an internal tag into, so it round-trips back as `Ok({})`.
Switched to `#[serde(tag = "status", content = "value")]`. The wire shape is
`{"status":"ok","value":...}` — matches the README example all along.

### Daemon fails fast when IPC bind fails

Originally `ipc_server::serve` was in a spawned task that logged its error
and exited. The daemon process kept running idle with no IPC surface. Worst
of both worlds. Now `serve` runs inline against the shutdown signal; if it
errors (e.g. another daemon owns the pipe), `main` returns Err and the
process exits non-zero. Tested by `pipe_singleton.rs`.

### `ListFilter` and a few other Plan-1 types gained `PartialEq` retroactively

The Plan 2 IPC `Request` enum derives `PartialEq`, which required every
nested type to also impl it. `ListFilter` was missing one. Plan 1 was
amended in place rather than carrying it as a "known issue."

### `hold_mode_writes_wav_with_pushed_samples` uses `wait_for_finalize`

Not `stop_and_finalize`. The latter races the cmd-channel `Stop` against
the source channel inside `tokio::select!` (unbiased). In production this
race doesn't matter (CPAL never closes, so `Stop` is the only exit). In the
synthetic-source test it caused flakes. The test now closes the sink and
awaits natural completion.

### `pipeline.rs` uses `cfg.llm.model_path`'s file stem, not `cfg.llm.system_prompt`

Spec had `payload.model = cfg.llm.system_prompt.clone()` — but
`system_prompt` is the prompt *text*, not a model identifier. Until the
llama-server supervisor (Task 12) publishes the actually-loaded model,
we derive a placeholder from `cfg.llm.model_path`'s file stem (or
`"unknown"`).

### Tauri 2 lib.rs/main.rs split

The Plan 4 Task 1 spec put the Tauri builder in a single `main.rs`. Tauri 2
wants the actual code in `lib.rs` (so `#[cfg(mobile)] tauri::mobile_entry_point`
can work). `main.rs` is now a thin entrypoint that calls `lib::run()`.

### pnpm 10+ build-script approvals

pnpm 10+ refuses to run unapproved postinstall scripts (esbuild needs one).
`frontend/pnpm-workspace.yaml` has `onlyBuiltDependencies: [esbuild]` and
`frontend/.npmrc` has `verify-deps-before-run=false` so `vite build` doesn't
prompt. If you ever see "ERR_PNPM_IGNORED_BUILDS" come back, those two files
are why we don't anymore.

---

## 6. Known follow-ups (good places to start)

These are real things I'd want fixed; in rough order of value.

### Daemon integration tests (Plan 3a Task 14 deferred work)

Only 3 of the 9 spec'd integration scenarios are landed:
`daemon_status.rs`, `list_empty.rs`, `pipe_singleton.rs`. The rest need a
`test-mode` cargo feature that swaps `CpalSource` for `SyntheticSource` plus
a feature-gated `Request::TestPushAudio { samples }` IPC variant.

**Why it matters:** the recording → transcription → hook flow has no
end-to-end test. It works manually but regressions could slip in.

**Files to look at:** `bin/phoneme-daemon/src/recorder.rs` (needs the
feature-gated branch), `crates/phoneme-ipc/src/schema.rs` (the new variant),
`bin/phoneme-daemon/tests/common/mod.rs` (the harness already does most of
the plumbing).

### Real tray + bundle icons

Currently 32×32 solid-color PNG placeholders. `cargo tauri icon path/to/source.png`
generates the full icon set if you've got a source image. Once that lands,
delete the manual PNGs in `src-tauri/icons/` and let the generated set take
over.

### `phoneme config set` is stubbed

Prints "not yet implemented; edit config.toml directly." A minimal dotted-
path setter (`phoneme config set llm.external_url http://…`) would be a
~20-line patch in `bin/phoneme/src/commands/config_cmd.rs`. The harder part
is round-tripping unrelated comments / formatting in the TOML — `toml_edit`
crate handles that.

### `phoneme doctor --rebuild-catalog`

Flag is recognized but prints "not yet implemented." The daemon-side
reconstruction logic should walk `inbox/done/*.json` + `audio_dir/**/*.wav`
and re-INSERT into the catalog. Probably a new `Request::RebuildCatalog`
variant + a `reconcile.rs` helper.

### Hook scripts library

`hooks/to-stdout.ps1`, `hooks/to-org-journal.ps1`, `hooks/to-markdown-daily.ps1`
don't exist yet — they're Plan 6. The default config points at
`%APPDATA%/phoneme/hooks/to-stdout.ps1` so the hook step fails until you
either ship those or change the config. Adding a one-liner
`Write-Output (Get-Content -Raw)` works as a placeholder.

### Bundle target switches to NSIS (or stays MSI)

`tauri.conf.json` currently has `targets: ["msi"]`. Once you settle on a
distribution channel (auto-update vs. one-shot installer), revisit. If you
want auto-update later, NSIS via `tauri-plugin-updater` is the easier path.

### Frontend search input isn't wired

`HeaderBar` has a search input but the callback is empty
(`onSearchChange: () => {}`). The catalog already has FTS5 search via
`Request::ListRecordings { filter: { search: Some("…") } }`. Just thread
the input value through to the list refresh. ~10-line change.

### Settings view + first-run wizard

Plan 5 in its entirety. Big enough to be its own session.

---

## 7. Conventions

### No AI attribution anywhere

No "Co-Authored-By", no "with Claude", no AI mention in commits, doc author
tags, READMEs, or code comments. This is enforced for everything user-visible.

### Commit style

`<crate-or-area>: <imperative summary>`

```
phoneme-core: add Catalog::update_duration method
phoneme-daemon: fix Send-safe CpalSource wrapper
frontend: add HeaderBar component + shared styles
milestone: Plan 3a complete (phoneme-daemon green)
Merge Plan 4: Tauri shell + Recordings view
```

Body covers the *why* and any plan-deviation notes (see §5 for examples).

### Per-plan branches

Each plan landed on `feat/plan-<n>-<name>` and was merged into master with
`--no-ff` so `git log --graph master` shows the plan boundaries. Keep using
this pattern.

### Test gates before merge

Every plan merge required all four:
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `cargo build --workspace --release`

Plus frontend `pnpm type-check` for plans that touch TS.

### Deviations sync back into the plan file

If you have to deviate from the per-plan spec, edit the plan file in the
same commit. Future readers should be able to read the plan file and the
code together without surprises. The list in §5 lives here AND in the
relevant plan files.

---

## 8. How to add a new IPC command

This is the most common extension. Walk-through:

1. **Schema** — `crates/phoneme-ipc/src/schema.rs`. Add a variant to
   `Request` (and to `DaemonEvent` if it emits one). Add a roundtrip test
   in `crates/phoneme-ipc/tests/schema.rs`.

2. **Daemon handler** — `bin/phoneme-daemon/src/ipc_handler.rs`. Match the
   new variant in `handle_request`. Call the right `state.catalog.*` or
   `state.recorder.*` method. Return a `Response::Ok(json!(...))` or
   `Response::Err`.

3. **CLI** — `bin/phoneme/src/commands/`. New file per command, plus a
   match arm in `bin/phoneme/src/main.rs`. Add `args.rs` definitions if
   needed.

4. **Tauri command** — `src-tauri/src/commands.rs`. Add a
   `#[tauri::command]` that calls `forward(&bridge, Request::Whatever { ... })`.
   Register it in the `invoke_handler!` macro in `src-tauri/src/lib.rs`.

5. **Frontend wrapper** — `frontend/src/services/ipc.ts`. Add a typed
   `await tauriInvoke<…>("whatever", {…})` function.

6. **Frontend caller** — wherever in `frontend/src/components/` it belongs.

Reference example: `Request::UpdateTranscript` — exists at all five layers
already, ~10–20 lines each.

---

## 9. Quick map: "where do I find…"

| Looking for | File |
|---|---|
| The on-disk paths Phoneme uses | `bin/phoneme-daemon/src/app_state.rs` (`ResolvedPaths`) |
| What `phoneme list` actually shows | `crates/phoneme-core/src/types.rs` (`Recording`) |
| The wire protocol | `crates/phoneme-ipc/src/schema.rs` |
| CPAL → 16kHz/i16 conversion chain | `crates/phoneme-audio/src/source.rs::CpalSource` + `convert.rs` |
| Default config values | `crates/phoneme-core/src/config.rs::Default for Config` |
| Hook JSON payload contract | `crates/phoneme-core/src/types.rs` (`HookPayload`) |
| Tray icon state machine | `src-tauri/src/events.rs::derive_tray_state` |
| Tauri ↔ daemon plumbing | `src-tauri/src/bridge.rs` + `src-tauri/src/commands.rs` |
| Theme tokens | `frontend/src/styles/theme.css` |
| The split layout / live updates | `frontend/src/components/RecordingsView/index.ts` |
| All the per-plan specs | `docs/superpowers/plans/2026-05-19-phoneme-*.md` |
| Top-level architecture spec | `docs/superpowers/specs/2026-05-19-phoneme-design.md` |

---

## 10. If something's wrong

**Tests fail after pulling:** `cargo clean` once, then `cargo test --workspace`.
Stale incremental compilation has bitten me here.

**`cargo tauri dev` won't start:** check `frontend/dist/` exists
(`node frontend/node_modules/vite/bin/vite.js build` regenerates it).
The `tauri::generate_context!` macro panics at compile time if it's missing.

**Daemon won't start (`another phoneme-daemon is already running`):**
`taskkill //F //IM phoneme-daemon.exe //T` (the double-slash is for Git Bash).
No PID lockfile to clean up — the singleton check is purely the named-pipe
bind, which releases when the process exits.

**Recording works, transcript empty:** llama-server probably isn't running
on `127.0.0.1:5809`. Check `phoneme doctor`. The recording itself still
lands in the catalog with status `transcribe_failed`; the WAV is intact and
re-transcribable with `phoneme replay <id>` once the LLM is reachable.

**Hook step always fails:** the default config points at
`%APPDATA%/phoneme/hooks/to-stdout.ps1` which doesn't exist by default.
Either create that file (`Write-Output (Get-Content -Raw)` is enough), or
edit `hook.command` in `config.toml`.

**`pnpm install` complains about build scripts:** `frontend/.npmrc` and
`frontend/pnpm-workspace.yaml` are the workaround. If pnpm gets stricter,
`node node_modules/vite/bin/vite.js build` bypasses the gate.

---

## 11. The plans

Per-plan order files in `docs/superpowers/plans/`. Read these in order if
you want to see the build sequence.

Already executed:

- [`2026-05-19-phoneme-core-foundations.md`](superpowers/plans/2026-05-19-phoneme-core-foundations.md) — Plan 1
- [`2026-05-19-phoneme-audio-and-ipc.md`](superpowers/plans/2026-05-19-phoneme-audio-and-ipc.md) — Plan 2
- [`2026-05-19-phoneme-daemon.md`](superpowers/plans/2026-05-19-phoneme-daemon.md) — Plan 3a
- [`2026-05-19-phoneme-cli.md`](superpowers/plans/2026-05-19-phoneme-cli.md) — Plan 3b
- [`2026-05-19-phoneme-tauri-shell-and-recordings-view.md`](superpowers/plans/2026-05-19-phoneme-tauri-shell-and-recordings-view.md) — Plan 4

Not yet executed:

- [`2026-05-19-phoneme-settings-doctor-wizard.md`](superpowers/plans/2026-05-19-phoneme-settings-doctor-wizard.md) — Plan 5
- [`2026-05-19-phoneme-hooks-ci-distribution.md`](superpowers/plans/2026-05-19-phoneme-hooks-ci-distribution.md) — Plan 6
- [`2026-05-19-phoneme-v1-1-features.md`](superpowers/plans/2026-05-19-phoneme-v1-1-features.md) — Plan 7
- [`2026-05-19-phoneme-cross-platform.md`](superpowers/plans/2026-05-19-phoneme-cross-platform.md) — Plan 8
- [`2026-05-19-phoneme-mobile-v2.md`](superpowers/plans/2026-05-19-phoneme-mobile-v2.md) — Plan 9 (outline only)

Each plan has a list of tasks, each task has its own diff-sized scope.
Mid-plan deviations get appended to the plan file in the same commit that
introduces them, so the plan and code never drift.
