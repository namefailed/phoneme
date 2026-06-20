# Testing & CI

Phoneme CI runs on **windows-latest** for all jobs. Local parity commands match GitHub Actions.

## CI jobs (`.github/workflows/ci.yml`)

Five jobs run on every push/PR to `main`/`master`:

| Job | Commands |
|-----|----------|
| **Rust** | `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace -- --test-threads=1` |
| **Rustdoc** | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` — fails on any rustdoc warning, including a `missing_docs` gap or a broken intra-doc link |
| **Frontend** | `pnpm install --frozen-lockfile` · `pnpm lint` · `pnpm exec vitest run` · `pnpm type-check` · `pnpm build` |
| **Tauri build** | `cargo build --workspace` then `cargo tauri build --debug` — runs only after Rust + Frontend pass |
| **Dependency audit** | `cargo audit` + `pnpm audit` — **advisory only** (`continue-on-error`), surfaces RUSTSEC / npm advisories without blocking merges |

Both the Rust and Rustdoc jobs need `frontend/dist` to exist before they compile (the Tauri macro in `src-tauri` requires it), so CI creates an empty `frontend/dist` first.

### The doc-coverage gate

`#![warn(missing_docs)]` is set on `phoneme-core`, `phoneme-audio`, `phoneme-ipc`
(`src/lib.rs`) and the daemon binary (`bin/phoneme-daemon/src/main.rs`). On its
own a `warn` lint wouldn't fail a build — but the **Rustdoc** job builds with
`RUSTDOCFLAGS="-D warnings"`, which promotes every rustdoc warning (an undocumented
public item, a dead intra-doc link) into a hard error. The practical effect: every
public item in those crates must carry a doc comment, or the `docs` job goes red.
Run `cargo doc --workspace --no-deps` locally with the same flag before pushing if
you touched a public API.

Beyond the lint-enforced crates, **every crate and binary in the workspace** carries
a crate-level `//!` module doc that explains its role (and, for the daemon and tray,
its boot/flow model), so the codebase reads top-down. The cross-cutting engineering
decisions — the bugs, races, and constraints behind the non-obvious code — are
written up in
[Technical Challenges & Engineering Decisions](technical_challenges.md), and the
subsystem-level deep dives in [Internals](internals.md).

## Local pre-PR checklist

```powershell
# From repo root
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
$env:RUSTDOCFLAGS="-D warnings"; cargo doc --workspace --no-deps

cd frontend
pnpm install
pnpm lint
pnpm exec vitest run
pnpm type-check
pnpm build
```

Stop `phoneme-daemon` and `phoneme-tray` before `cargo test` if link fails with "Access is denied" on `.exe` files.

### The serial-test contract (`--test-threads=1`)

`cargo test` is always run with `-- --test-threads=1`, locally and in CI. This is
not optional: many backend tests mutate **process-global** state — chiefly the
`PHONEME_DATA_LOCAL` / `PHONEME_CONFIG` environment variables that redirect the
inbox, catalog, and log directories into a per-test temp dir. Environment variables
are shared across threads, so running tests in parallel would let one test's temp
path leak into another's. Single-threaded execution keeps each test's data dirs
isolated. If you add a test that sets an env var, assume the serial contract and
restore/remove the var when done.

### Avoiding lock contention with a separate target dir

The Tauri tray and the daemon hold `target/debug/*.exe` open while running, which
makes `cargo test` fail to relink. To run tests without stopping a live build, point
Cargo at a separate target directory:

```powershell
$env:CARGO_TARGET_DIR="target-test"
cargo test --workspace -- --test-threads=1
```

`target-test/` is gitignored. Parallel work in another terminal may hold the
`target-test` lock — Cargo simply waits for it, which is expected.

## Rust test layout

| Crate / binary | Tests |
|----------------|-------|
| `phoneme-core` | Unit + integration (`tests/`) |
| `phoneme-audio` | `meeting_align`, recorder, wav, silence, decode |
| `phoneme-ipc` | Codec NDJSON, schema round-trips |
| `phoneme-daemon` | In-crate unit tests (`*_test.rs`) plus end-to-end integration tests under `bin/phoneme-daemon/tests/` that spawn a real daemon over a temp pipe and drive it with synthetic audio (`record_synthetic`, `import`, `hook_controls`, `list_session`, …) |
| `phoneme` CLI | Command parsing, doctor |

