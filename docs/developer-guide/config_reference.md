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
| `timeout_secs` | u64 | `3600` | Transcription HTTP timeout — a generous flat cap, since a long recording on a slow local model can legitimately take minutes |
| `language` | string? | `null` | BCP-47 hint; omit for auto-detect |
| `initial_prompt` | string | seeded marker hint | Custom-vocabulary hint — names/jargon/acronyms to bias decoding toward (e.g. `"Phoneme, pyannote, WebView2"`). A fresh config ships a short marker hint (`"Voice memo. Common markers: Action Item:, Task:, …"`) so the default keyword hooks transcribe their triggers verbatim; an empty string disables it. Sent as the OpenAI `prompt` field on the whisper-family path (local `whisper.cpp`, `openai`, `groq`, `custom`) and as `initial_prompt` on the native path. Keep it short (Whisper conditions on ~the last 224 tokens). `deepgram`/`assemblyai`/`elevenlabs` ignore it. |
| `api_key` | string | `""` | Cloud provider key (redacted in logs) |
| `model` | string | `""` | Cloud model id |
| `api_url` | string | `""` | Custom provider base URL |
| `use_own_bundled_server` | bool | `false` | Only meaningful on `[in_place].stt` — opt a **dedicated dictation whisper-server** into supervision (see `[in_place]` below). Ignored on the main `[whisper]` block (the main server always runs). |
| `low_confidence_threshold` | f32 | `0.6` | **Confidence-driven re-do.** A recording whose **mean per-word confidence** (stored in `recordings.mean_confidence`, computed when transcription completes) falls **below** this `0..1` value is flagged *low confidence*: an amber badge in the list, a one-click **Improve…** re-transcribe, and a *Low confidence* library filter (`ListFilter.low_confidence_below`). Set `0` to disable flagging. Per-word confidence only exists for providers that return it — local `whisper.cpp` does; the `openai`/`groq` cloud transcription endpoints do **not**, so cloud transcripts (and pre-feature recordings) get a `NULL` aggregate and are never flagged. Only the main `[whisper]` block's value is read (preview/dictation blocks clone the field but ignore it). |

---

## `[preview_whisper]` (optional)

An optional, **independent** transcription provider used only for the live preview, so it never contends with the final transcription. It has the **same keys as `[whisper]`**. The final transcript always uses `[whisper]` regardless. Set a distinct `bundled_server_port` if you point it at a second local bundled model — like the main server's, it is a preference: a taken port makes the daemon fall back to a free one, and the preview never picks the main server's port.

**Auto-default when omitted (local main only).** If you omit this section *and* `[whisper]` is a **local bundled** model (`provider = local`, `mode = bundled_model`), the daemon auto-selects the **smallest local Whisper model sitting in the same directory as your final model** and runs the preview on it via a dedicated, thread-capped second server (preferred port `whisper.bundled_server_port + 1`, default `5810`). This is resolved once per config (re)load and is **in-memory only** — it is never written back to `config.toml`. It applies **only** when a model strictly smaller than the final one is present locally; if the only local model is the final one, or the final provider is cloud/external, the preview falls back to reusing the main `[whisper]` provider exactly as before. Setting `[preview_whisper]` explicitly always wins over the auto-pick. See [Live Preview & Pre-Roll](../user-guide/streaming_preview_and_preroll.md).

---

