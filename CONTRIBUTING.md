# Contributing to Phoneme

Thank you for your interest in contributing to Phoneme! This document outlines the architecture, the development workflow, and how to get your code merged.

## Architecture

> For a contributor's deep dive — the async task topology, the audio path, the
> SQLite/FTS5 catalog, and the IPC wire protocol — see
> [`docs/developer-guide/internals.md`](docs/developer-guide/internals.md).

Phoneme is designed as a local-first voice transcription suite for Windows. The project is split into three main components within a single Cargo workspace:

1. **`phoneme-daemon`**: The headless backend. It manages the audio recording lifecycle (via CPAL), queueing, sqlite catalog storage, and the lifecycle of the local Whisper (whisper-server). It exposes a Windows Named Pipe (`\\.\pipe\phoneme-daemon`) for IPC.
2. **`phoneme-tray`** (in `src-tauri`): The Tauri 2 desktop shell and system tray icon. It provides the vanilla TypeScript/Vite frontend (in `frontend/`) and communicates with the daemon over IPC.
3. **`phoneme`**: The command-line interface (CLI) client. It is a first-class citizen and can trigger the exact same actions as the GUI (e.g., `phoneme record --oneshot`).

### The IPC Protocol

All IPC communication happens over a newline-delimited JSON protocol defined in `crates/phoneme-ipc`. If you want to add a new feature that requires the frontend to talk to the backend, you must:
1. Define the request/response schema in `phoneme-ipc/src/schema.rs`.
2. Implement the handler in `phoneme-daemon/src/ipc_handler.rs`.
3. Call it from `phoneme-tray` or the `phoneme` CLI.

## Development Environment Setup

### Prerequisites
- **Rust**: installed via `rustup`. The repo's `rust-toolchain.toml` pins `channel = "stable"`, so `rustup` fetches the current stable toolchain (plus `rustfmt` and `clippy`) automatically — no manual version to track.
- **Node.js**: 20+
- **pnpm**: 9+
- **Tauri CLI 2**: `cargo install tauri-cli --version "^2.0" --locked`

### Running the App Locally

To develop the app with live hot-reloading for the frontend, use **three terminal
windows**. The daemon, Vite dev server, and Tauri shell are separate processes —
`cargo tauri dev` loads the UI from `http://localhost:5173` but does **not** start
Vite for you (there is no `beforeDevCommand` in `tauri.conf.json`).

**One-time setup** (from the repo root):
```powershell
cd frontend
pnpm install
cd ..
```

**Terminal 1 — backend daemon** (logs to this terminal; optional but recommended
when debugging recording/IPC):
```powershell
cargo run -p phoneme-daemon -- --foreground
```

**Terminal 2 — Vite dev server** (frontend hot reload):
```powershell
cd frontend
pnpm dev
```

**Terminal 3 — Tauri desktop shell** (from the repo root, after Vite is up):
```powershell
cargo tauri dev
```

If you skip Terminal 1, `phoneme-tray` will auto-spawn a background daemon on
startup — fine for UI work, less ideal when you want daemon logs in the foreground.

**Quick run without hot reload** (uses the built `frontend/dist`, not Vite):
```powershell
cd frontend && pnpm build && cd ..
cargo run --bin phoneme-tray
```
The tray auto-spawns the daemon if none is running.

### Running the Tests

Before submitting a PR, ensure all tests pass and the code is formatted:

```powershell
# Run the Rust test suite
cargo test --workspace

# Run the frontend unit tests and type checker
cd frontend
pnpm install          # ensure node_modules/.bin is populated
pnpm test --run       # or: npx vitest run
pnpm type-check       # or: npx tsc --noEmit
cd ..

# Run formatting and linting
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Pre-PR Checklist

Before opening a pull request, run the following against a local build:

```powershell
# Build the full workspace
cargo build --workspace

# Verify daemon and tray start cleanly
cargo run -p phoneme-daemon -- --foreground &
phoneme doctor
```

Consult [docs/smoke-test.md](docs/smoke-test.md) for a full end-to-end verification checklist. Full doc index: [docs/README.md](docs/README.md).

## Submitting Changes

1. **Fork the repository** and create your branch from `master`.
2. **Write tests** for any new features or bug fixes.
3. **Ensure CI passes**. Our GitHub Actions workflow will automatically run the test suite on all PRs.
4. **Commit messages**: We prefer imperative commit messages (e.g., "Add feature X", not "Added feature X"). If your change is significant, please explain the *why* in the commit body.
5. **No AI-tool attribution**: Do not add `Co-authored-by`, `Made-with`, or similar lines naming Cursor or other coding assistants. Commits and PRs should read as human-authored project work. Run `./scripts/install-git-hooks.ps1` once per clone to enforce this locally.

### Scratch space

Personal scratch notes, drafts, and one-off scripts belong in **`archive_internal/`**
(gitignored) or **`scratch/`** (gitignored) — never in tracked paths. Contributor-facing
documentation lives in `docs/` and this file.

## Code of Conduct

Please be respectful to everyone in issues and pull requests. We are building this for the love of local software.
