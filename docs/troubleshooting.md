# Troubleshooting

## "Daemon not reachable" from the CLI

```
$ phoneme list
error: daemon not reachable
```

The CLI auto-spawns the daemon if it's missing, but with an 8-second timeout.
On very slow machines the first cold start may exceed this.

**Fix:** Start the daemon explicitly: `phoneme daemon start`. Then try again.

## "Pipe in use" when starting the daemon

```
$ phoneme daemon start
error: another phoneme-daemon is running (pid 4521)
```

Another instance is already running.

**Fix:**
```powershell
phoneme daemon stop
phoneme daemon start
```

Or kill the process:
```powershell
Stop-Process -Name phoneme-daemon
```

## Tray icon doesn't appear

Windows sometimes hides tray icons by default. Right-click the taskbar →
Taskbar settings → "Select which icons appear on the taskbar" and enable
Phoneme.

## "Whisper unreachable" — recordings pile up

The configured `whisper.external_url` is not responding. Either the server is
down, the URL is wrong, or your `whisper.timeout_secs` is too low.

**Fix:**
```bash
phoneme doctor    # confirms the diagnosis
```

The recordings stay in `%LOCALAPPDATA%\phoneme\inbox\pending\`. Once
whisper-server is reachable, the daemon retries automatically (exponential
backoff, capped at 5 minutes between attempts).

## Hook fails or times out

Check the hook log:
```
%LOCALAPPDATA%\phoneme\logs\hook.log
```

Test the hook directly:
```bash
phoneme hook test
```

Common causes:
- Script not found (check `hook.command` in `%APPDATA%\phoneme\config.toml`)
- Script needs `-ExecutionPolicy Bypass` (we set this for `.ps1` automatically)
- Script does network I/O exceeding `hook.timeout_secs` — bump the timeout

## Model Download Wizard Fails Mid-Stream

If you were downloading the default model inside the First Run Wizard and the application crashed or the network dropped, you might be left with a corrupted, partially downloaded `.gguf` file.

**Fix:** Delete the corrupted file manually.
```powershell
Remove-Item -Path "$env:LOCALAPPDATA\phoneme\models\*.gguf"
```
Then restart Phoneme to try the download again.

## Catalog corruption

If the recordings list is empty or wrong but you have audio files on disk:

```bash
phoneme doctor --rebuild-catalog
```

This walks `audio_dir/` and `inbox/done/` and reconstructs the catalog
database from disk.

## Where is everything?

| What | Where |
|---|---|
| Config | `%APPDATA%\phoneme\config.toml` |
| Hooks (your edits) | `%APPDATA%\phoneme\hooks\` |
| Hooks (installer source) | `Program Files\Phoneme\hooks-templates\` |
| Catalog DB | `%LOCALAPPDATA%\phoneme\catalog.db` |
| Inbox queue | `%LOCALAPPDATA%\phoneme\inbox\` |
| Logs | `%LOCALAPPDATA%\phoneme\logs\` |
| Audio files | (configurable) — default `%USERPROFILE%\Documents\phoneme\audio\` |

## Doctor "Fix" works but the UI still shows errors after restart

If the tray app was launched while the daemon was already down (rare — usually happens if you quit the daemon manually), clicking **Fix** in the Doctor will successfully start the daemon. However, the GUI may still show stale error states until you close and reopen the main window.

**Why:** The tray process keeps a single IPC connection established at launch. When no daemon was available at launch, the connection was never established, and Tauri's state is immutable once the app starts. The daemon *is* running after Fix — other commands will reconnect automatically. Close the main window (tray stays alive) and click the tray icon to reopen it.

## Reset to factory defaults

```powershell
Stop-Process -Name phoneme-daemon -ErrorAction SilentlyContinue
Remove-Item -Recurse "$env:APPDATA\phoneme"
Remove-Item -Recurse "$env:LOCALAPPDATA\phoneme"
```

Then relaunch Phoneme — the wizard runs again from scratch. Audio files in
your `audio_dir` are preserved.