## `[recording]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `audio_dir` | path | `~/Documents/phoneme/audio` | WAV output directory |
| `sample_rate` | u32 | `16000` | Capture rate (8000–96000) |
| `channels` | u8 | `1` | 1 = mono, 2 = stereo |
| `silence_threshold_dbfs` | f32 | `-45.0` | Oneshot silence detection |
| `silence_window_ms` | u32 | `3000` | Contiguous silence to stop oneshot |
| `max_duration_secs` | u32 | `10800` | Hard cap per recording (3 hours) |
| `input_device` | string | `default` | CPAL device name |
| `source` | `microphone` \| `system_audio` | `microphone` | Single-track capture source |
| `pre_roll_ms` | u32 | `1500` | Idle mic ring buffer; `0` = off. A fresh config ships `1500`; a config file that simply **omits** the key reads as `0` (disabled), so pre-upgrade configs keep the old mic-only-while-recording behavior. |
| `streaming_preview` | bool | `false` | Live partial transcript while recording |
| `preview_adaptive` | bool | `true` | When a preview transcription tick takes longer than its interval, back the cadence off toward the tick's own cost (clamped to 8 s) so a heavy model on a weak box self-throttles instead of wedging the recording. `false` = fixed cadence. |
| `preview_reveal_words_per_sec` | f32 | `12.0` | Overlay token-bucket reveal speed — live words stream toward the latest text at this rate (with an instant snap when Whisper revises earlier words). `0` = render each update immediately (no smoothing). |
| `preview_idle_ms` | u32 | `2500` | After this long with no new preview words, the overlay label switches from **LIVE** to **LISTENING**. |
| `preview_waveform` | bool | `true` | Show the "it hears me" audio-level waveform pill in the desktop overlay. Driven by a cheap RMS loop (no whisper permit); runs for single recordings, in-place dictation, and meetings. |
| `auto_stop_on_silence` | bool | `false` | GUI Record button auto-stops on silence; `false` = manual start/stop toggle. Push-to-talk hotkey is always hold-to-record regardless. The Record button's **▾ stop-mode dropdown** (manual / silence / fixed seconds) is stored per device in the browser, not in this file — until a mode is picked there, this key decides. |
| `normalize` | bool | `false` | Peak-normalize a finished recording's gain before writing the WAV, so a quiet mic still hands transcription a healthy signal. Boost-only; silent / already-loud recordings are left untouched. Final captured recording only (single recordings + each meeting track) — not the live preview, not imported files. |
| `normalize_target_dbfs` | f32 | `-1.0` | Target peak ceiling in dBFS when `normalize` is on. `0.0` = full scale; `-1.0` leaves a hair of headroom below clipping. |

---

## `[hook]`

