# 🛠️ Troubleshooting

> See also the [FAQ](faq.md) for quick answers. Full path reference: [Storage, Paths & Retention](storage_paths_and_retention.md).

## 🔌 "Daemon not reachable" from the CLI

```
$ phoneme list
error: daemon not reachable
```

The CLI auto-spawns the daemon if it's missing, but with an 8-second timeout.
On very slow machines the first cold start may exceed this.

> [!TIP]
> **Fix:** Start the daemon explicitly via `phoneme daemon start`. Then try again.

## 🚫 "Pipe in use" when starting the daemon

```
$ phoneme daemon start
error: another phoneme-daemon is running (pid 4521)
```

Another instance is already running.

> [!TIP]
> **Fix:**
> ```powershell
> phoneme daemon stop
> phoneme daemon start
> ```

Or kill the process:
```powershell
Stop-Process -Name phoneme-daemon
```

## 👻 Tray icon doesn't appear

Windows sometimes hides tray icons by default. Right-click the taskbar →
Taskbar settings → "Select which icons appear on the taskbar" and enable
Phoneme.

## ⏳ "Whisper unreachable" — recordings pile up

The configured `whisper.external_url` is not responding. Either the server is
down, the URL is wrong, or your `whisper.timeout_secs` is too low.

> [!TIP]
> **Fix:**
> ```bash
> phoneme doctor    # confirms the diagnosis
> ```

