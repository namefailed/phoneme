# Testing & CI

Phoneme CI runs on **windows-latest** for all jobs. Local parity commands match GitHub Actions.

## CI jobs (`.github/workflows/ci.yml`)

| Job | Commands |
|-----|----------|
| **Rust** | `cargo fmt --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace -- --test-threads=1` |
| **Frontend** | `pnpm install --frozen-lockfile` · `pnpm lint` · `pnpm exec vitest run` · `pnpm type-check` · `pnpm build` |
| **Tauri build** | Debug MSI/build after Rust + frontend pass |

Tauri build needs `frontend/dist` — CI creates an empty `frontend/dist` before clippy so the Tauri macro succeeds.

## Local pre-PR checklist

```powershell
# From repo root
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1

cd frontend
pnpm install
pnpm lint
pnpm exec vitest run
pnpm type-check
pnpm build
```

Stop `phoneme-daemon` and `phoneme-tray` before `cargo test` if link fails with "Access is denied" on `.exe` files.

## Rust test layout

| Crate / binary | Tests |
|----------------|-------|
| `phoneme-core` | Unit + integration (`tests/`) |
| `phoneme-audio` | `meeting_align`, recorder, wav, silence |
| `phoneme-ipc` | Codec NDJSON, schema round-trips |
| `phoneme-daemon` | Integration tests spawn daemon with synthetic audio |
| `phoneme` CLI | Command parsing, doctor |

### Synthetic audio backend

Set `PHONEME_AUDIO_BACKEND=synthetic` to drive capture without a microphone. Used in CI E2E tests (`GeneratorSource`).

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
