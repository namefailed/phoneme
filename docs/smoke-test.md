# Manual smoke test (10 minutes)

> [!IMPORTANT]
> Run this on a clean Windows VM (or fresh `%APPDATA%\phoneme\` and `%LOCALAPPDATA%\phoneme\`) before each release.

## Setup

- [ ] Uninstall any existing Phoneme.
- [ ] Delete `%APPDATA%\phoneme\` and `%LOCALAPPDATA%\phoneme\`.
- [ ] Have a whisper-server reachable at `http://127.0.0.1:5809` (or use
      bundled-server mode with a known-good GGML model, e.g. `ggml-base.en.bin`).

## Install + wizard

- [ ] Install the new MSI.
- [ ] First launch: the first-run wizard appears. The progress stepper shows
      **5 phases** — Welcome, Transcription & AI, Capture, Output, Done.
- [ ] **Express setup** (the default path) — the recommended local setup runs:
      whisper-server + a RAM-appropriate model download, then mic and hotkey.
  - [ ] Welcome — "Customize setup" is offered alongside the express path.
  - [ ] Transcription & AI — the model download reports progress and finishes
        without error (or, with a working endpoint, the connection test passes).
  - [ ] Capture — the microphone device list populates.
  - [ ] Capture — the hotkey toggle is off by default.
  - [ ] Done — big Record button visible.
- [ ] Re-run the wizard and pick **Customize setup**: the per-feature pages show
      under their phases — Features (three cards, mode 3 selectable) and Connect
      AI under Transcription & AI; Microphone and Live Preview under Capture;
      Auto Summary and Destination under Output. The default destination points
      at `to-stdout.ps1`, with `to-clipboard.ps1` listed as an alternative.
- [ ] Finish wizard. Recordings view appears empty.

## Core flow

- [ ] CLI: `phoneme record --oneshot` records 3 seconds, transcribes, prints
      transcript. Exit code 0.
- [ ] Window: the recording appears in the list within ~10 seconds. Status
      is `done` with a green dot.
- [ ] Click the row: detail pane shows waveform + transcript + action buttons.
- [ ] Click Play: audio plays back through the system's default output.
- [ ] Edit the transcript, Ctrl+S: dirty indicator clears.
- [ ] Reopen the recording: edit persists.
- [ ] Click Delete: confirmation dialog appears with "Don't ask again" checkbox.
      Confirm — row disappears.
- [ ] Click Delete again on another row: if checkbox was checked, deletes
      immediately without the dialog.

## External hotkey

- [ ] Kanata (or AHK) is configured to send `phoneme record start` /
      `record stop` on a key combo.
- [ ] Press the combo, speak, release. Recording appears in list.

## Tray

- [ ] Tray icon is gray (idle) at rest.
- [ ] Recording: icon turns red, tooltip says "Recording…".
- [ ] Transcribing: icon turns amber.
- [ ] After hook completes: icon returns to gray.
- [ ] Right-click tray → menu items all clickable.
- [ ] Left-click tray: toggles window visibility.

## Failure modes

- [ ] Kill whisper-server. Trigger a recording.
  - [ ] Tray turns amber with "N pending" tooltip.
  - [ ] Recording lands in inbox/pending and stays there.
  - [ ] Restart whisper-server. Within ~30 seconds the queue drains.
- [ ] Disconnect microphone mid-recording. The daemon surfaces an error;
      tray shows red icon; Doctor view explains.
- [ ] Edit `config.toml` with a bogus value (e.g., `recording.sample_rate = 7`) and restart daemon. It refuses to start with a clear error pointing at the offending key.

## Settings

- [ ] Open Settings via tray or ⚙ button.
- [ ] All 14 tabs render in order: Appearance, Transcription, Capture, Dictation,
      Live Preview, Post-Processing, Playbook, Integrations, Hotkeys, Diarization,
      Tags, Search, Profiles, System.
- [ ] Appearance: switch theme — UI repaints immediately.
- [ ] Post-Processing: enable Smart Cleanup with the Ollama provider, enter a
      model name, Save. The next recording should run the LLM cleanup step.
- [ ] Capture: change the silence threshold and Save. Verify
      `%APPDATA%\phoneme\config.toml` is updated atomically.

## Doctor

- [ ] Open Doctor. All checks green (Ollama shows green even if not running,
      since it is optional when Smart Cleanup is disabled).
- [ ] Move the model file. Re-run checks. The "Whisper model file" row goes red.
- [ ] Kill the daemon (`phoneme daemon stop`). Reopen Doctor.
      "Daemon" row shows ✗. Click Fix — daemon restarts.
      Close and reopen the window; all checks green again.

## Uninstall

- [ ] Uninstall via Add/Remove Programs.
- [ ] `Program Files\Phoneme\` is gone.
- [ ] `%APPDATA%\phoneme\config.toml` is preserved.
- [ ] `%LOCALAPPDATA%\phoneme\catalog.db` is preserved.

---

If all the above pass, ship.
