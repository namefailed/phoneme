# 🚀 New Developer Onboarding Guide

This guide covers setting up your local environment, the three-terminal development workflow, and the coding conventions used across the frontend and backend.

---

## 💻 1. Development Environment Setup

To work on Phoneme, you will need the following tools installed on your development machine:

1. **Rust Toolchain:** Install Rust via [rustup](https://rustup.rs/). We use the stable toolchain.
2. **NodeJS & pnpm:** Install Node.js (version 20+ recommended) and the `pnpm` package manager (version 9+):
   ```bash
   corepack enable
   corepack prepare pnpm@9.0.0 --activate
   ```
3. **Tauri CLI:** Install the Tauri command-line tool globally:
   ```bash
   cargo install tauri-cli --version "^2.0" --locked
   ```
4. **SQLite Client:** A database viewer (like DB Browser for SQLite) is highly recommended for auditing the local `catalog.db` database.

---

## 🚦 2. The Three-Terminal Developer Workflow

Because Phoneme separates the background daemon from the GUI window, local development requires launching the services in three separate terminals:

### Terminal 1: Start the Background Daemon
Runs the headless daemon which hosts the named pipe IPC server and audio capture
engine. `--foreground` keeps it attached to the terminal so you see its logs while
debugging:
```bash
cargo run -p phoneme-daemon -- --foreground
```
*(If you skip this terminal, the tray auto-spawns a background daemon when it
starts — but running it yourself is the easier way to read backend logs.)*

### Terminal 2: Run the Webpack/Vite Dev Server
Serves the Lit/TypeScript web application:
```bash
cd frontend
pnpm install
pnpm dev
```
*Vite will start a local server, usually at `http://localhost:5173`.*

### Terminal 3: Run the Tauri App Window
Launches the system tray app, webview container, and global shortcut hooks:
```bash
cargo tauri dev
```
*Tauri automatically connects to the Vite dev server and opens the GUI window.*

---

## 🎨 3. Frontend Architecture & Conventions (`frontend/src`)

The frontend is a single-page app built with **Vite**, **TypeScript**, **Lit**, and **Vanilla CSS**.

### Lit Components
All views extend Lit's `LitElement` class ([`App.ts`](../../frontend/src/App.ts)). 

> [!IMPORTANT]
> **Light DOM vs. Shadow DOM:**
> We override `createRenderRoot()` to bypass the Shadow DOM boundary. This allows global stylesheet rules (like Vim cursors or theme variables) to apply cleanly across components:
> ```typescript
> override createRenderRoot() {
>   return this; // Renders directly into Light DOM
> }
> ```

### Reactive Store
UI data is synchronized via a custom reactive store ([`store.ts`](../../frontend/src/state/store.ts)):
- Components subscribe to state changes inside `connectedCallback` and release their subscriptions inside `disconnectedCallback`.
- The store acts as a single source of truth, receiving state updates over the Tauri IPC channel.

### Styling Conventions
- **Vanilla CSS:** Maintain design systems inside vanilla CSS files (e.g. `index.css`, `modal.css`). Do not use Tailwind utility classes.
- **Chrome Transitions:** Slides and collapses are triggered by applying state classes (e.g., `.phoneme-zen-active`, `.phoneme-hide-header`) to `<body>` and animating layouts using CSS transitions.

---

## 🦀 4. Backend Architecture & Conventions (`crates/`, `bin/`)

The backend is built in **Rust** using the **Tokio** async runtime.

### SQLite Database & sqlx
The catalog is stored in `catalog.db` ([`catalog`](../../crates/phoneme-core/src/catalog/mod.rs)).
- **Migrations:** All schema changes must be versioned. Schema migration files are placed under [`crates/phoneme-core/migrations/`](../../crates/phoneme-core/migrations).
- **WAL Mode:** The catalog is opened in Write-Ahead Logging mode to ensure read queries don't block concurrent writes.

### Async Task Cancellation
Background tasks (transcription, cleanup, hooks) accept a `CancellationToken` from `tokio-util`. If the user cancels an operation or quits the app, the token is aborted, allowing the tokio task to unwind and clean up its lock permits safely.

### Where to read next
The backend is documented to 100% rustdoc coverage in three crates — start there
before diving into source:
- **`phoneme-core`** — the shared engine (config, catalog, providers, pipeline types).
- **`phoneme-audio`** — capture, decode, WAV, silence, meeting alignment.
- **`phoneme-ipc`** — the daemon ↔ client wire contract (`schema.rs`).

Build and open it with `cargo doc --workspace --no-deps --open`. The
[Architecture Wiki](architecture.md) is the prose narrative that ties these
together — one story from hotkey press to searchable transcript. The
[Backend](backend_guide.md) and [Frontend](frontend_guide.md) developer guides go
deeper on each side.

---

## 🧪 5. Testing & Code Quality

Before opening a pull request, run the test suites and code linters locally.

### Running Tests
- **Rust backend tests** (run in parallel — each test owns an isolated in-memory
  or tempdir catalog, so there's nothing shared to serialize on; see
  [Testing & CI](testing_and_ci.md)):
  ```bash
  cargo test --workspace
  ```
  *(Tests swap the real microphone for a synthetic `GeneratorSource`, so they run
  on headless CI runners without physical microphones).*
- **Frontend Vitest unit tests:**
  ```bash
  cd frontend
  pnpm test
  ```

### Code Formatters & Linters
- **Rust formatting:**
  ```bash
  cargo fmt --all -- --check
  ```
- **Rust clippy linting:**
  ```bash
  cargo clippy --workspace --all-targets -- -D warnings
  ```
- **TypeScript type checking:**
  ```bash
  cd frontend
  pnpm type-check
  ```