The recordings stay in `%LOCALAPPDATA%\phoneme\inbox\pending\`. Once
whisper-server is reachable, the daemon retries automatically (exponential
backoff, capped at 5 minutes between attempts).

## ⚠️ A recording failed — seeing why, and retrying

Permanent failures (bad audio, a 4xx from a transcription provider, a hook
that exits non-zero) mark the recording **Failed** and light up a red
**⚠ N failed** badge on the queue panel at the bottom of the sidebar.

Click the badge to open the failure-details panel — one row per failed
recording:

- **What broke** — the step (Transcription or Hook) and the error message.
  The message is captured live as failures happen, so anything that failed
  while the app was open shows the real reason (the text is selectable —
  copy it straight into a search). For failures that predate the current
  session the message isn't available in the panel; the full story is in
  `%LOCALAPPDATA%\phoneme\logs\daemon.log`.
- **Retry** re-runs the whole pipeline for that recording (the same path as
  **Re-transcribe**); **Open** jumps to it in the library.
- **Retry all** walks the list top to bottom, one at a time, with a progress
  count.
- **Clear failed** resets the badge (the inbox `failed/` quarantine) only —
  the recordings keep their **Failed** status and stay in the library and in
  this panel. To find them later, use the list's status filter:
  **Transcription Failed** / **Hook Failed**.

`Esc` closes the panel. Transient problems (whisper-server down or
restarting) never land here — the queue retries those automatically, as
described above.

Cancelling is not failing: a recording you cancel yourself (removed from the
queue, or aborted mid-transcription) is marked **Cancelled** — a quiet, gray
status of its own. It never appears in this panel and never lights the failed
badge's red. Cancelled recordings stay in the library (find them with the
status filter's **Cancelled** entry) and can be re-run any time via
**Re-transcribe**.

## 🔌 Something else is using port 5809

You don't have to free the port. `whisper.bundled_server_port` (and the
preview's) is a **preference**, not a hard requirement: before each start the
daemon probes the port, and when another app already holds it, whisper-server
is started on a free port instead. Everything that talks to the server —
final transcription, the live preview, dictation, the Settings "Test"
button — follows the live port automatically, and the preview server never
picks the main server's port.

You can see the fallback happen in `%LOCALAPPDATA%\phoneme\logs\daemon.log`:

```
WARN preferred port 5809 in use by another app — whisper-server starting on 51234
```

and ask the daemon where its servers currently are:

```powershell
phoneme daemon status --json
# "whisper_preferred_port": 5809, "whisper_effective_port": 51234, ...
```

(`whisper_effective_port` is `null` while that server isn't running.)

Notes:

- The fallback lasts until the next server start (config change, Doctor →
  restart, daemon restart). Every start tries the preferred port first, so
  the server moves back to 5809 once the other app lets go of it.
- This only applies to the **bundled** server. An external endpoint
  (`whisper.mode = "external"`) is yours to manage — the daemon never moves
  or rewrites it.

## ⚠️ Hook fails or times out

Check the daemon log (hook activity is logged there; a failed hook also stores its last ~4 KB of stderr on the recording):
```
%LOCALAPPDATA%\phoneme\logs\daemon.log
```

Test the hook directly:
```bash
phoneme hook test
```

Common causes:
- Script not found (check `hook.command` in `%APPDATA%\phoneme\config.toml`)
- Script needs `-ExecutionPolicy Bypass` (we set this for `.ps1` automatically)
- Script does network I/O exceeding `hook.timeout_secs` — bump the timeout

## 🦙 Ollama didn't start automatically

When an AI step (cleanup, summary, tags, titles) points at a **local** Ollama
that isn't running, the daemon launches `ollama serve` for you (the
`[llm_post_process] autostart_ollama` knob, on by default). If the step still
fails with "couldn't reach":

- **Is `ollama` on PATH?** The daemon launches it from PATH. Run `ollama
  --version` in a fresh terminal; if that fails, reinstall Ollama or add it to
  PATH, then restart the daemon (PATH changes don't reach a running process).
- **Is the URL actually local?** Auto-launch only fires for
  `127.0.0.1`/`localhost`/`::1` endpoints (or an empty `api_url`, which means
  the local default). A remote Ollama is yours to run.
- **Check the launch log** — the launched server's output lands in
  `%LOCALAPPDATA%\phoneme\logs\ollama.log` (port already taken, missing
  models, etc.).
- **Was Ollama already running when the daemon started it up?** Then Phoneme
  treats it as *yours* for the daemon's whole lifetime: it is never restarted,
  never stopped, and if you later stop it yourself, Phoneme won't launch a
  replacement until the daemon restarts. That is deliberate — Phoneme never
  manages an Ollama it didn't start.
- A model can take a while to load on first use; the launcher waits ~15 s for
  the server itself, but the first generation may still need a model pull
  (`ollama pull <model>` once, manually).

## 🔑 Doctor says my API key is missing

With a cloud provider selected, the Doctor verifies a key is actually set for
each thing that will use one — **Transcription API key** for the main
provider, **Live-preview API key** / **Dictation STT key** for those
features, and **LLM API key (…)** for the AI steps. AI steps inherit the
cleanup connection's key whenever their own key field is blank, so the check
only fails when there is no key anywhere along that chain.

> [!TIP]
> **Fix:** Paste the key where the feature is configured — Settings →
> Transcription for the main provider, Settings → Transcription → Live
> Preview for the preview, Settings → Capture → Dictation for dictation, or
> Settings → Post-Processing → Connection for the AI steps. For AI steps you
> can also set the key once on the cleanup connection and leave the step's
> own field blank to inherit it.

The Doctor checks **presence, not validity** — it never sends a billable
request, so a typo'd key still shows as "configured" and only fails on the
first real run. A reachable endpoint (any HTTP answer counts, even 401) plus
a configured key is the most it can verify for free.

## 🛑 Model Download Wizard Fails Mid-Stream

If you were downloading the default model inside the First Run Wizard and the application crashed or the network dropped, you might be left with a corrupted, partially downloaded `.gguf` file.

> [!WARNING]
> **Fix:** Delete the corrupted file manually.
> ```powershell
> Remove-Item -Path "$env:LOCALAPPDATA\phoneme\models\*.gguf"
> ```
Then restart Phoneme to try the download again.

## 💥 Catalog corruption

If the recordings list is empty or wrong but you have audio files on disk:

```bash
phoneme doctor --rebuild-catalog
```

This walks `audio_dir/` and `inbox/done/` and reconstructs the catalog
database from disk.

## 🗺️ Where is everything?

| What | Where |
|---|---|
| Config | `%APPDATA%\phoneme\config.toml` |
| Hooks (your edits) | `%APPDATA%\phoneme\hooks\` |
| Hooks (installer source) | `Program Files\Phoneme\hooks-templates\` |
| Catalog DB | `%LOCALAPPDATA%\phoneme\catalog.db` |
| Inbox queue | `%LOCALAPPDATA%\phoneme\inbox\` |
| Logs | `%LOCALAPPDATA%\phoneme\logs\` |
| Audio files | (configurable) — default `%USERPROFILE%\Documents\phoneme\audio\` |

## 🩺 Doctor "Fix" works but the UI still shows errors after restart

If the tray app was launched while the daemon was already down (rare — usually happens if you quit the daemon manually), clicking **Fix** in the Doctor will successfully start the daemon. However, the GUI may still show stale error states until you close and reopen the main window.

**Why:** The tray process keeps a single IPC connection established at launch. When no daemon was available at launch, the connection was never established, and Tauri's state is immutable once the app starts. The daemon *is* running after Fix — other commands will reconnect automatically. Close the main window (tray stays alive) and click the tray icon to reopen it.

## Reset to factory defaults

> [!CAUTION]
> This wipes every setting and your whole transcript catalog. Audio files in
> your `audio_dir` are left untouched, but the recordings list is rebuilt from
> scratch.
>
> ```powershell
> Stop-Process -Name phoneme-daemon -ErrorAction SilentlyContinue
> Remove-Item -Recurse "$env:APPDATA\phoneme"
> Remove-Item -Recurse "$env:LOCALAPPDATA\phoneme"
> ```

Then relaunch Phoneme — the wizard runs again from scratch. Audio files in
your `audio_dir` are preserved.
