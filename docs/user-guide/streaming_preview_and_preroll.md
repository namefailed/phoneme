# Streaming Preview & Pre-Roll

Two recording-quality features that address opposite ends of a capture: **pre-roll** saves the *start* of your speech; **streaming preview** shows progress *while* you are still talking.

> [!IMPORTANT]
> Streaming preview is a **beta** feature and ships **off by default**. The
> *wave 1* overhaul landed a big smoothness/stability pass — adaptive cadence
> so a heavy model can't wedge your recording, word-by-word reveal, a
> LIVE/LISTENING state, and the "it hears me" waveform (see **Feel &
> performance** below). The Beta label stays on until it's verified across a
> long dictation. Turn it on in **Settings → Transcription → Live Preview**.

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

Enable `recording.streaming_preview = true`. While recording, the daemon periodically re-transcribes only the **last ~15 seconds** of audio (a bounded rolling window, not the whole growing buffer) and stitches the genuinely-new tail onto a forward-growing **partial transcript** pushed to the UI. Keeping the window bounded makes each tick a constant cost instead of growing with the take.

### Important limitations

- This is a **preview**, not the final transcript. After stop, the normal pipeline runs again for the authoritative result.
- whisper.cpp's `/v1/audio/transcriptions` returns a **full** transcript per request — not token streaming. Phoneme simulates "live" feel via incremental re-transcription, roughly every **2 s** (a fast local/native provider tightens this to ~1 s).
- The preview yields to the final transcription (it only runs a tick when the single whisper permit is free), so it can never starve the authoritative pass.
- **Preview does not run during [in-place dictation](transcribe_in_place.md).** A quick dictation has no overlay to feed, and the per-second preview ticks would contend with the dictation's own latency-critical transcribe-and-paste on the single whisper permit — so Phoneme skips the preview loop entirely for in-place recordings.
- Preview costs extra CPU/GPU while recording. Disable on low-end hardware.
- Optionally use a separate, faster provider just for the preview — see the `[preview_whisper]` section in the [Configuration Reference](../developer-guide/config_reference.md).

### Configuration

```toml
[recording]
streaming_preview = true

[interface]
# Optional: float the preview caption over the whole desktop in an
# always-on-top window (requires streaming_preview).
preview_overlay = false
```

Or **Settings → Capture → Streaming preview** (and the **System-wide overlay** checkbox under **Settings → Transcription → Live Preview**).

### System-wide overlay

With `interface.preview_overlay = true`, the live caption also appears in a frameless, always-on-top window that floats over any app — useful during a meeting or screen share when the main window is hidden. It auto-shows when a recording or meeting starts, can be dragged anywhere (its position is remembered), and dims/hides shortly after capture stops. Off by default.

### Feel & performance

**Settings → Transcription → Live Preview → Feel & performance** tunes how the
preview reads. The defaults are designed to stay smooth on a modest machine.

| Setting | Config key | Default | What it does |
|---------|-----------|---------|--------------|
| Auto-throttle on slow machines | `recording.preview_adaptive` | `true` | When a preview update takes longer than its interval (a heavy model on a modest box), the daemon automatically slows the cadence toward the update's own cost (capped at 8 s) instead of thrashing the machine — the fix for "live preview makes recording lag/crash". Turn it off for a fixed update rate. |
| Reveal speed | `recording.preview_reveal_words_per_sec` | `12` | How fast live words stream into the overlay. Words flow in like speech instead of the caption jumping a whole chunk per update; a correction (when Whisper revises earlier words) still appears instantly. **`0`** = show each update the moment it arrives (no smoothing). |
| Overlay waveform | `recording.preview_waveform` | `true` | Shows the **"it hears me"** bars in the desktop overlay — live audio levels so you can see your voice is being captured, even between words. Cheap (an audio-level reading, no extra transcription), and it runs for single recordings, in-place dictation, and meetings. |
| "Listening" after | `recording.preview_idle_ms` | `2500` | When no new words arrive for this long, the overlay label calms from **LIVE** to **LISTENING** instead of leaving a frozen caption. |

> [!TIP]
> **Heavy final model?** If you enable preview while it's set to *Same as final
> model* and your final model is a heavy local one (medium / large), Phoneme
> shows a one-time nudge and a one-click **Use a dedicated Tiny model** button.
> A small dedicated preview model (Tiny / Base) on its own thread-limited server
> keeps the overlay snappy without changing the model that produces your
> authoritative transcript. See the **Preview source** options above.

> [!NOTE]
> The waveform bars run even during [in-place dictation](transcribe_in_place.md)
> (where the caption preview itself is skipped) — so you still get the "it hears
> me" feedback while dictating.

## Using both together

| Feature | When it helps |
|---------|----------------|
| Pre-roll | Hotkey dictation, quick reactions |
| Streaming preview | Long rants, meetings where you want to see text forming |
| Both | Long-form voice notes where you care about start *and* mid-flight feedback |

In **Meeting Mode**, the preview follows your **microphone** track (your dense local voice), so you still see live feedback during a call. Neither feature affects Meeting Mode timeline alignment — see [Meeting Mode](meeting_mode.md).

## 👥 Meetings: two tracks in the overlay

A meeting records your **microphone** and the **system audio** as two tracks.
**Settings → Transcription → Live Preview → Meetings** picks how the overlay
captions them:

- **One track at a time** (default) — one caption line plus a **🎤/🔊 button**
  on the overlay that switches which track the preview follows. Starts on your
  mic; same cost as a single-recording preview.
- **Both tracks at once** — two stacked caption lines, one per track, updating
  together. Roughly double the preview transcription work — pair it with a
  small dedicated preview model. The two loops take turns on the transcription
  server, so they never run two requests at once.

Config: `recording.meeting_preview = "toggle" | "both"`.
