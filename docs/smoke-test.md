# Manual smoke test (10 minutes)

Run this on a clean Windows VM (or fresh `%APPDATA%\phoneme\` and
`%LOCALAPPDATA%\phoneme\`) before each release.

## Setup

- [ ] Uninstall any existing Phoneme.
- [ ] Delete `%APPDATA%\phoneme\` and `%LOCALAPPDATA%\phoneme\`.
- [ ] Have a whisper-server reachable at `http://127.0.0.1:5809` with a Gemma
      model loaded (or use bundled-server mode 2 with a known-good GGUF).

## Install + wizard

- [ ] Install the new MSI.
- [ ] First launch: wizard appears.
  - [ ] Step 2 (mode) — three cards visible, mode 3 selectable.
  - [ ] Step 3 (configure) — Test button reports success for working endpoint.
  - [ ] Step 4 (microphone) — device list populates.
  - [ ] Step 5 (hook) — default points at `to-stdout.ps1`.
  - [ ] Step 6 (hotkey) — toggle off by default.
  - [ ] Step 7 (done) — big Record button visible.
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
- [ ] Click Delete: row disappears.

## External hotkey

- [ ] Kanata (or AHK) is configured to send `phoneme record --start` /
      `--stop` on a key combo.
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
- [ ] Edit `config.toml` with a bogus value (e.g., `recording.sample_rate = 7`)
      and restart daemon. It refuses to start with a clear error pointing at
      the offending key.

## Settings

- [ ] Open Settings via tray or ⚙ button.
- [ ] All eight sections render: Whisper, Recording, Hotkey, Hook, Storage, Tray,
      Advanced.
- [ ] Change the silence threshold and Save. Verify `%APPDATA%\phoneme\config.toml`
      is updated atomically.

## Doctor

- [ ] Open Doctor. All checks green.
- [ ] Move the model file. Re-run checks. The "Model file" row goes red
      with a Fix button.

## Uninstall

- [ ] Uninstall via Add/Remove Programs.
- [ ] `Program Files\Phoneme\` is gone.
- [ ] `%APPDATA%\phoneme\config.toml` is preserved.
- [ ] `%LOCALAPPDATA%\phoneme\catalog.db` is preserved.

---

If all the above pass, ship.
