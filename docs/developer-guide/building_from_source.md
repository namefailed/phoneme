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

Install the frontend dependencies:
```bash
cd frontend
npm install
cd ..
```

To build and run the entire application (Daemon + Frontend + Tray) in development mode:
```bash
cargo run --bin phoneme-tray
```
*(Note: Tauri handles spinning up the Vite dev server for the frontend automatically).*

## 🧪 Testing

Phoneme has a comprehensive test suite. We use `SyntheticSource` audio generators so you can run the entire test suite without needing a physical microphone.

To run the Rust backend tests:
```bash
cargo test --workspace
```

To run the frontend tests:
```bash
cd frontend
npm test
```

## 🚑 Troubleshooting Build Errors

**Error:** `Could not find 'libclang'.`
**Fix:** You skipped Step 4. You must install LLVM and ensure `LIBCLANG_PATH` is set correctly in your environment.

**Error:** `failed to run custom build command for whisper-rs-sys`
**Fix:** This usually means the C++ compiler (MSVC on Windows, GCC/Clang on Unix) is missing. Re-verify Step 3.
