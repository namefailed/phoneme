# 🔊 phoneme-audio

This crate is the heart of Phoneme's audio pipeline. It handles everything from the raw microphone input to the final generated `.wav` files.

## 🗂️ Responsibilities

- **Capture**: Interfacing with the operating system's audio stack via `cpal`. Handles both microphone input and Windows WASAPI loopback (for system audio).
- **Processing**: Real-time resampling (using `rubato`), stereo-to-mono downmixing, and `f32` to `i16` conversions to produce the canonical 16 kHz format.
- **Pre-Roll & Silence**: Managing the idle ring buffer to catch the first syllable, and detecting silence to auto-stop recordings.
- **Decoding**: Importing and decoding `.mp3`, `.m4a`, and `.wav` files via `symphonia`.
