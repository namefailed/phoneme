# Contributing to Phoneme

Thank you for your interest in contributing to Phoneme! This document outlines the architecture, the development workflow, and how to get your code merged.

## Architecture

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
- **Rust**: 1.75+ (installed via `rustup`, which will automatically read our `rust-toolchain.toml`).
- **Node.js**: 20+
- **pnpm**: 9+
- **Tauri CLI 2**: `cargo install tauri-cli --version "^2.0" --locked`

### Running the App Locally

To develop the app with live hot-reloading for the frontend, you need two terminal windows.

**Terminal 1: Start the backend daemon**
```powershell
cargo run -p phoneme-daemon -- --foreground
```

**Terminal 2: Start the Tauri dev server**
```powershell
cd frontend
pnpm install
cd ..
cargo tauri dev
```

### Running the Tests

Before submitting a PR, ensure all tests pass and the code is formatted:

```powershell
# Run the Rust test suite
cargo test --workspace

# Run the frontend unit tests and type checker
cd frontend
pnpm test --run
pnpm type-check
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

Consult `docs/smoke-test.md` for a full end-to-end verification checklist.

## Submitting Changes

1. **Fork the repository** and create your branch from `master`.
2. **Write tests** for any new features or bug fixes.
3. **Ensure CI passes**. Our GitHub Actions workflow will automatically run the test suite on all PRs.
4. **Commit messages**: We prefer imperative commit messages (e.g., "Add feature X", not "Added feature X"). If your change is significant, please explain the *why* in the commit body.

## Code of Conduct

Please be respectful to everyone in issues and pull requests. We are building this for the love of local software.
