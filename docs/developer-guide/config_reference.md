# Configuration Reference (`config.toml`)

Location: `%APPDATA%\phoneme\config.toml` (expanded from `~/`, `%APPDATA%`, and `%USERPROFILE%` tokens on load).

The config is **validated on load/reload** — an invalid file is rejected with an error. Reload after editing: `phoneme config reload` or IPC `reload_config`. Override the active path with the `PHONEME_CONFIG` environment variable.

Schema source: `crates/phoneme-core/src/config.rs`.

### Example files

A fully-commented `config.example.toml` and `.env.example` live at the **repo root**. `config.example.toml` lists every section and key with its default value and a plain-language note, and is itself a valid, copy-paste-runnable config (drop it in at `%APPDATA%\phoneme\config.toml`). `.env.example` documents the runtime environment variables below. Neither stores a real API key — secrets are entered via Settings and encrypted at rest with DPAPI.

---

## `[whisper]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mode` | `external` \| `bundled_model` \| `bundled_download` | `bundled_download` | How local whisper-server is provisioned |
| `provider` | `local` \| `openai` \| `groq` \| `deepgram` \| `assemblyai` \| `elevenlabs` \| `custom` | `local` | Transcription backend |
| `external_url` | string | `http://127.0.0.1:5809` | OpenAI-compatible server base URL |
| `model_path` | path | `""` | GGML model path (`.bin`) when `mode = bundled_model` |
| `bundled_server_port` | u16 | `5809` | **Preferred** local server port — when another app already holds it, the daemon starts whisper-server on a free port instead and every consumer follows automatically (see [troubleshooting](../user-guide/troubleshooting.md#-something-else-is-using-port-5809)) |
| `bundled_server_args` | string[] | `[]` | Extra whisper-server CLI args |
| `timeout_secs` | u64 | `60` | Transcription HTTP timeout |
| `language` | string? | `null` | BCP-47 hint; omit for auto-detect |
| `api_key` | string | `""` | Cloud provider key (redacted in logs) |
| `model` | string | `""` | Cloud model id |
| `api_url` | string | `""` | Custom provider base URL |

---

## `[preview_whisper]` (optional)

An optional, **independent** transcription provider used only for the live preview, so it never contends with the final transcription. It has the **same keys as `[whisper]`**. Omit the section entirely (the default) to make the preview reuse the main `[whisper]` provider. The final transcript always uses `[whisper]` regardless. Set a distinct `bundled_server_port` if you point it at a second local bundled model — like the main server's, it is a preference: a taken port makes the daemon fall back to a free one, and the preview never picks the main server's port. See [Live Preview & Pre-Roll](../user-guide/streaming_preview_and_preroll.md).

---

## `[recording]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `audio_dir` | path | `~/Documents/phoneme/audio` | WAV output directory |
| `sample_rate` | u32 | `16000` | Capture rate (8000–96000) |
| `channels` | u8 | `1` | 1 = mono, 2 = stereo |
| `silence_threshold_dbfs` | f32 | `-45.0` | Oneshot silence detection |
| `silence_window_ms` | u32 | `3000` | Contiguous silence to stop oneshot |
| `max_duration_secs` | u32 | `300` | Hard cap per recording |
| `input_device` | string | `default` | CPAL device name |
| `source` | `microphone` \| `system_audio` | `microphone` | Single-track capture source |
| `pre_roll_ms` | u32 | `1500` | Idle mic ring buffer; `0` = off. A fresh config ships `1500`; a config file that simply **omits** the key reads as `0` (disabled), so pre-upgrade configs keep the old mic-only-while-recording behavior. |
| `streaming_preview` | bool | `false` | Live partial transcript while recording |
| `auto_stop_on_silence` | bool | `false` | GUI Record button auto-stops on silence; `false` = manual start/stop toggle. Push-to-talk hotkey is always hold-to-record regardless. The Record button's **▾ stop-mode dropdown** (manual / silence / fixed seconds) is stored per device in the browser, not in this file — until a mode is picked there, this key decides. |
| `normalize` | bool | `false` | Peak-normalize a finished recording's gain before writing the WAV, so a quiet mic still hands transcription a healthy signal. Boost-only; silent / already-loud recordings are left untouched. Final captured recording only (single recordings + each meeting track) — not the live preview, not imported files. |
| `normalize_target_dbfs` | f32 | `-1.0` | Target peak ceiling in dBFS when `normalize` is on. `0.0` = full scale; `-1.0` leaves a hair of headroom below clipping. |

---

## `[hook]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `commands` | string[] | `to-stdout.ps1` | Always-run scripts (stdin = JSON payload) |
| `timeout_secs` | u64 | `30` | Per-hook kill timeout |
| `webhook_url` | string? | `null` | Optional HTTP POST target |
| `run_on_transcribe` | bool | `true` | Auto-run hooks after transcription |
| `keyword_rules` | array | `[]` | `{ pattern, command, case_sensitive? }` |

---

## `[webhook]`

Network policy (SSRF guard) for the `hook.webhook_url` POST. Loopback targets
(`127.0.0.1`, `[::1]`, `localhost`) are always allowed, any scheme — webhooks
into n8n / Home Assistant on this machine are the primary use case and no knob
can break that. A hostname is resolved and judged by every address it points
at, so a DNS name aimed at a private IP counts as private; redirects are never
followed.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `allow_private_network` | bool | `false` | Allow non-loopback private targets — RFC1918, link-local, IPv6 ULA (e.g. n8n on a NAS) |
| `allow_http` | bool | `false` | Allow plain `http://` for **public** targets; otherwise public targets must be `https://` |
| `hmac_secret` | string (secret) | `""` | Shared secret for HMAC-SHA256 signing of the POST body. Non-empty adds an `X-Phoneme-Signature: sha256=<hex>` header (HMAC over the exact body bytes) so the receiver can verify authenticity. Encrypted at rest (DPAPI), masked in the UI; empty = signing off. |
| `custom_headers` | table | `{}` | Extra `name = "value"` headers on every webhook POST (e.g. `Authorization`). Entries colliding with a header Phoneme controls (`Content-Type`, the signature header) are ignored. |

---

## `[hotkey]` · `[in_place_hotkey]` · `[meeting_hotkey]`

| Key | Type | Record default | In-place default | Meeting default |
|-----|------|----------------|------------------|-----------------|
| `enabled` | bool | `false` | `false` | `false` |
| `combo` | string | `Ctrl+Alt+Space` | `Ctrl+Alt+I` | `Ctrl+Alt+M` |
| `mode` | `hold` \| `toggle` | `hold` | `hold` | `toggle` |

---

## `[tray]`

| Key | Default | Description |
|-----|---------|-------------|
| `show_on_startup` | `true` | Open main window on launch |
| `minimize_to_tray` | `true` | Close button → tray |
| `start_at_login` | `false` | Windows Run key |

---

## `[interface]`

| Key | Default | Description |
|-----|---------|-------------|
| `strip_titlebar` | `false` | Custom window chrome |
| `format_24h` | `false` | 24-hour timestamps |
| `theme` | `catppuccin-mocha` | CSS theme id |
| `visible_columns` | day, time, duration, status, transcript | List columns |
| `column_widths` | px/fr strings | Resizable column layout |
| `preview_overlay` | `false` | Float the live preview in a system-wide, always-on-top overlay window (requires `recording.streaming_preview`) |
| `vim_nav` | `false` | System-wide vim-style keyboard navigation (`h`/`l` across panes, `j`/`k` within the list, `gg`/`G`, `i`/`Enter`, `Esc`). Distinct from `editor.vim_mode`, which only affects the transcript editor. |
| `animation_speed` | `normal` | Pane show/hide animation speed: `off` \| `fast` \| `normal` \| `slow`. `off` makes sidebar / detail-pane / focus-mode toggles instant. |
| `step_notifications` | `true` | Toast a note as each pipeline step finishes (transcribed, cleaned up, summarized, tags suggested) and when a recording is fully ready. Failure toasts always show regardless — a silently lost transcription is never the right default. |
| `quit_stops_daemon` | `true` | Tray **Quit** also shuts the daemon down: an in-flight recording is stopped and queued first, then the whisper-server(s) and a Phoneme-launched Ollama go with it. `false` = the daemon outlives the tray (headless setups). Also read at daemon **spawn** time to decide whether the tray ties the daemon's lifetime to its own at the OS level (kill-on-close job) — that part of a change applies on the next spawn. |

---

## `[editor]`

| Key | Default | Description |
|-----|---------|-------------|
| `vim_mode` | `false` | Vim bindings in transcript editor |
| `vimrc` | `""` | Inline vimrc |
| `vimrc_path` | `""` | External vimrc file |

---

## `[diarization]`

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `none` | `none` \| `local` \| `deepgram` \| `assemblyai` |
| `local_model_path` | `""` | speakrs ONNX path |

---

## `[daemon]`

| Key | Default | Description |
|-----|---------|-------------|
| `log_level` | `info` | `trace` … `error` (`RUST_LOG` overrides it) |
| `log_max_size_mb` | `10` | **Currently unused** — rotation is daily, not size-based. Kept so older configs keep parsing; a future size-based rotator would honor it. |
| `log_max_files` | `5` | Max rotated **daily** log files (`daemon.log.YYYY-MM-DD`) retained; older days are pruned at daemon startup |
| `pipe_name` | `phoneme-daemon` | Named pipe: `\\.\pipe\<name>` |

---

## `[llm_post_process]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Run LLM after Whisper |
| `provider` | `none` | `ollama`, `openai`, `groq`, `anthropic`, … |
| `api_key` | `""` | Provider key |
| `api_url` | `""` | Override endpoint |
| `model` | `llama3.2:3b` | Model id |
| `prompt` | clean-up instruction | System prompt |
| `timeout_secs` | `30` | LLM HTTP timeout |
| `autostart_ollama` | `true` | Launch `ollama serve` on demand when an LLM step's effective connection is a **local** Ollama and nothing answers there. Applies to every step that inherits this connection (cleanup, summary, tags, titles, in-place polish). An Ollama that was already running when the daemon first probed it is never managed; one the daemon launched is stopped again at daemon shutdown. Remote URLs and non-Ollama providers never launch anything. |

The cleanup provider speaks one of four wire protocols: `ollama`, `openai` (OpenAI-compatible chat completions — used by most cloud providers), `groq`, or `anthropic`. See [Providers & Models](../user-guide/providers_and_models.md).

---

## `[summary]`

Auto AI summary. Generated on demand (**View summary**) or — when `auto = true` — automatically as the **final** pipeline step. Each provider field falls back to the corresponding `[llm_post_process]` value when left empty, so summaries can use a fully independent provider+model or just reuse the cleanup connection.

| Key | Default | Description |
|-----|---------|-------------|
| `auto` | `false` | Summarize every recording automatically |
| `provider` | `""` (inherit) | `ollama`, `openai`, `groq`, `anthropic`; empty inherits cleanup |
| `api_key` | `""` (inherit) | Empty inherits the cleanup key |
| `api_url` | `""` (inherit) | Empty inherits / provider default |
| `model` | `""` (inherit) | Empty inherits the cleanup model |
| `prompt` | summarize instruction | Summary system prompt |

Stored results: `summary` and `summary_model` columns on the recording.

---

## `[recording]` — meeting preview

| Key | Default | Meaning |
|-----|---------|---------|
| `meeting_preview` | `"toggle"` | How the live preview handles a meeting's two tracks (needs `streaming_preview`). `"toggle"` — one preview loop follows a single track; the overlay's 🎤/🔊 button switches it (same cost as a single recording). `"both"` — one loop per track, captions stacked in the overlay (~double the preview work; the loops interleave on the shared transcription semaphore). |

## `[auto_tag]`

LLM tag suggestions, approved by the user before they apply. Blank
provider/key/URL/model fields inherit the `[llm_post_process]` connection,
like `[summary]`.

| Key | Default | Meaning |
|-----|---------|---------|
| `auto` | `false` | Suggest tags automatically as a pipeline step on every recording |
| `provider` | `""` | `ollama` / `openai` / `groq` / `anthropic`; empty → inherit |
| `api_key` | `""` | Empty → inherit the cleanup key (DPAPI-encrypted at rest) |
| `api_url` | `""` | Empty → inherit / provider default |
| `model` | `""` | Empty → the cleanup model |
| `prompt` | (built-in) | Tagger instructions; the existing-tag list and transcript are appended at run time |
| `auto_accept_existing` | `false` | Auto-apply a suggestion whose tag already exists (any tag in your library, matched case-insensitively); only suggestions that would create a brand-new tag wait as approve/dismiss chips. |
| `max_tags` | `5` | Maximum suggestions per recording (clamped 1–12) |

Suggestions land on the recording (`tag_suggestions`) and are surfaced in the
GUI as approve/dismiss chips; approving creates-or-fetches the tag and attaches
it.

## `[title]`

Auto-generated recording titles. The heuristic (first meaningful sentence of
the transcript — leading filler stripped, cut at a word boundary near 60
chars) is free and runs by default; `use_llm` upgrades it to a short
LLM-written title that falls back to the heuristic on any error. Blank
provider/key/URL/model fields inherit the `[llm_post_process]` connection,
like `[summary]` and `[auto_tag]`.

| Key | Default | Meaning |
|-----|---------|---------|
| `enabled` | `true` | Generate a title for every recording as a pipeline step (and refresh it on retranscribe) |
| `use_llm` | `false` | Ask the LLM for the title instead of the heuristic; the heuristic remains the fallback on any error |
| `provider` | `""` | `ollama` / `openai` / `groq` / `anthropic`; empty → inherit |
| `api_key` | `""` | Empty → inherit the cleanup key (DPAPI-encrypted at rest) |
| `api_url` | `""` | Empty → inherit / provider default |
| `model` | `""` | Empty → the cleanup model |
| `prompt` | (built-in) | Title instructions; the transcript is appended at run time |

Stored results: `title` plus `title_is_auto` on the recording. A title the
user sets by hand (`SetRecordingTitle` with a value → `title_is_auto = 0`)
is never overwritten by auto generation; clearing it (`SetRecordingTitle`
with `null`) reverts to auto and the next pipeline run fills it again.

## `[in_place]`

Dictation (transcription-in-place) behavior — the fast lane. Edited by
Settings → Capture → Dictation, including the `stt` picker (Automatic ↔ Custom).

| Key | Default | Meaning |
|-----|---------|---------|
| `cleanup` | `"fast"` | Text polish before typing: `"fast"` (rule-based, instant: fillers, non-speech tags, stutters, caps, punctuation), `"off"` (raw), `"llm"` (full pass through `[llm_post_process]` — slow). |
| `type_mode` | `"type"` | `"type"` = simulated keystrokes; `"paste"` = clipboard + Ctrl+V with the previous clipboard restored (near-instant for long text). |
| `save_to_library` | `true` | Persist the dictation (transcript, segments, audio) after typing. `false` = ephemeral — row and WAV deleted once typed. |
| `full_pipeline` | `false` | Route dictations through the normal queue and every configured step (cleanup, summary, tags, hooks) — the legacy behavior. `type_first` picks when the text is typed. |
| `type_first` | `false` | Only meaningful with `full_pipeline`. `true` = a type-only fast pass types the quick transcription immediately while the pipeline continues in the background for the library copy (the typed text is the fast polish, not the LLM cleanup, and the pipeline skips its own end-of-run typing). `false` = the typed text waits for, and includes, every configured step. |
| `stt` | *(unset)* | Optional dedicated STT provider table shaped like `[whisper]`. Unset: dictation follows the Live Preview's provider when the preview is enabled, else `[whisper]`. For a local model, point it at an already-running server — the daemon does not supervise a third server. |


## `[semantic_search]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Index new transcripts (chunked, hybrid with FTS5) |
| `model_dir` | `""` | ONNX model + tokenizer directory |
| `max_tokens` | `256` | Truncation length before embedding (all-MiniLM was trained at 256) |
| `pooling` | `mean` | Token pooling: `mean` (MiniLM/MPNet/E5/BGE) or `cls` |
| `token_type_ids` | `true` | Feed `token_type_ids` (BERT-family yes; some E5 exports reject it) |
| `query_prefix` | `""` | Prefix prepended to a search **query** (e.g. `query: ` for E5) |
| `passage_prefix` | `""` | Prefix prepended to a stored **transcript** (e.g. `passage: ` for E5) |

Changing the model or its dimension makes old vectors unsearchable — re-index with **Re-embed all recordings** (IPC `ReembedAll`). See [Semantic Search](../user-guide/semantic_search.md).

---

## `[retention]`

| Key | Default | Description |
|-----|---------|-------------|
| `max_age_days` | `null` | Delete older than N days |
| `max_count` | `null` | Keep newest N recordings |
| `delete_audio` | `false` | Drop WAV, keep catalog row |

---

## `[rest_api]`

Optional localhost HTTP/REST + SSE bridge (the `phoneme-rest` binary). Off by default; binds `127.0.0.1` only (loopback is the trust boundary). See [REST API](rest_api.md).

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Allow the `phoneme-rest` bridge to run. When `false`, the binary exits with a clear message and the HTTP surface is never exposed. |
| `port` | `3737` | TCP port bound on `127.0.0.1` (loopback only — the bridge never listens on `0.0.0.0`). |

---

## Config profiles

Named copies under `%APPDATA%\phoneme\profiles\`. Switch via tray menu. See [Config Profiles](../user-guide/config_profiles.md).

---

## Environment variables

Runtime variables read by the daemon / CLI / tray. See the commented `.env.example` at the repo root for examples; Phoneme reads these from the process environment and does not auto-load a `.env` file.

| Variable | Effect |
|----------|--------|
| `PHONEME_CONFIG` | Override the active config file path (honored by the daemon, CLI, and tray) |
| `PHONEME_DATA_LOCAL` | Override the local data dir (inbox / catalog / logs); default `%LOCALAPPDATA%\phoneme`. Primarily for test isolation, but a real runtime override |
| `RUST_LOG` | Tracing filter for the daemon (e.g. `debug`); overrides `daemon.log_level` |
| `NO_COLOR` | When set to any value, the `phoneme` CLI disables ANSI color (same as `--no-color`) |
| `HF_HOME` | Hugging Face cache root the doctor reads to locate downloaded models |
| `PHONEME_AUDIO_BACKEND=synthetic` | Use generator source instead of CPAL (tests/CI) |

API keys are **not** environment variables — they are entered via Settings and stored DPAPI-encrypted in `config.toml`.

Hook scripts additionally receive `PHONEME_ID`, `PHONEME_AUDIO_PATH`, and `PHONEME_TRANSCRIPT` in their environment (set by Phoneme, not read from yours) — see [`[hook]`](#hook).

See [Testing & CI](testing_and_ci.md) for synthetic audio in integration tests.
