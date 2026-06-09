# Testing & CI

Phoneme CI runs on **windows-latest** for all jobs. Local parity commands match GitHub Actions.

## CI jobs (`.github/workflows/ci.yml`)

| Job | Commands |
|-----|----------|
| **Rust** | `cargo fmt --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace -- --test-threads=1` |
| **Frontend** | `pnpm install --frozen-lockfile` · `pnpm exec vitest run` · `pnpm type-check` · `pnpm build` |
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

No ESLint job in CI — repo has no ESLint config.

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
