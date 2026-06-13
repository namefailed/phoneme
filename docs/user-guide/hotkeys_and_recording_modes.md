# Hotkeys & Recording Modes

Phoneme supports several ways to start and stop capture. You can use the UI buttons, global hotkeys, the CLI, or IPC — they all talk to the same daemon.

## Global hotkeys (defaults)

Configure under **Settings → Capture → Hotkeys**. All hotkeys are **disabled by default** until you enable them in the wizard or settings.

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
phoneme record start
phoneme record stop
phoneme meeting start
phoneme meeting stop
```

See [CLI Reference](../developer-guide/cli_reference.md) for the full command set.

## UI recording modes

### Standard record

Click **Record** or use the record hotkey.

- **GUI Record button** — the **▾** next to Record opens a dropdown; under **"A voice note stops"** pick how the recording ends:
  - **When I click Stop** (default) — a manual start/stop toggle; it never cuts off on a quiet mic.
  - **When I go quiet** — stops automatically after the silence window (`silence_window_ms`, default ~3 s).
  - **After N seconds** — stops after exactly the number of seconds you type into the row.

  The choice is remembered on this device, applies to every later click of the Record button, and shows in the button's tooltip. Until you pick one, the old default applies: manual stop, or silence-stop if **Auto-stop on silence** (`recording.auto_stop_on_silence`) is enabled in **Settings → Capture**. Meetings are unaffected — they always run until you end them.
- **Built-in record hotkey** — Hold or Toggle, per **Settings → Capture → Hotkeys**. Hold is always hold-to-record regardless of the auto-stop setting (the dropdown has no hold option — a mouse click can't be held).
- **CLI** — the same three behaviors: stop signal, one-shot (`--oneshot`, stop on silence), and fixed-duration (`--duration N`) recording.

### Pause / resume

While recording, click **Pause** to suspend capture without finalizing the
recording; **Resume** continues the *same* catalog entry, so you never get
duplicate rows. This works on the CLI too — `phoneme record pause` and
`phoneme record resume` — and pausing a meeting pauses every track at once.

### Meeting Mode

Toggle **Meeting Mode** before or instead of a normal record. Captures:

1. **Mic track** — your voice (continuous buffer)
2. **System track** — WASAPI loopback (what you hear through speakers/headphones)

Both tracks share a **wall-clock timeline** so scrubbing to the same timestamp on either WAV hears the same moment. See [Meeting Mode](meeting_mode.md).

### Cancel

**Cancel** discards the in-progress capture without writing a catalog row. Useful when you started recording by mistake.

## Normalize audio level

A microphone left turned down captures the same words far quieter than the transcription model expects, and quiet recordings transcribe worse. Turn on **Normalize audio level** under **Settings → Capture → Recording** (`recording.normalize`) to fix this: when a recording finishes, Phoneme boosts its gain so the loudest moment sits just below clipping before the WAV is written.

- It is a single gain applied to the whole recording, so relative dynamics are preserved — loud parts stay louder than quiet parts.
- It only ever **boosts quiet audio**: an already-loud recording is left as captured, and a silent clip is never amplified into hiss.
- The ceiling is set by `recording.normalize_target_dbfs` (default **-1.0 dBFS** — a hair below full scale).
- It is **off by default**, and applies to **newly captured recordings only** — not the live streaming preview, and not [imported files](importing_audio.md) (those keep whatever level their author chose).

## CLI equivalents

| UI action | CLI |
|-----------|-----|
| Start / stop mic | `phoneme record start` / `stop` |
| Toggle (start if idle, else stop) | `phoneme record toggle` |
| In-place dictation | `phoneme record start --in-place` |
| Oneshot (silence-stop) | `phoneme record --oneshot` |
| Fixed duration | `phoneme record --duration 30` |
| Pause / resume | `phoneme record pause` / `resume` |
| Start / stop meeting | `phoneme meeting start` / `stop` |
| Toggle meeting | `phoneme meeting toggle` |
| Cancel | `phoneme record cancel` |

The `record toggle` / `meeting toggle` variants are atomic (start-or-stop in one
call), which makes them the cleanest thing to bind to a single external hotkey.
(The pre-1.8 flag spellings — `record --start`, `--stop`, … — still work as
deprecated aliases, so existing bindings don't break.)

## Tips

- **Pre-roll** (`recording.pre_roll_ms`, default 1500 ms) keeps a rolling mic buffer so the first syllable is not clipped when you react to a hotkey. See [Streaming Preview & Pre-Roll](streaming_preview_and_preroll.md).
- **Quiet mic?** Enable **Normalize audio level** (`recording.normalize`) to boost soft recordings to a consistent level before transcribing — see above.
- **Wear headphones in Meeting Mode** so speaker bleed does not duplicate remote audio on your mic track.
- If built-in hotkeys conflict with another app, change the combo in Settings or disable Phoneme's listener and use an external tool.
