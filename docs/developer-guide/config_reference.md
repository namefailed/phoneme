# Configuration Reference (`config.toml`)

Location: `%APPDATA%\phoneme\config.toml` (expanded from `~/` paths on load).

Validate: `phoneme config validate` · Reload: `phoneme config reload` or IPC `reload_config`.

Schema source: `crates/phoneme-core/src/config.rs`.

---

## `[whisper]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mode` | `external` \| `bundled_model` \| `bundled_download` | `bundled_download` | How local whisper-server is provisioned |
| `provider` | `local` \| `openai` \| `groq` \| `deepgram` \| `assemblyai` \| `elevenlabs` \| `custom` | `local` | Transcription backend |
| `external_url` | string | `http://127.0.0.1:5809` | OpenAI-compatible server base URL |
| `model_path` | path | `""` | GGUF path when `mode = bundled_model` |
| `bundled_server_port` | u16 | `5809` | Local server port |
| `bundled_server_args` | string[] | `[]` | Extra whisper-server CLI args |
| `timeout_secs` | u64 | `60` | Transcription HTTP timeout |
| `language` | string? | `null` | BCP-47 hint; omit for auto-detect |
| `api_key` | string | `""` | Cloud provider key (redacted in logs) |
| `model` | string | `""` | Cloud model id |
| `api_url` | string | `""` | Custom provider base URL |

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
| `pre_roll_ms` | u32 | `1500` | Idle mic ring buffer; `0` = off |
| `streaming_preview` | bool | `false` | Live partial transcript while recording |

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
| `log_level` | `info` | `trace` … `error` |
| `log_max_size_mb` | `10` | Rotation size |
| `log_max_files` | `5` | Retained rotations |
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

---

## `[semantic_search]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Index new transcripts |
| `model_dir` | `""` | ONNX model + tokenizer directory |

---

## `[retention]`

| Key | Default | Description |
|-----|---------|-------------|
| `max_age_days` | `null` | Delete older than N days |
| `max_count` | `null` | Keep newest N recordings |
| `delete_audio` | `false` | Drop WAV, keep catalog row |

---

## Config profiles

Named copies under `%APPDATA%\phoneme\profiles\`. Switch via tray menu. See [Config Profiles](../user-guide/config_profiles.md).

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `PHONEME_AUDIO_BACKEND=synthetic` | Use generator source instead of CPAL (tests/CI) |
| `PHONEME_CONFIG` | Override config file path (if supported by binary) |

See [Testing & CI](testing_and_ci.md) for synthetic audio in integration tests.
