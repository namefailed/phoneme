# Settings Overview

Phoneme stores all preferences in `%APPDATA%\phoneme\config.toml`. The Settings UI is a visual editor for that file. Changes apply after **Save**; the daemon hot-reloads on save.

Open Settings from the cog icon in the header or **Tray → Settings**.

The left sidebar lists every section; a **search box** at the top jumps to any field across all sections — it matches field labels and **model names** too — with a live results count and a breadcrumb back to each result's home tab.

| Tab | What it contains |
|-----|------------------|
| 🎨 **Appearance** | Theme, interface font & size, animation speed, transcript editor (incl. Vim) |
| 🗣️ **Transcription** | Whisper / transcription provider + model |
| 🎙️ **Capture** | Recording device, capture source, auto-stop on silence, pre-roll, streaming preview |
| ⌨️ **Dictation** | In-place dictation (type-at-cursor) options |
| 👁️ **Live Preview** | Live-preview provider + system-wide overlay |
| ✨ **Post-Processing** | AI cleanup, Auto Summary, Auto-tagging |
| 🎭 **Playbook** | Recipes — the ordered chains of cleanup / enrichment / **hook** steps |
| 🪝 **Integrations** | Inbound REST & MCP; global Outbound webhook policy |
| ⚡ **Hotkeys** | Built-in + custom global shortcuts (per-hotkey recipe / model / source) |
| 👥 **Diarization** | Speaker diarization backend |
| 🏷️ **Tags** | Create, rename, recolor, delete, merge tags |
| 🔍 **Search** | Saved searches + the semantic-search embedding model |
| 👤 **Profiles** | Named full-config snapshots |
| ⚙️ **System** | Storage & retention, Tray, Advanced (+ Doctor) |

---

## 🗣️ Transcription

![Whisper settings](../screenshots/settings-whisper.png)

| Area | What it controls |
|------|------------------|
| **Provider** | Local whisper.cpp, OpenAI, Groq, Deepgram, AssemblyAI, ElevenLabs, or custom OpenAI-compatible endpoint |
| **Model manager** | Download GGML sizes (tiny → large-v3); hardware recommendation badge |
| **Language** | BCP-47 hint (`en`, `es`, …) or auto-detect |
| **Bundled server** | Port, model path, extra server args when running the local whisper-server |
| **Timeout** | How long to wait for transcription before giving up |

See [Providers & Models](providers_and_models.md) and [Whisper & Diarization](diarization_and_whisper.md).

## 🎙️ Capture

![Recording settings](../screenshots/settings-recording.png)

| Area | What it controls |
|------|------------------|
| **Audio directory** | Where `.wav` files are stored (default `~/Documents/phoneme/audio`) |
| **Input device** | Microphone selection (or `default`) |
| **Audio source** | Microphone vs. system-audio (loopback) capture — the global default a hotkey can override per-binding |
| **Auto-stop on silence** | Whether the Record button auto-stops on a quiet mic (off = manual start/stop toggle) |
| **Silence threshold / window** | dBFS level and duration for silence auto-stop |
| **Max duration** | Hard cap per recording (seconds) |
| **Pre-roll** | Milliseconds of idle mic buffer prepended on record start (anti-clip; microphone source only) |
| **Streaming preview** | Live partial transcript while recording (opt-in) |

