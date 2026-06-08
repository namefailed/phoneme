# Hotkeys & Recording Modes

Phoneme supports several ways to start and stop capture. You can use the UI buttons, global hotkeys, the CLI, or IPC — they all talk to the same daemon.

## Global hotkeys (defaults)

Configure under **Settings → Hotkeys**. All hotkeys are **disabled by default** until you enable them in the wizard or settings.

| Hotkey | Default combo | Default mode | Purpose |
|--------|---------------|--------------|---------|
| **Record** | `Ctrl+Alt+Space` | Hold | Standard microphone recording |
| **Transcribe-in-Place** | `Ctrl+Alt+I` | Hold | Dictate into the focused application |
| **Meeting Mode** | `Ctrl+Alt+M` | Toggle | Dual-track mic + system audio |

### Hold vs toggle

- **Hold** — Recording runs only while the key combo is physically held. Release to stop. Best for short dictation.
- **Toggle** — First press starts; second press stops. Best for meetings and long notes.

Meeting Mode uses **toggle only** — a meeting can run for many minutes.

### External hotkey tools

You do not have to use Phoneme's built-in listener. Many users bind **AutoHotkey**, **Kanata**, or **PowerToys** to shell commands:

```powershell
phoneme record --start
phoneme record --stop
phoneme meeting --start
phoneme meeting --stop
```

See [CLI Reference](../developer-guide/cli_reference.md) for the full command set.

## UI recording modes

### Standard record

Click **Record** or use the record hotkey. Modes in **Settings → Recording**:

| Mode | Behavior |
|------|----------|
| **Hold** | Stop when you release the hotkey (if using built-in hotkey) |
| **Toggle** | Stop on second press |
| **Oneshot** | Auto-stops after ~3 s of silence (configurable via `silence_window_ms`) |
| **Duration** | Stops after a fixed number of seconds |

### Pause / resume

While recording, click **Pause** (or send `record_pause` over IPC). Capture suspends without finalizing. **Resume** continues the same catalog entry — no duplicate rows.

### Meeting Mode

Toggle **Meeting Mode** before or instead of a normal record. Captures:

1. **Mic track** — your voice (continuous buffer)
2. **System track** — WASAPI loopback (what you hear through speakers/headphones)

Both tracks share a **wall-clock timeline** so scrubbing to the same timestamp on either WAV hears the same moment. See [Meeting Mode](meeting_mode.md).

### Cancel

**Cancel** discards the in-progress capture without writing a catalog row. Useful when you started recording by mistake.

## CLI equivalents

| UI action | CLI |
|-----------|-----|
| Start / stop mic | `phoneme record --start` / `--stop` |
| Toggle mic | `phoneme record --toggle` |
| Oneshot (silence-stop) | `phoneme record --oneshot` |
| Fixed duration | `phoneme record --duration 30` |
| Start / stop meeting | `phoneme meeting --start` / `--stop` |
| Cancel | `phoneme record --cancel` |

## Tips

- **Pre-roll** (`recording.pre_roll_ms`, default 1500 ms) keeps a rolling mic buffer so the first syllable is not clipped when you react to a hotkey. See [Streaming Preview & Pre-Roll](streaming_preview_and_preroll.md).
- **Wear headphones in Meeting Mode** so speaker bleed does not duplicate remote audio on your mic track.
- If built-in hotkeys conflict with another app, change the combo in Settings or disable Phoneme's listener and use an external tool.
