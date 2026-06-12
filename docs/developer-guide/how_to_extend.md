# 🔌 How to Extend Phoneme

This guide provides step-by-step instructions on how to extend Phoneme with new capabilities: adding custom transcription/LLM providers, creating new IPC commands, and adding new keyboard shortcuts/views.

---

## 🎙️ 1. Adding a New Transcription (STT) or LLM Provider

Phoneme abstracts STT engines using the [`TranscriptionProvider`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/transcription.rs) trait, and LLM text-generation backends using the [`LlmProvider`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/llm.rs) trait.

### Step 1: Update the Configuration Schema
Define configuration settings for the new provider under the corresponding config section:
1. Open [`crates/phoneme-core/src/config.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/config.rs).
2. Add your new backend enum variant to `TranscriptionBackend`:
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum TranscriptionBackend {
       Local,
       External,
       Openai,
       Deepgram,
       Assemblyai,
       MyNewProvider, // Add this
   }
   ```
3. Update any validator rules or defaults inside the `Config` struct (e.g. `validate_keys` checking if your API key is configured).

### Step 2: Implement the Provider Trait
Write the API client wrapper for the new provider:
- **For STT:** Implement [`TranscriptionProvider`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/transcription.rs) inside `transcription.rs`:
  ```rust
  pub struct MyNewSttProvider {
      pub client: reqwest::Client,
      pub api_key: String,
      pub model: String,
  }

  #[async_trait]
  impl TranscriptionProvider for MyNewSttProvider {
      async fn transcribe(&self, wav_bytes: &[u8], language: Option<&str>) -> Result<TranscriptionResult> {
          // Send request, parse JSON response, and return Speaker segments
      }
  }
  ```
- **For LLM:** Implement [`LlmProvider`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/llm.rs) inside `llm.rs` (e.g. implementing `process` and `process_streaming` for streaming outputs).

### Step 3: Instantiate the Provider in the Pipeline
Wire the provider into the factory methods:
- For STT, update `Transcriber::provider` in [`transcription.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/transcription.rs):
  ```rust
  TranscriptionBackend::MyNewProvider => Box::new(MyNewSttProvider { ... })
  ```
- For LLM post-processing, update `LlmPostProcessor::provider` in [`llm.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/llm.rs).

### Step 4: Map the Frontend Settings UI
Expose the new provider to the user interface:
1. Register the provider string key inside the frontend settings list: [`sttProviders.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/services/sttProviders.ts) or [`llmProviders.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/services/llmProviders.ts).
2. Update the **Quick Model Switcher** modal ([`ModelPicker.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/components/ModelPicker.ts)) to render your new provider options in the tabs.

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
Add the Request and Response payload definitions:
1. Open [`crates/phoneme-ipc/src/schema.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-ipc/src/schema.rs).
2. Add your new variant inside the `Request` enum:
   ```rust
   pub enum Request {
       // ... existing requests
       GetServerStats, // New command
   }
   ```
3. Add a corresponding payload type or update the `ResponseValue` enum if your command returns custom structured JSON.

### Step 2: Implement the Handler routing in the Daemon
Tell the daemon how to execute the command:
1. Open [`bin/phoneme-daemon/src/ipc_handler.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme-daemon/src/ipc_handler.rs).
2. Add the matching arm to `route_request`:
   ```rust
   Request::GetServerStats => {
       let stats = state.get_stats().await?;
       Ok(ResponseValue::Stats(stats))
   }
   ```

### Step 3: Add the Tauri Bridge Command
Proxy the command through the Tauri tray process:
1. Open [`src-tauri/src/commands.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/src-tauri/src/commands.rs).
2. Add a Tauri command binding:
   ```rust
   #[tauri::command]
   pub async fn get_server_stats() -> Result<ServerStats, String> {
       let res = send_ipc_req(Request::GetServerStats).await?;
       match res {
           ResponseValue::Stats(s) => Ok(s),
           _ => Err("invalid response type".into()),
       }
   }
   ```
3. Register the command in `tauri::Builder` inside [`lib.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/src-tauri/src/lib.rs).

### Step 4: Expose to Frontend IPC Service
Map the Tauri bridge command to a TypeScript function:
1. Open [`frontend/src/services/ipc.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/services/ipc.ts).
2. Add the async wrapper function:
   ```typescript
   export async function getServerStats(): Promise<ServerStats> {
     return await tauriInvoke<ServerStats>("get_server_stats");
   }
   ```

### Step 5: (Optional) Add a CLI Subcommand
Add command-line support for the new action:
1. Open [`bin/phoneme/src/args.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme/src/args.rs) and add your subcommand argument to clap.
2. Open [`bin/phoneme/src/commands/`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme/src/commands) and add your command runner file.

---

## ⌨️ 3. Adding a Custom Keybind or Navigation Panel

The frontend keyboard router is centralized in [`keyboard.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/services/keyboard.ts).

### Step 1: Bind a Hotkey
1. Open [`keyboard.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/services/keyboard.ts).
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
2. Open the view element handling the split layouts (e.g. [`RecordingsView/index.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/components/RecordingsView/index.ts)).
3. Listen to the `"phoneme:vim"` custom event and execute the panel focus shift or UI action inside your Lit element listener callback.
