# 🔌 How to Extend Phoneme

This guide provides step-by-step instructions on how to extend Phoneme with new capabilities: adding custom transcription/LLM providers, creating new IPC commands, and adding new keyboard shortcuts/views.

> **New to the codebase?** Read the [Architecture Wiki](architecture.md) first for
> the end-to-end picture, then the rustdoc — `phoneme-core`, `phoneme-audio`, and
> `phoneme-ipc` are documented to 100% coverage (`cargo doc --workspace --no-deps
> --open`). The module-level docs explain each piece's role before you touch it.
> The frontend has no rustdoc equivalent; its map is the [Frontend Developer
> Guide](frontend_guide.md) plus the TSDoc on every exported symbol.

---

## 🎙️ 1. Adding a New Transcription (STT) or LLM Provider

Phoneme abstracts STT engines using the [`TranscriptionProvider`](../../crates/phoneme-core/src/transcription.rs) trait, and LLM text-generation backends using the [`LlmProvider`](../../crates/phoneme-core/src/llm.rs) trait.

### Step 1: Update the Configuration Schema
Define configuration settings for the new provider under the corresponding config section:
1. Open [`crates/phoneme-core/src/config.rs`](../../crates/phoneme-core/src/config.rs).
2. Add your new backend enum variant to `TranscriptionBackend` (the live set is
   `Local`, `Openai`, `Groq`, `Deepgram`, `Assemblyai`, `Elevenlabs`, `Custom` —
   `Custom` already covers any OpenAI-compatible `/v1/audio/transcriptions`
   endpoint, so reach for it before adding a one-off variant):
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum TranscriptionBackend {
       #[default]
       Local,
       Openai,
       Groq,
       Deepgram,
       Assemblyai,
       Elevenlabs,
       Custom,
       MyNewProvider, // Add this
   }
   ```
3. Update the `validate()` rules in `Config` so a cloud backend that needs a key
   fails fast when one isn't configured (mirror the existing per-backend checks).

### Step 2: Implement the Provider Trait
Write the API client wrapper for the new provider:
- **For STT:** Implement [`TranscriptionProvider`](../../crates/phoneme-core/src/transcription.rs)
  inside `transcription.rs`. The trait takes a **path** to the canonical WAV (the
  provider reads/uploads the file itself) and returns the transcript text. There
  are two methods: `transcribe` returns just the text, and the default-providing
  `transcribe_with_segments` returns timed segments for the timeline views —
  override the latter when your API exposes word/segment timing. It also takes a
  `DiarizationTrack` hint (the recording's Meeting-Mode track): only the local
  OpenAI-compatible path acts on it, labelling a meeting mic track as one fixed
  speaker instead of diarizing it. A new provider can simply ignore the hint and
  run its normal flow:
  ```rust
  pub struct MyNewSttProvider {
      pub client: reqwest::Client,
      pub api_key: String,
      pub model: String,
  }

  #[async_trait]
  impl TranscriptionProvider for MyNewSttProvider {
      async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
          // Read/upload the WAV at `audio_path`, call your API, return the text.
      }
      // Optional: override `transcribe_with_segments` to return timed segments.
  }
  ```
- **For LLM:** Implement [`LlmProvider`](../../crates/phoneme-core/src/llm.rs)
  inside `llm.rs` — `process(prompt, text) -> Result<String>` for one-shot calls,
  and override `process_streaming` when your API streams tokens (so the GUI's
  activity popout can show the response as it arrives).

The exact trait contracts (units, error/None semantics) are documented in the
rustdoc on `transcription.rs` and `llm.rs` — build it with `cargo doc -p
phoneme-core --open`.

### Step 3: Instantiate the Provider in the Pipeline
Wire the provider into the factory methods:
- For STT, update `Transcriber::provider` in [`transcription.rs`](../../crates/phoneme-core/src/transcription.rs):
  ```rust
  TranscriptionBackend::MyNewProvider => Box::new(MyNewSttProvider { ... })
  ```
- For LLM post-processing, update `LlmPostProcessor::provider` in [`llm.rs`](../../crates/phoneme-core/src/llm.rs).

### Step 4: Map the Frontend Settings UI
Expose the new provider to the user interface:
1. Register the provider string key inside the frontend settings list: [`sttProviders.ts`](../../frontend/src/services/sttProviders.ts) or [`llmProviders.ts`](../../frontend/src/services/llmProviders.ts).
2. Update the **Quick Model Switcher** modal ([`ModelPicker.ts`](../../frontend/src/components/ModelPicker.ts)) to render your new provider options in the tabs.

---

## 📡 2. Adding a New IPC Command

IPC command routing follows a compiler-enforced path. If you miss a matching arm, the compiler will fail, ensuring your client and daemon stay in lockstep.

```text
  [Frontend: ipc.ts] ──(Tauri Proxy)──> [Tauri backend: commands.rs]
                                                       │
                                               (Windows Named Pipe)
                                                       │
  [Daemon: pipeline/catalog] <──(Matches Arm)── [Daemon: ipc_handler.rs]
