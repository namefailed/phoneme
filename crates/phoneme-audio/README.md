# phoneme-audio

Audio capture and WAV encoding for the [Phoneme](../../README.md) voice notes app.

## What it does

| Module | Responsibility |
|---|---|
| `format` | `AudioConfig`, `SampleRate`, `Channels` types |
| `wav` | `write_wav` / `read_wav` / `duration_ms` via hound |
| `device` | CPAL input device enumeration + lookup |
| `convert` | `f32 ↔ i16`, downmix-to-mono, resample (rubato) |
| `silence` | `SilenceDetector` — RMS over rolling window |
| `source` | `Source` trait: production `CpalSource` + test `SyntheticSource` |
| `recorder` | `Recorder` public API — start/stop/cancel + auto-modes |

## Canonical format

Phoneme always saves recordings as **16-bit PCM, 16 kHz, mono**. The Recorder
converts whatever the CPAL device offers (commonly 44.1/48 kHz f32 stereo) to
this format before saving.

## Recording modes

```rust
use phoneme_audio::recorder::{Recorder, RecorderConfig, RecordingMode};

// Hold: stop when externally told.
let cfg = RecorderConfig { mode: RecordingMode::Hold, ..Default::default() };

// Oneshot: stop on silence.
let cfg = RecorderConfig {
    mode: RecordingMode::Oneshot,
    silence_threshold_dbfs: -45.0,
    silence_window_ms: 3000,
    ..Default::default()
};

// Fixed duration.
let cfg = RecorderConfig { mode: RecordingMode::Duration { secs: 10 }, ..Default::default() };
```

## Testing without a microphone

The `Source` trait abstracts the input. Tests use `SyntheticSource` to feed
hand-crafted PCM data:

```rust
let (source, sink) = SyntheticSource::new(AudioConfig::phoneme_default());
let recorder = Recorder::start(Box::new(source), RecorderConfig::default()).await?;
sink.push(loud_samples).await?;
sink.close();
let result = recorder.wait_for_finalize(&wav_path).await?;
```

## Running the tests

```bash
cargo test -p phoneme-audio
```

Device-dependent tests early-return if no input device is found on the
runner.
