# Streaming Preview & Pre-Roll

Two recording-quality features that address opposite ends of a capture: **pre-roll** saves the *start* of your speech; **streaming preview** shows progress *while* you are still talking.

## Pre-roll buffer

### Problem

When you hit a hotkey, there is always a small delay before capture starts. Without pre-roll, the first syllable of "Okay so…" can be clipped.

### Solution

When `recording.pre_roll_ms > 0` (default **1500 ms**), the daemon keeps the microphone open in the background between recordings. Audio rolls through an in-memory ring buffer; only the last N milliseconds are retained.

On **Record Start**, those buffered samples are **prepended** to the new WAV before live capture continues.

### Configuration

```toml
[recording]
pre_roll_ms = 1500   # 0 = disabled (mic only open while recording)
```

Or **Settings → Capture → Pre-roll**.

### Notes

- Pre-roll applies to **microphone** capture only, not system loopback.
- Idle buffer is **never written to disk** unless you start a recording.
- Slightly higher idle CPU — the mic stream stays open.

## Streaming transcription preview

### Problem

After you stop a long recording, Whisper can take seconds to minutes. The UI shows "Transcribing…" with no feedback.

### Solution

Enable `recording.streaming_preview = true`. While recording, the daemon periodically re-transcribes **new** audio (not the entire buffer every tick) and pushes a **partial transcript** to the UI.

### Important limitations

- This is a **preview**, not the final transcript. After stop, the normal pipeline runs again for the authoritative result.
- whisper.cpp's `/v1/audio/transcriptions` returns a **full** transcript per request — not token streaming. Phoneme simulates "live" feel via incremental re-transcription every ~2 seconds.
- Preview costs extra CPU/GPU while recording. Disable on low-end hardware.

### Configuration

```toml
[recording]
streaming_preview = true
```

Or **Settings → Capture → Streaming preview**.

## Using both together

| Feature | When it helps |
|---------|----------------|
| Pre-roll | Hotkey dictation, quick reactions |
| Streaming preview | Long rants, meetings where you want to see text forming |
| Both | Long-form voice notes where you care about start *and* mid-flight feedback |

Neither affects **Meeting Mode** timeline alignment — see [Meeting Mode](meeting_mode.md).