The **Audio source** here is the global default. Each custom hotkey can capture a different source — see [Hotkeys](#-hotkeys) below and [Hotkeys & Recording Modes](hotkeys_and_recording_modes.md).

## ⌨️ Dictation

In-place dictation transcribes and types (or pastes) the result straight at the system cursor, skipping the library queue by default. This section controls the fast lane: the cleanup mode (instant rule-based / an LLM / none), **type vs. paste vs. off** insertion, whether to run the full recipe first (`full_pipeline`), and *when* the text lands (`type_first`). See [Transcribe in Place](transcribe_in_place.md).

## 👁️ Live Preview

An independent transcription provider just for the live partial-transcript preview, so it never contends with the final transcription. Leave it unset to reuse the main provider, or point it at a small/fast local model on its own server or a fast cloud API (Groq, OpenAI, Deepgram). A **System-wide overlay** checkbox additionally floats the live caption in an always-on-top window over the whole desktop. See [Live Preview & Pre-Roll](streaming_preview_and_preroll.md).

## ✨ Post-Processing

![Post-processing settings](../screenshots/settings-post-processing.png)

- **AI Post-Processing (cleanup):** LLM cleanup after Whisper, with one-click presets for many local and cloud providers, a live model picker, preset prompts, and a request timeout.
- **Auto AI Summary:** optional per-recording summary with its own provider/model/prompt (or inherit the cleanup connection).
- **Auto-Tagging:** let the AI **suggest tags** for each new transcript — it prefers your existing tags and proposes new ones only when nothing fits; every suggestion waits as a dashed ✨ chip until you approve or dismiss it. Provider/model (blank inherits the cleanup connection), a suggestion cap, and tunable instructions. See [Auto-Tagging](auto_tagging.md).

> [!NOTE]
> Post-transcription **scripts and webhooks are no longer here** — they're **Hook steps in the 🎭 Playbook** (below). The Post-Processing section is the LLM steps only.

See [Smart Cleanup](smart_cleanup.md) and [Providers & Models](providers_and_models.md).

## 🎭 Playbook

The Playbook is where post-transcription processing lives. A **recipe** is one ordered chain of steps:

- **Transform** — rewrites the transcript in place (e.g. cleanup, formalize, bulletize).
- **Enrichment** — derives metadata: a **title**, a **summary**, or **tags**.
- **Hook** — runs a side-effect: a **shell command** and/or an **outbound webhook**, optionally **keyword-gated** (only fire when the transcript contains a phrase) and optionally flagged **"fail the recording"** (by default a failed hook is surfaced but non-fatal).

Each step reads its own provider/model/prompt, so a recipe is fully self-contained. The built-in **`default` recipe** runs for normal recordings; a custom **Hotkey** can point at any other recipe so that combo's recordings run a different chain. A one-time migration folded any legacy `[hook]` config into Hook entries on the `default` recipe.

See [Plugins & Hooks](../developer-guide/plugins_and_hooks.md) for the full hook + recipe model.

## 🪝 Integrations

Two halves, both off until you turn them on:

- **Inbound (REST & MCP):** enable the `phoneme-rest` loopback HTTP/SSE bridge (`127.0.0.1` only) and pick its port; plus an info card for the `phoneme-mcp` Model Context Protocol stdio server. Lets scripts and AI clients drive Phoneme.
- **Outbound (webhook policy):** the **global policy** that governs every webhook a Playbook Hook makes — the SSRF guard (allow private network), allow insecure HTTP, an HMAC signing secret, and custom headers. The hooks themselves live in the 🎭 Playbook.

## ⚡ Hotkeys

![Hotkey settings](../screenshots/settings-hotkey.png)

The three **built-in** global combos — record, transcribe-in-place, and meeting mode — each Hold or Toggle. **Custom Hotkeys** binds any number of extra global shortcuts; expand a Record or in-place hotkey's **▸ Recipe & options** to give it its own:

- **Recipe** — the Playbook chain that hotkey's recordings run.
- **Whisper model** — a per-hotkey transcription model.
- **Audio source** — Default (the global Capture source) / Microphone / System audio (loopback) — Windows.

The audio-source override is ignored for **Meeting** hotkeys — a meeting always records both the mic and system tracks. See [Hotkeys & Recording Modes](hotkeys_and_recording_modes.md).

## 👥 Diarization

Speaker diarization backend: `none`, local ONNX, Deepgram, or AssemblyAI, plus an optional **Models folder** for a custom local bundle. A live warning flags a mismatch (cloud diarization picked but a different provider transcribes). Once a recording is diarized you can rename each `Speaker N` to a real name (persisted per recording, applied across every view). With **Recognize known speakers** on, naming a speaker enrolls their voice so later recordings suggest who they are; the **Speaker Library** here lists, renames, merges, and forgets those voices, and a **Recognition threshold** (Advanced) tunes how strict the match is. See [Whisper & Diarization](diarization_and_whisper.md) and [Named speakers](search_and_organization.md#-named-speakers).

## 🎨 Appearance

![Interface settings](../screenshots/settings-interface.png)

Theme, 24-hour time, visible list columns (reorderable / toggleable), column widths, title-bar stripping, vim navigation, and **animation speed** for pane show/hide (Off / Fast / Normal / Slow — Off makes the sidebar, detail-pane, and focus-mode toggles instant).

- **Theme** — pick from a grouped list of faithful ports of established palettes. **Dark:** Catppuccin Mocha (default), Catppuccin Macchiato, Catppuccin Frappé, Dracula, Everforest, Gruvbox, Kanagawa, Nord, One Dark, Rosé Pine, Tokyo Night. **Light:** Catppuccin Latte, Gruvbox Light, Rosé Pine Dawn, Solarized Light, Tokyo Night Day.
- **UI font** (`interface.ui_font`) — a CSS font-family name (e.g. `Segoe UI`, `JetBrains Mono`); leave it empty to use the bundled default. An uninstalled choice falls back cleanly to the default stack.
- **UI font size** (`interface.ui_font_size`) — the interface text size in px (10–24, default 14). The whole UI scales from this real root font-size — it's not a zoom, so spacing and boxes stay crisp.

**Strip title bar:** removing the OS title bar applies live. Turning it back **on** needs an app restart on Windows.

![Editor settings](../screenshots/settings-editor.png)

Optional Vim keybindings (with inline or external `vimrc`) for the transcript editor.

> [!TIP]
> A few handy shortcuts from the `?` cheat-sheet: **g A** toggles the AI-activity panel (the floating brain/FAB, whose log persists across restarts), **Ctrl+D** toggles the detail pane (an alias of **Ctrl+\\**), and **Shift+Esc** leaves the Notes editor (just like the transcript editor).

## 🏷️ Tags

Create, rename, recolor, delete, and merge tags (the quick Tag Manager popup is still on `Shift+T` / `g T`). See [Search & Organization](search_and_organization.md).

## 🔍 Search

- **Saved searches** — the full saved-search manager: apply, rename, update to the current filters, delete (`g S` jumps here). A saved search captures the *complete* filter state. See [Search & Organization](search_and_organization.md).
- **Semantic Search** — enable meaning-based search, set the embedding model directory and its knobs (max tokens, pooling, `token_type_ids`, query/passage prefixes), and **Re-embed all recordings** after a model change. See [Semantic Search](semantic_search.md).

## 👤 Profiles

Named full-config snapshots — save the current configuration and switch between setups in one click (`g P` jumps here). See [Config Profiles](config_profiles.md).

## ⚙️ System

### Storage & retention

![Storage settings](../screenshots/settings-storage.png)

Audio directory, auto-delete by age or count, optional audio-only deletion (keep searchable metadata), an **Import audio** button (bring a `.wav`/`.mp3`/`.m4a`/`.flac` into the pipeline), and export. See [Storage, Paths & Retention](storage_paths_and_retention.md) and [Importing Audio](importing_audio.md).

### System tray

![Tray settings](../screenshots/settings-system-tray.png)

Show window on startup, minimize to tray on close, start at Windows login.

### Advanced

![Advanced settings](../screenshots/settings-advanced.png)

Daemon log level, pipe name, re-run the First Run Wizard, and other power-user options.

## Manual editing

Power users can edit `config.toml` directly. After editing, tell the daemon to reload:

```powershell
phoneme config reload
```

Full schema: [Configuration Reference](../developer-guide/config_reference.md).

## 🩺 Health indicators

Phoneme watches its own health (the same checks as the Doctor) and surfaces
problems three ways:

- a **health pill** in the header — green dot when everything passes, blinking
  red with an issue count when something fails; click it to open the Doctor;
- the **⚙ Settings button pulses red** while anything is unhealthy;
- a **banner** appears under the header naming the failing checks, with
  **🔧 Fix now** (restarts the whisper-server(s) / starts the daemon) and
  **🩺 Open Doctor**. Dismissing it re-arms automatically once health returns
  to ok.

The Doctor covers config presence, the audio folder, **free disk space** on
the volumes holding your recordings and the app data (a warning under ~2 GB,
critical under ~500 MB), the hook command, **model-file integrity** (a
missing, 0-byte or truncated model download is flagged before it bites), and
the Whisper / live-preview / Ollama servers. The checks **follow your
providers**: run transcription, the live preview, dictation, or an AI step on
a cloud provider and its local model/server checks make way for what can
still be verified — the API key is set and the endpoint answers — without
ever sending a billable request.

Each check shows a category badge when it fails — **Critical** (red: recording
or transcription is broken), **Warning** (amber: something is degraded but
capture still works) — plus a plain-English line on what the check verifies
and a `fix:` hint with the next step.

The Doctor triages instead of listing: a **health strip** stays pinned at the
top with the one-glance state — "All systems good ✓", or count chips per
category — alongside **Fix All** and **Re-run**. Failing checks come first as
full rows (badge, explanation, fix hint, per-check Fix), while everything
healthy folds into a single collapsed **"✓ N checks passing"** line — expand
it for a compact list grouped by subsystem (Servers / Models / Storage /
Configuration). Both the quick modal and the Settings Doctor tab use the same
layout.

The Doctor itself can **restart the bundled whisper-server(s)** with one click
when the Whisper or live-preview probe fails — it sweeps hung or orphaned
server processes and respawns them from your config (CLI:
`phoneme doctor --fix`). When several checks fail at once, **🔧 Fix All** runs
every available fix top-down in one go and re-checks when done.
