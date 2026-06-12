# 🚀 New Developer Onboarding Guide

Welcome to Phoneme! This guide outlines the key design patterns, technologies, and styling conventions used across the project to help you get up to speed.

---

## 🎨 Frontend Architecture & Conventions (`frontend/src`)

The Phoneme user interface is a desktop single-page application built on a modern, lightweight web stack: **Vite**, **TypeScript**, **Lit**, and **Vanilla CSS**.

### 1. Web Components with Lit
Instead of a virtual-DOM framework (like React or Vue), Phoneme uses native Web Components managed by **Lit** ([`App.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/App.ts)).
- Lit provides lightweight, reactive property bindings. When a `@property()` or `@state()` changes, Lit efficiently re-renders the element.

> [!IMPORTANT]
> **Light DOM vs. Shadow DOM:**
> By default, Lit components encapsulate their templates inside a Shadow DOM. However, to allow global stylesheet variables (Vim cursors, themes, headers) to style components seamlessly without complex styling proxies, most Phoneme components override `createRenderRoot()` to render directly in the **Light DOM**:
> ```typescript
> override createRenderRoot() {
>   return this; // Renders directly into Light DOM instead of creating a Shadow Root
> }
> ```

### 2. State Management & Store
Reactive state is orchestrated via a lightweight, custom store implementation ([`store.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/state/store.ts)).
- Components subscribe to the store upon mounting (`connectedCallback`) and unsubscribe when unmounting (`disconnectedCallback`).
- This minimizes re-renders and decouples data fetching (over IPC) from visual drawing logic.

### 3. Styling Guidelines
- **Vanilla CSS:** We use vanilla CSS stylesheets (e.g. `index.css`, `modal.css`) with CSS custom properties (variables) for theme-matching.
- **No Tailwind CSS:** Avoid adding Tailwind utility classes unless explicitly requested. Design systems, margins, and layouts are defined cleanly in CSS files using flex/grid rules.
- **Chrome Toggles:** Pane slides and animations are managed by applying state classes (e.g., `.phoneme-zen-active`, `.phoneme-hide-header`) to the top-level `<body>` tag, allowing global CSS to smoothly transition positions.

---

## 🦀 Backend Architecture & Conventions (`crates/`, `bin/`)

The backend is built in **Rust** using the **Tokio** asynchronous runtime and **Tauri** for system integration.

### 1. SQLite Database & Migrations (`phoneme-core`)
The database catalog is powered by SQLite ([`catalog.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/catalog.rs)).
- **`sqlx` Migrations:** Database schemas are versioned and managed under [`crates/phoneme-core/migrations/`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/migrations). Never modify a completed migration file directly; instead, create a new chronologically-prefix-named SQL file to execute schema changes.
- **Connection Pools:** The SQLite pool is shared across async tasks and initialized in WAL mode.

### 2. Async Task Lifecycle & Cancellation
The daemon uses Tokio tasks to run operations in the background:
- **Abort Tokens:** Long-running pipeline operations accept a `CancellationToken` from `tokio-util`. If the user cancels a recording or transcription, the token is cancelled, and the async task exits early, reverting state.
- **Process Supervision:** Bundled server executables are spawned and monitored as child processes. If a child process crashes, the supervisor ([`whisper_supervisor.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme-daemon/src/whisper_supervisor.rs)) handles exponential backoff and respawn attempts.

### 3. IPC (JSON Line Named Pipes)
- Communication between the tray, CLI, and daemon runs over Windows Named Pipes using the `JsonLineCodec` (JSON objects separated by `\n`).
- All message types are strictly typed. The shared crate `phoneme-ipc` ([`schema.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-ipc/src/schema.rs)) forces compile-time safety: you cannot add a new action to the CLI or frontend without handling it in the daemon's IPC routing loops.
