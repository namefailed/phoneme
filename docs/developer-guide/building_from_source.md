# 🏗️ Building Phoneme from Source

Welcome! If you want to contribute to Phoneme, or just want to compile it yourself, you're in the right place. 

Phoneme's backend is written in **Rust**, the frontend is **Vanilla TypeScript** (Vite + Lit), and the desktop wrapper is **Tauri**.

## 📦 Prerequisites

Before you can build Phoneme, you need to install the required toolchains.

### 1. 🦀 Rust
Install Rust using [rustup](https://rustup.rs/). Phoneme tracks the latest stable Rust release.
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. 🟢 Node.js
Install Node.js (v20+ recommended). We recommend using [nvm](https://github.com/nvm-sh/nvm) (or `nvm-windows`).
```bash
nvm install 20
nvm use 20
```

### 3. 🖥️ Tauri OS Dependencies
Tauri requires specific C++ build tools and Webview libraries depending on your OS.
- **Windows**: Install the [C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and the WebView2 SDK.
- **macOS**: `xcode-select --install`
- **Linux**: `sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`

### 4. 🧠 LLVM / libclang (Required for Native Whisper)
Phoneme links directly to `whisper.cpp` via the `whisper-rs` crate. This requires `libclang` to generate the C++ bindings at compile time.

- **Windows**: `winget install LLVM`
  - Then, set the environment variable: `$env:LIBCLANG_PATH="C:\Program Files\LLVM\bin"`
- **macOS**: `brew install llvm`
- **Linux**: `sudo apt install llvm clang libclang-dev`

## 🛠️ Compiling Phoneme

Once your environment is set up, clone the repository and navigate into it:

```bash
git clone https://github.com/namefailed/phoneme.git
cd phoneme
```

Install the frontend dependencies (one time):
```bash
cd frontend
pnpm install
cd ..
```

### Development mode (hot reload)

Use **three terminals**. Vite must be running before `cargo tauri dev` — Tauri
loads `http://localhost:5173` but does not start the dev server for you.

**Terminal 1 — daemon** (recommended for backend debugging):
```bash
cargo run -p phoneme-daemon -- --foreground
```

**Terminal 2 — Vite**:
```bash
cd frontend
pnpm dev
```

**Terminal 3 — Tauri shell** (from the repo root):
```bash
cargo tauri dev
```

If you skip Terminal 1, the tray auto-spawns a background daemon when it starts.

### Quick run (no hot reload)

Build the frontend once, then run the tray binary. It serves `frontend/dist`
and auto-spawns the daemon if needed:
```bash
cd frontend && pnpm build && cd ..
cargo run --bin phoneme-tray
```

## 🔒 Download verification

The first-run wizard downloads its model weights and the bundled whisper-server
from a small allow-list of hosts (Hugging Face, GitHub releases). On top of the
host allow-list, every artifact Phoneme itself loads or extracts is pinned to an
exact **SHA-256**: the whisper GGML models, the semantic-search ONNX model and
tokenizer, and the `whisper-bin-x64.zip` (verified *before* it's unzipped). A
download whose contents don't match its pin — or that comes from a URL with no
pin — is deleted and the wizard surfaces a clear error rather than loading the
file. The pin table and its hash provenance live in
`src-tauri/src/checksums.rs`; if you add a new download URL to the wizard, add
its SHA-256 there too (a unit test fails if a wizard URL has no pin). The Ollama
installer is intentionally not pinned — it's a third-party auto-updating
installer the user launches themselves from a floating URL.

## 🧪 Testing

Phoneme has a comprehensive test suite. Capture is abstracted behind a `Source`
trait, so tests swap the real microphone (`CpalSource`) for a `GeneratorSource`
that emits synthetic audio — you can run the entire suite without a physical
microphone.

To run the Rust backend tests (they run in parallel — each test owns an isolated
in-memory or tempdir catalog; see [Testing & CI](testing_and_ci.md)):
```bash
cargo test --workspace
```

To run the frontend tests:
```bash
cd frontend
pnpm test
```

## 📚 API docs

The Rust backend is documented inline with rustdoc comments. Render the full
API reference and open it in your browser with:
```bash
cargo doc --workspace --no-deps --open
```

Three crates carry `#![warn(missing_docs)]` and are documented to **100%
coverage** — start your reading there:

- **`phoneme-core`** — the shared engine: config, catalog, transcription/LLM
  providers, hooks, webhook, the pipeline types. The crate-level doc is a map of
  the whole system grouped by pipeline stage.
- **`phoneme-audio`** — capture, decode, WAV, silence detection, meeting
  alignment (every public item documents its units — ms, samples, frames).
- **`phoneme-ipc`** — the wire contract. `schema.rs` documents every
  `Request`/`Response`/`DaemonEvent` variant: its payload, reply shape, the
  events it emits, and which surfaces send it.

CI builds the same docs with `RUSTDOCFLAGS="-D warnings"`, so the reference stays
warning-clean — a missing doc comment or a broken intra-doc link fails the build
like any other lint. For the prose companion to the rustdoc, see the
[Architecture Wiki](architecture.md).

## 🚑 Troubleshooting Build Errors

**Error:** `Could not find 'libclang'.`
**Fix:** You skipped Step 4. You must install LLVM and ensure `LIBCLANG_PATH` is set correctly in your environment.

**Error:** `failed to run custom build command for whisper-rs-sys`
**Fix:** This usually means the C++ compiler (MSVC on Windows, GCC/Clang on Unix) is missing. Re-verify Step 3.
