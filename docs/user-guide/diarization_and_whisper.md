# Native Whisper & Offline Diarization

Phoneme v1.8 completely overhauls the internal transcription pipeline, migrating from a Python-based sub-process architecture to a pure-Rust, lightning-fast native integration.

## Native Whisper Engine

Instead of relying on external binaries or HTTP servers, Phoneme now links directly against `whisper.cpp` via the `whisper-rs` crate. This fundamentally changes how recordings are transcribed.

### Word-by-Word Streaming

With the native engine, audio is fed into the Whisper model as it is recorded. You will see words appear in the UI in real-time, often with less than 500ms of latency.

- **Offline by Default:** Everything runs locally on your machine.
- **Hardware Agnostic:** Phoneme uses the optimized `whisper.cpp` engine which accelerates using your CPU, or offloads to your GPU if supported.
- **Model Sizes:** Through the First Run Wizard, Phoneme detects your system RAM/VRAM and automatically recommends the best Whisper model. You can always change this later in **Settings -> Whisper**.

## Offline Speaker Diarization (Pyannote)

When you capture audio using Meeting Mode (recording both your mic and the system audio), Phoneme has enough data to accurately reconstruct the conversation. But what if multiple people are speaking on the system audio track?

Enter **Pyannote**.

Phoneme integrates the powerful Pyannote ONNX model for offline speaker diarization. This means it can listen to a track and separate out different speakers entirely locally.

### How it Works
1. **Recording:** You start a Meeting Mode session. Phoneme captures `Mic` and `System` as two separate files.
2. **Diarization Pipeline:** Before transcription, Phoneme pipes the audio through the Pyannote ONNX model.
3. **Speaker Tagging:** The model emits timestamps of who spoke when.
4. **Transcription:** Phoneme uses Whisper to transcribe those specific time-slices.
5. **Merging:** The final transcript neatly identifies `[Speaker 1]`, `[Speaker 2]`, and your own `[Mic]` track.

> [!NOTE]
> Diarization requires a dedicated ONNX model which you can download via the First Run Wizard or Settings menu. Because it runs locally, processing a 30-minute meeting might take a few minutes on older hardware, but it is 100% private.