```

### Step 1: Update the IPC Schema
Add the Request variant. Responses are **not** a separate enum — `Response::Ok`
wraps a `serde_json::Value`, so a command's reply shape is just whatever JSON the
handler builds. Document that shape in the variant's rustdoc (the schema doc
comments are the wire contract).
1. Open [`crates/phoneme-ipc/src/schema.rs`](../../crates/phoneme-ipc/src/schema.rs).
2. Add your new variant inside the `Request` enum (with a doc comment stating the
   payload, the Ok-`value` shape, and any events it emits):
   ```rust
   pub enum Request {
       // ... existing requests
       /// Read live server stats. Ok `{"uptime_ms":n,"jobs_done":n}`.
       GetServerStats,
   }
   ```

### Step 2: Implement the Handler in the Daemon
Tell the daemon how to execute the command:
1. Open [`bin/phoneme-daemon/src/ipc_handler.rs`](../../bin/phoneme-daemon/src/ipc_handler.rs).
2. Add the matching arm to `handle_request` (which returns a `Response`). Build the
   reply with `serde_json::json!`, and use `err_response(&e)` for the failure path:
   ```rust
   Request::GetServerStats => {
       let stats = state.server_stats().await;
       Response::Ok(serde_json::json!({
           "uptime_ms": stats.uptime_ms,
           "jobs_done": stats.jobs_done,
       }))
   }
   ```

### Step 3: Add the Tauri Bridge Command
Proxy the command through the Tauri tray process:
1. Open [`src-tauri/src/commands/mod.rs`](../../src-tauri/src/commands/mod.rs).
2. Add a Tauri command that forwards the request over the bridge. The shared
   `forward` helper handles connect/auto-spawn and maps `Response::Err` to a
   `CommandError`, so most commands are a one-liner returning `Result<Value, CommandError>`:
   ```rust
   /// Read live daemon stats. Forwards a `GetServerStats` request.
   #[tauri::command]
   pub async fn get_server_stats(bridge: Br<'_>) -> Result<Value, CommandError> {
       forward(&bridge, Request::GetServerStats).await
   }
   ```
3. Register the command in the `tauri::generate_handler!` list inside [`lib.rs`](../../src-tauri/src/lib.rs).

### Step 4: Expose to Frontend IPC Service
Map the Tauri command to a TypeScript wrapper:
1. Open [`frontend/src/services/ipc.ts`](../../frontend/src/services/ipc.ts).
2. Add the async wrapper function (it calls the Tauri `invoke`, imported as
   `tauriInvoke`, with the snake_case command name):
   ```typescript
   export async function getServerStats(): Promise<ServerStats> {
     return await tauriInvoke<ServerStats>("get_server_stats");
   }
   ```

### Step 5: (Optional) Add a CLI Subcommand
Add command-line support for the new action:
1. Open [`bin/phoneme/src/args.rs`](../../bin/phoneme/src/args.rs) and add your subcommand argument to clap.
2. Open [`bin/phoneme/src/commands/`](../../bin/phoneme/src/commands) and add your command runner file.

---

## ⌨️ 3. Adding a Custom Keybind or Navigation Panel

The frontend keyboard router is centralized in [`keyboard.ts`](../../frontend/src/services/keyboard.ts).

### Step 1: Bind a Hotkey
1. Open [`keyboard.ts`](../../frontend/src/services/keyboard.ts).
2. Under `onKeyDown` or in the shortcut definitions list, add your key matches:
   ```typescript
   // Cycle view on Ctrl + Shift + X
   if (e.ctrlKey && e.shiftKey && e.key === "X") {
     e.preventDefault();
     navigate("custom_view");
     return;
   }
   ```

### Step 2: Handle Pane Actions in Vim Navigation
If the action affects pane-level vim movement (using `h`/`l`/`j`/`k`):
1. Register your action variant inside `dispatchVim("my-custom-action")`.
2. Open the view element handling the split layouts (e.g. [`RecordingsView/index.ts`](../../frontend/src/components/RecordingsView/index.ts)).
3. Listen to the `"phoneme:vim"` custom event and execute the panel focus shift or UI action inside your Lit element listener callback.