> **Legacy.** `commands` / `keyword_rules` / `webhook_url` are **read once** by
> the hooks-cutover migration (the `schema_version` `1 → 2` step, below), folded
> into Hook `[[playbook]]` entries on the `default` recipe, then cleared — hooks
> now live in the **Playbook**. New setups should add Hook entries there, not
> here. `run_on_transcribe` still gates whether the recipe's Hook steps fire on a
> given pass.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `commands` | string[] | `to-stdout.ps1` | *(legacy, migrated)* Always-run scripts (stdin = JSON payload) |
| `timeout_secs` | u64 | `30` | Per-hook kill timeout |
| `webhook_url` | string? | `null` | *(legacy, migrated)* Optional HTTP POST target |
| `run_on_transcribe` | bool | `true` | Fire post-transcription hooks (incl. recipe Hook steps) — off skips them on re-transcribe |
| `keyword_rules` | array | `[]` | *(legacy, migrated)* `{ pattern, command, case_sensitive? }` |

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
| `allow_private_network` | bool | `false` | Allow non-loopback private targets — RFC1918, v4 link-local `169.254/16`, CGNAT `100.64/10`, IPv6 ULA, IPv6 link-local (e.g. n8n on a NAS) |
| `allow_http` | bool | `false` | Allow plain `http://` for **public** targets; otherwise public targets must be `https://` |
| `hmac_secret` | string (secret) | `""` | Shared secret for HMAC-SHA256 signing of the POST body. Non-empty adds an `X-Phoneme-Signature: sha256=<hex>` header (HMAC over the exact body bytes) so the receiver can verify authenticity. Encrypted at rest (DPAPI), masked in the UI; empty = signing off. |
| `custom_headers` | table | `{}` | Extra `name = "value"` headers on every webhook POST (e.g. `Authorization`). Entries colliding with a header Phoneme controls (`Content-Type`, the signature header) are ignored. Header **values** are encrypted at rest (DPAPI) and masked in the UI, since they often carry credentials. |
| `max_retries` | int | `2` | Retries (after the first attempt) for a failed webhook POST, with exponential backoff (~250 ms → 2 s cap). Retries only **transient** failures — a timeout, connection error, HTTP 429, or 5xx; a 4xx (the receiver refusing) and an SSRF-policy block fail immediately. `0` disables retries. |

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
| `strip_titlebar` | `false` | Custom window chrome (no OS title bar). Turning it **on** applies live; turning it back **off** needs an app restart on Windows — the chrome mode is set once at Tauri init. |
| `format_24h` | `false` | 24-hour timestamps |
| `date_day_first` | `false` | Day column shows `DD/MM` instead of `MM/DD` |
| `theme` | `catppuccin-mocha` | CSS theme id. Dark: `catppuccin-mocha`, `catppuccin-macchiato`, `catppuccin-frappe`, `dracula`, `everforest`, `gruvbox`, `kanagawa`, `nord`, `one-dark`, `rose-pine`, `tokyo-night`. Light: `catppuccin-latte`, `gruvbox-light`, `rose-pine-dawn`, `solarized-light`, `tokyo-night-day`. (Defined in `frontend/src/styles/theme.css`.) |
| `visible_columns` | day, time, duration, status, transcript | List columns |
| `column_widths` | px/fr strings | Resizable column layout |
| `preview_overlay` | `false` | Float the live preview in a system-wide, always-on-top overlay window (requires `recording.streaming_preview`) |
| `recording_indicator` | `false` | Show a minimal, always-on-top "recording indicator" pill while recording — only a pulsing record dot, an audio-reactive waveform, and an mm:ss elapsed timer (no caption text). A separate, independent window from `preview_overlay`; needs no streaming preview, so it works even with live preview off. Either, both, or neither can run. |
| `vim_nav` | `false` | System-wide vim-style keyboard navigation (`h`/`l` across panes, `j`/`k` within the list, `gg`/`G`, `i`/`Enter`, `Esc`). Distinct from `editor.vim_mode`, which only affects the transcript editor. |
| `arrow_nav` | `false` | Arrow-key navigation for non-vim users — `←`/`→`/`↑`/`↓` drive the same pane/grid cursor as `vim_nav`, `Enter` activates, `Esc` steps out. Independent of and combinable with `vim_nav` (bare `h`/`j`/`k`/`l` stay vim-only). Opt-in so an upgrade never changes what the arrow keys do; surfaced in the wizard and Settings → Appearance. |
| `animation_speed` | `normal` | Pane show/hide animation speed: `off` \| `fast` \| `normal` \| `slow`. `off` makes sidebar / detail-pane / focus-mode toggles instant. |
| `cursor_animation` | `off` | Animate the roving keyboard cursor (the `.kbd-cursor` highlight) as it jumps between controls: `off` \| `glide` (a translucent accent glow slides to the new control) \| `smear` (glide + a brief streak on bigger jumps) \| `trail` (a streak on every move). Inspired by smear-cursor.nvim / mini.animate. Purely cosmetic & frontend-only; honors the OS "reduce motion" setting regardless. |
| `ui_font` | `""` (Inter) | Base interface font family — a single CSS family name (e.g. `Segoe UI`, `JetBrains Mono`) prepended to the bundled Inter fallback stack, so an uninstalled font falls back cleanly. Empty = the bundled Inter stack. Transcript/code blocks keep their own monospace. Frontend-only. |
| `ui_font_size` | `14` | Base interface font size (u8), clamped to 10–24. The UI scales from this real **root** font-size — it is not a zoom. Frontend-only; the daemon never reads it. |
| `step_notifications` | `true` | Toast a note as each pipeline step finishes (transcribed, cleaned up, summarized, tags suggested) and when a recording is fully ready. Failure toasts always show regardless — a silently lost transcription is never the right default. |
| `quit_stops_daemon` | `true` | Tray **Quit** also shuts the daemon down: an in-flight recording is stopped and queued first, then the whisper-server(s) and a Phoneme-launched Ollama go with it. `false` = the daemon outlives the tray (headless setups). Also read at daemon **spawn** time to decide whether the tray ties the daemon's lifetime to its own at the OS level (kill-on-close job) — that part of a change applies on the next spawn. |

---

## `[editor]`

| Key | Default | Description |
|-----|---------|-------------|
| `vim_mode` | `false` | Vim bindings in transcript editor |
| `vimrc` | `""` | Inline vimrc |
| `vimrc_path` | `""` | External vimrc file |
| `resync_views_on_edit` | `true` | On a transcript edit + save, re-flow the per-word / per-segment timing onto the new text so the **Synced** and **Timeline** views follow the edit. `false` keeps the original timings. |

---