### Synthetic audio backend

Capture is abstracted behind the `Source` trait in `phoneme-audio`. Production uses
`CpalSource` (the real microphone / WASAPI loopback); tests use `GeneratorSource`,
which feeds silence/sine blocks so the whole pipeline runs on a headless CI runner
with no audio hardware. Set `PHONEME_AUDIO_BACKEND=synthetic` to make the daemon's
recorder pick `GeneratorSource` instead of CPAL — this is how the daemon E2E tests
(e.g. `tests/record_synthetic.rs`) drive capture.

### Meeting alignment tests

`crates/phoneme-audio/src/meeting_align.rs` includes scenario tests for wall-clock dual-track placement (sparse loopback, dense mic).

## Frontend tests

Vitest + jsdom. Run single file:

```powershell
cd frontend
pnpm exec vitest run src/utils/import.test.ts
```

### Lint & format

ESLint (flat config, `frontend/eslint.config.js`) and Prettier (`frontend/.prettierrc`) cover the frontend:

```powershell
cd frontend
pnpm lint           # eslint src — CI runs this; must exit 0
pnpm lint:fix       # apply safe autofixes
pnpm format         # prettier --write src
pnpm format:check   # prettier --check src
```

Errors vs warnings: anything that is a likely bug or dead code (unused vars, useless
escapes/assignments, malformed lit templates) is an **error** and fails `pnpm lint` — fix it.
Style-of-the-moment rules we are knowingly living with are **warnings**; today that is only
`@typescript-eslint/no-explicit-any` (the daemon config object is passed as `any` by design —
its shape is owned by the Rust side). Prefix intentionally-unused params with `_`.

Lint is deliberately not type-aware — `pnpm type-check` already runs `tsc`, so the
type-checked typescript-eslint variants would only slow CI down for no extra coverage.

Prettier matches the existing style (100 cols, double quotes, trailing commas, LF); it is for
new code and editor integration. Don't reformat whole files you aren't otherwise touching —
that buries real changes in noise.

## Manual smoke test

Pre-release: [smoke-test.md](../smoke-test.md) (~10 minutes on a clean VM).

## Diarization Error Rate (DER) harness

`phoneme_core::der` is a pure, unit-tested **collar-0 DER** metric for measuring
how well the local diarizer labels who-spoke-when:
`der = (missed + false_alarm + confusion) / total_reference_speech`. Hypothesis
speakers are mapped onto the reference by overlap first, so labels (`SPEAKER_00`
vs `A`) don't matter — only the grouping. `parse_rttm` reads a reference RTTM and
`DerSegment::from_spans` turns the diarizer's output into a hypothesis, so scoring
a recording is `compute_der(&parse_rttm(reference), &DerSegment::from_spans(&diar.spans))`.

The metric ships with the code; running it needs a **fixture set** (an audio file
plus a hand-checked reference RTTM — not in the repo, since real labelled audio is
the scarce part). The runnable harness lives behind that fixture set as an
`#[ignore]`d test ([`tests/der_harness.rs`](../../crates/phoneme-core/tests/der_harness.rs)),
so it can be a manual check or an optional nightly gate (never a PR blocker):

```text
PHONEME_DER_AUDIO=fixtures/meeting.wav \
PHONEME_DER_RTTM=fixtures/meeting.rttm \
PHONEME_DER_MAX=0.4 \
  cargo test -p phoneme-core --test der_harness -- --ignored --nocapture
```

It prints the full missed / false-alarm / confusion breakdown and fails when the
DER exceeds `PHONEME_DER_MAX` (default 0.5).

## Git hooks

```powershell
./scripts/install-git-hooks.ps1
```

`commit-msg` rejects `Co-authored-by: Cursor` and similar AI attribution lines.

## Worktrees for parallel development

Use isolated worktrees so branch switches do not disturb in-progress work:

```powershell
git worktree add .worktrees/my-feature -b my-feature
```

`.worktrees/` is gitignored.