## `[diarization]`

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `none` | `none` \| `local` \| `deepgram` \| `assemblyai` |
| `models_dir` | `""` | Folder holding a custom speakrs diarization bundle (segmentation + embedding ONNX) to load **instead of** the pretrained models. Empty = auto-download/use the defaults. Changing it reloads the cached pipeline. |
| `local_model_path` | `""` | **Deprecated / unused** — superseded by `models_dir`; it was never wired into the load path. Kept so older configs keep parsing. |
| `solo_one_speaker` | `false` | Treat a single (non-meeting) recording as ONE speaker — skip diarization for it so it never splits into `[Speaker N]` turns. Off by default. For when the local diarizer hears two voices in a one-person note (a big tonal shift, or background audio). Meetings and genuinely multi-speaker files are unaffected. Local diarization path. |
| `expected_speakers` | _(unset)_ | Expected speaker count, used as a *prior* on the auto-detected count. Unset (or `0`) = trust whatever the model clusters (today's behavior). Set to `n` and, if the local pipeline finds **more** than `n` speakers, the closest clusters are greedily merged (by voiceprint cosine) until exactly `n` remain. It never splits — finding `≤ n` is left untouched, since the prior means "no more than this many voices". speakrs has no native target-count knob, so this is enforced as a post-clustering merge; cloud providers ignore it. Use when you know the headcount (a 1:1 call, a fixed panel) and the model over-splits one voice into several. |
| `merge_gap_secs` | `0.25` | Gap (seconds) below which adjacent same-speaker turns are merged into one. Lower = more, shorter turns; higher = fewer, longer turns. Local diarization path. |
| `speaker_keep_threshold` | `1e-7` | Speaker-cluster keep threshold — clusters with weaker presence than this are dropped. speakrs' own default; raise it to suppress spurious extra speakers, lower it to keep faint ones. |
| `reconstruct_method` | `smoothed` | Turn-boundary reconstruction: `standard` (hard boundaries) or `smoothed` (softened by `reconstruct_method_epsilon`). Stored as a plain string. |
| `reconstruct_method_epsilon` | `0.1` | Smoothing strength for `reconstruct_method = "smoothed"`, in [0, 1] (speakrs' default). Ignored when the method is `standard`. |
| `preload_at_startup` | `false` | Warm the local diarization models at daemon startup instead of lazily on the first recording that needs them. Off by default so users who keep diarization off (or rarely diarize) don't pay the ~500 MB RAM up front; turn it on to trade that memory for a fast first diarized recording. No-op for `none`/cloud providers. |
| `recognize_speakers` | `true` | Capture a voiceprint per diarized speaker and match it against the names you've assigned before, suggesting who they are when you open a recording. Naming a speaker enrolls their voice into a cross-recording library. Local diarization only. |
| `voiceprint_match_threshold` | `0.5` | Cosine-similarity bar (0–1) a voiceprint must clear to be suggested as a known speaker. Higher = stricter (fewer false matches, more misses). Tune to your own recordings. Used when `voiceprint_score_norm = "off"`. |
| `voiceprint_score_norm` | `off` | Score normalization for speaker matching (V2). `off` (default) compares the raw cosine against `voiceprint_match_threshold` — byte-for-byte the previous behavior. `s_norm` / `as_norm` z-score each comparison against the *other* enrolled voices (the cohort), so one threshold means the same thing across speakers and sessions instead of drifting with how "central" a voice is. `s_norm` normalizes the probe side only; `as_norm` is symmetric (averages probe-side and target-side z-scores). Cohort = the named-voice library; a cohort of one degrades gracefully to the raw score. When on, the bar used is `voiceprint_score_norm_threshold`. Local diarization only; no Settings UI yet (config-only, follow-up). |
| `voiceprint_score_norm_threshold` | `2.0` | Z-score bar a normalized match must clear when `voiceprint_score_norm` is `s_norm`/`as_norm`. This is standard-deviations above the probe's cohort mean — a different scale than the cosine bar, so typical values are ~1.5–3.0, not 0–1. Ignored when norm is `off`. Tune with the EER harness against your own enrolled voices. |
| `name_propagation` | `ask` | What to do with PAST recordings when you name a speaker (V5 back-fill). Naming a speaker already enrolls their voice; this controls whether the name is also applied to the *same* voice where it appears **unnamed** in other recordings. `ask` (default) gathers the matching unnamed speakers and returns them so the UI can confirm — nothing past is changed automatically. `auto` back-fills every match immediately. `off` never back-fills (naming only affects the recording you named in). Candidates are the unnamed speakers at/above `voiceprint_match_threshold` (the same bar recognition uses, score-norm aware); an already-named speaker is never overwritten. Local diarization only; the confirm prompt and a "don't ask again → switch to `auto`" toggle are a Settings-UI follow-up (config-only for now). |

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
| `timeout_secs` | `300` | LLM HTTP timeout — generous, since an LLM cleaning a long transcript can take minutes; streaming providers bound idle time rather than total, so a slow-but-progressing local model never trips it |
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
| `meeting_preview` | `"toggle"` | How the live preview handles a meeting's two tracks (needs `streaming_preview`). `"toggle"` — one preview loop follows a single track; the overlay's 🎤/🔊 button switches it (same cost as a single recording). `"both"` — one loop per track, captions stacked in the overlay (the window grows to two lines). By default both loops **alternate** on the single preview server (each track at ~half rate); set `meeting_preview_own_server` to stream them concurrently. Validated to `"toggle"`/`"both"` at load. |
| `meeting_preview_own_server` | `false` | Meeting `"both"` mode only: spawn a **second** live-preview whisper-server so the two tracks caption **concurrently** instead of alternating. Reuses the `[preview_whisper]` model on a derived port (preview port + 2, default `5812`). ⚠️ Keeps a second model resident and runs a second concurrent transcription — opt-in for capable machines only. Takes effect only with `streaming_preview` + `meeting_preview = "both"` + a dedicated **local** preview server (`[preview_whisper]` local bundled). |

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
| `save_to_library` | `true` | Keep the dictation in the library: after typing, the transcript, segments, and embeddings persist like any recording. `false` = ephemeral — the row and audio are deleted once typed. |
| `type_mode` | `"type"` | `"type"` = simulated keystrokes; `"paste"` = clipboard + Ctrl+V with the previous clipboard restored (near-instant for long text). |
| `stream_type` | `false` | **Experimental.** Type dictated words live as they finalize (the streaming preview's committed words, first letter capitalized), then reconcile to the accurate final transcript on stop with a minimal backspace + retype (`dictation::reconcile_edit`). Only clean forward extensions are typed mid-stream (never a backspace), so the cursor doesn't churn. Only honored with `type_mode = "type"`; forces the preview loop on even when the visible caption is off. Reads best with a fast `[preview_whisper]` model. |
| `app_overrides` | `{}` (empty) | Per-app delivery overrides, a table keyed by the foreground app's **lowercased executable stem** (e.g. `code` for `Code.exe`, `chrome`) — matched case-insensitively against the window focused when you stop speaking. Value is `"type"`, `"paste"`, or `"off"` (don't auto-insert text for that app; the dictation still saves). An unlisted app uses `type_mode`. Empty = every app uses `type_mode` (unchanged behavior). Windows-only — other platforms can't detect the foreground app and always fall back to `type_mode`. |
| `app_context` | `false` | Opt-in: include the **focused window's title** in the AI-cleanup prompt (only when `cleanup = "llm"`) so polish can adapt to what you're working in. **Privacy:** the title can be sensitive (a document name, an email subject) and, when this is on, it is **sent to your configured cleanup provider** (prefer a local LLM if that matters); it is never logged or persisted. When off (default) the title is never even read. |
| `app_context_denylist` | `[]` (empty) | Apps (lowercased executable stems) whose window titles are **never** read for context, even when `app_context` is on — e.g. a password manager or a banking app. |
| `voice_commands_enabled` | `true` | Master switch for the spoken editing-command pass. When `false`, no phrase is ever interpreted (they are typed literally), regardless of `voice_commands` — lets you turn the feature off without clearing a customized map. |
| `voice_commands` | `{}` (empty) | User-editable spoken editing commands: a table of **phrase → action**, where the action is `"newline"` (a single line break), `"paragraph"` (a blank line), or `"scratch"` (retract the immediately preceding dictated sentence). A command only fires when said as its own segment (the boundary rule), so a literal "on a new line of code" stays as written. **Empty = the built-in set** (`new line`/`newline` → newline, `new paragraph` → paragraph, `scratch that`/`delete that` → scratch), so an absent/empty table reproduces today's behavior byte-for-byte. A **non-empty** table fully *replaces* the built-ins (it is not merged) — copy the defaults in to extend them. Phrases are lowercased on load (so a cased entry still matches). Entries with an unrecognized action are dropped on load with a warning (the config still loads — a typo'd action never hard-fails startup). Honored in every cleanup mode; under `cleanup = "llm"` the active phrases are described to the cleanup LLM so a customized map is respected there too. |
| `full_pipeline` | `false` | Route dictations through the normal queue and every configured step (cleanup, summary, tags, hooks) — the legacy behavior. `type_first` picks when the text is typed. |
| `type_first` | `false` | Only meaningful with `full_pipeline`. `true` = a type-only fast pass types the quick transcription immediately while the pipeline continues in the background for the library copy (the typed text is the fast polish, not the LLM cleanup, and the pipeline skips its own end-of-run typing). `false` = the typed text waits for, and includes, every configured step. |
| `stt` | *(unset)* | Optional dedicated STT provider table shaped like `[whisper]`. Unset: dictation follows the Live Preview's provider when the preview is enabled, else `[whisper]`. For a local model you can point it at an already-running server (the daemon reuses it), **or** set `stt.use_own_bundled_server = true` to have the daemon supervise a **dedicated third whisper-server** just for dictation — its own process and model on its own port, so dictation is never starved by a main-server restart or model override. Default off (the weak-box default reuses the main/preview server); opt in via Settings → Capture → Dictation → "Dedicated dictation server". This is a power-user / multi-server option: a third local model means materially more RAM. Note: dictation still waits on the shared whisper permit, so the dedicated server buys reliability/isolation, not parallelism with final transcription. |


## `[filler]`

Tuning for the **deterministic** filler-word transform — the non-LLM step. A `[[playbook]]` entry of `kind = "filler_removal"` (the seeded `filler_removal` entry, off by default) strips these from the transcript in pure Rust: no provider, no network, instant and repeatable. Add it to a recipe (or a Custom Hotkey) to use it.

| Key | Default | Meaning |
|-----|---------|---------|
| `words` | `["um", "uh", "er", "ah", "hmm", "mhm"]` | Single filler words removed at word boundaries, case-insensitively (matched against each token's alphanumeric core, so `umbrella` survives). Conservative on purpose — every default is meaningless filler in any context. Replace the list to customize; an empty list removes no single words. |
| `phrases` | `["you know", "i mean", "sort of", "kind of", "like"]` | Multi-word filler phrases, removed as whole-word units — **but only when `aggressive` is on.** These double as real speech (`I `**`like`**` it`, **`kind of`**` blue`), so they are opt-in. |
| `aggressive` | `false` | Off (the safe path): only `words` are stripped. On: `phrases` are stripped too — more thorough, at the risk of removing a meaning-bearing `like` / `kind of`. |

---

## `[[playbook]]`

Reusable AI "moves" — the building blocks the recording pipeline and Custom Hotkeys run. An array-of-tables: each `[[playbook]]` block is one entry. Curated `builtin` entries (`cleanup`, `title`, `summary`, `auto_tag`) are seeded into a fresh config and are editable; users add their own. The Playbook is the **source of truth** for the whole post-transcription pipeline — the built-in entries drive each LLM step (replacing the legacy `[llm_post_process]` / `[title]` / `[summary]` / `[auto_tag]` sections), and `hook` entries run shell/webhook side-effects (replacing the legacy `[hook]` config). Both are migrated once, tracked by `schema_version` (below). Edited in Settings → 🎭 Playbook.

| Key | Default | Description |
|-----|---------|-------------|
| `id` | *(required)* | Stable unique id — what recipes and hotkeys reference (e.g. `cleanup`). Minted once; not user-editable. |
| `name` | `""` | User-facing name shown in the Playbook manager |
| `description` | `""` | One-line "what this does" hint |
| `builtin` | `false` | Seeded by Phoneme (editable; "Reset to default" restores the seed) vs. user-created |
| `kind` | `transform` | `transform` (LLM step that **rewrites** the running transcript text, feeding the next step), `filler_removal` (a **deterministic, non-LLM** rewrite — strips filler words per `[filler]`; the `llm` half is ignored), `enrichment` (LLM step that writes a named field — see `target`), or `hook` (a shell command / webhook) |
| `target` | `""` | For `enrichment` only: the field to write — `title` \| `summary` \| `tags` \| `entities` \| `custom:<key>`. `entities` runs LLM **entity extraction**, storing structured, typed entities (`person`/`org`/`topic`/`term`) in the `entities` child table (richer than the flat `tags`). A `custom:<key>` target has no backing store yet (the step warns and no-ops). Ignored for non-enrichment kinds. |
| `input` | `previous` | For a `transform` step: which transcript it reads — `previous` (the running text, so chained transforms **compound**, each refining the last toward a "perfect" transcript) or `base` (the raw transcription, an independent pass off the original, ignoring earlier steps). Ignored for non-transform kinds. |
| `llm.provider` | `""` | For `transform` / `enrichment`: provider id (`ollama` / `openai` / `groq` / `anthropic`); empty inherits the default `[llm_post_process]` connection |
| `llm.model` | `""` | Empty inherits the provider's configured default |
| `llm.prompt` | `""` | The step's system/instruction prompt |
| `llm.api_url` | `""` | Override base URL; empty uses the provider default |
| `llm.api_key` | `""` | Per-entry key, **DPAPI-encrypted at rest** and inherited at run time when blank. Like every other key field it is **never exported to the WebView** — it is masked (replaced with a sentinel) in any config the UI sees, and restored from disk on save. |
| `llm.timeout_secs` | `30` | Idle-based LLM HTTP timeout for this step |
| `hook.command` | `""` | For `hook` only: shell command / script (receives the recording JSON on stdin) |
| `hook.webhook_url` | `""` | For `hook` only: webhook URL to POST the recording payload to (governed by `[webhook]` policy) |
| `hook.timeout_secs` | `60` | For `hook` only: max execution time before the hook is killed |
| `hook.keyword` | `""` | For `hook` only: **trigger** — run only when the (post-processed) transcript contains this substring; empty = always run |
| `hook.case_sensitive` | `false` | For `hook` only: case-sensitive `keyword` matching (ignored when `keyword` is empty) |
| `hook.required` | `false` | For `hook` only: when `true`, a hook failure (non-zero exit / webhook error) **fails the recording**; default surfaces it but is non-fatal |

---

## `[[recipes]]`

Named, ordered chains of `[[playbook]]` entry ids — what the default recording pipeline and Custom Hotkeys actually run. An array-of-tables. The `default` recipe is the normal-recording pipeline (cleanup → title → summary → auto-tag); a Custom Hotkey can point at any other recipe.

| Key | Default | Description |
|-----|---------|-------------|
| `id` | *(required)* | Stable unique id (the normal-recording pipeline is `default`) |
| `name` | `""` | User-facing name |
| `description` | `""` | One-line description |
| `builtin` | `false` | Seeded by Phoneme vs. user-created |
| `steps` | `[]` | Ordered list of `[[playbook]]` entry ids to run — `transform`/`enrichment` (LLM) **and** `hook` (shell/webhook) steps, in order. A dangling id (entry deleted) is skipped with a warning; an empty list is a bare transcribe-only run. |

---

## `schema_version`

| Key | Default | Description |
|-----|---------|-------------|
| `schema_version` | `0` | One-time-migration version (top-level integer). Records how many config migrations have already run on this file, so Phoneme runs each migration **exactly once**. On load it runs every staged migration whose version step is newer than the stored value, in order, then writes the current version back; reloading an already-migrated config does nothing. Version steps: `0 → 1` = the Playbook reconcile (copies your LIVE `[llm_post_process]` / `[title]` / `[summary]` / `[auto_tag]` values into the matching built-in `[[playbook]]` entries and rebuilds the `default` recipe from the legacy enable flags); `1 → 2` = the hooks cutover (folds the legacy `[hook]` `commands` / `keyword_rules` / `webhook_url` into Hook `[[playbook]]` entries on the `default` recipe and clears the `[hook]` fields, so a hook fires once per transcribe — never twice). Idempotent — leave it alone. |

### Deprecated: `playbook_migrated` · `hooks_migrated`

These two top-level booleans were the old per-feature migration latches. They are **superseded by `schema_version`** and are now **deprecated**: an existing config that still has them keeps loading (Phoneme reads them **once** to infer the correct starting `schema_version` — `playbook_migrated && hooks_migrated` → already current; `playbook_migrated` only → version 1; neither → version 0), but Phoneme no longer writes them. They disappear from your `config.toml` the next time it is saved, replaced by `schema_version`. Nothing to do — the migration is automatic and runs no migration twice.

---

## `[[hotkeys]]`

Custom keybinds beyond the three built-ins (`[hotkey]` / `[in_place_hotkey]` / `[meeting_hotkey]`). An array-of-tables; each binds a combo to an action and carries its own pipeline. Only the Playbook-era additions are shown here.

| Key | Default | Description |
|-----|---------|-------------|
| `recipe_id` | `""` | The `[[recipes]]` id this binding's recordings run. Empty = the global `default` recipe (today's normal pipeline), so a pre-Playbook binding is unchanged. A deleted recipe falls back to `default`. **Ignored when `action = "meeting"`** — a meeting resolves its recipe per-track via the daemon's multi-track path, not the single-recording ledger. Supersedes the legacy `pipeline` flags. |
| `whisper_model` | `""` | Per-keybind transcription (STT) model override. Empty uses the globally configured model; a non-empty value transcribes this binding's recordings with that model (a local model-file path, or a cloud model id — same shape as the per-job retranscribe override). **Ignored when `action = "meeting"`** for the same reason as `recipe_id`. |
| `source` | _(unset)_ | Per-keybind capture-source override: `"microphone"` or `"system_audio"`. Unset (the default) follows the global `[recording].source`, so existing bindings are unchanged. Lets one hotkey record the mic and another record system audio. The source actually used is stored on each recording's `track` and shown in the list's **Source** column. **Ignored when `action = "meeting"`** — a meeting always records both tracks. |

---

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

### `[semantic_search.ann]` — optional approximate-nearest-neighbour index

An optional [usearch](https://github.com/unum-cloud/usearch) HNSW index that
replaces the brute-force cosine scan with sub-linear nearest-neighbour search on
large libraries. It is gated **twice** and **off by default**:

1. **Build-time:** the cargo feature `ann-usearch` (in `phoneme-core`, **not** in
   any default feature set) must be compiled in. A stock binary contains zero
   usearch native code and ignores every key in this table. Build/test the
   feature lane with:

   ```sh
   cargo build  -p phoneme-core --features ann-usearch
   cargo clippy -p phoneme-core --features ann-usearch -- -D warnings
   cargo test   -p phoneme-core --features ann-usearch
   ```

   The default lanes (no `--features`) must stay green too — that is the shipped
   build, and the brute-force scan is its guaranteed behaviour.
2. **Run-time:** `enabled` below must be `true`.

When both are on, the index only narrows *which* candidates are scored; the exact
cosine re-score, meeting-dedupe, and RRF fusion are unchanged, so displayed
scores are bit-identical to brute force and the worst the index can do is miss a
tail result (tunable via `oversample` / `expansion_search`). Any index trouble
(missing/stale sidecar, dimension mismatch, count drift, query error) logs a
`warn` and falls back to the brute-force scan — search never errors. The on-disk
index lives in a disposable sidecar next to the catalog (`catalog.ann`),
rebuilt from SQLite whenever it is absent or fails its integrity check; the
`phoneme doctor --rebuild-catalog` path deletes it so a rebuild re-derives it.

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Master switch. `false` = brute-force cosine scan (the guaranteed fallback). No effect unless built with the `ann-usearch` feature. |
| `oversample` | `5` | `k = limit * oversample` chunk neighbours fetched before the exact re-score and meeting-dedupe collapse them, so the final top-`limit` survives. Higher trades a little query cost for recall. |
| `connectivity` | `16` | HNSW graph connectivity (M). Higher = better recall, more memory/build time. |
| `expansion_add` | `128` | HNSW build-time candidate-list width (efConstruction). Higher = better graph quality at the cost of build time. |
| `expansion_search` | `64` | HNSW query-time candidate-list width (efSearch). The main recall/latency knob: higher recovers more true neighbours per query. |

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
