# Phoneme Hooks

Hooks are the seam that lets Phoneme stay agnostic to where transcripts go.
Phoneme commits to one delivery mechanism: **a user-owned subprocess receives
JSON on stdin**.

## The contract

| Channel | Direction | Content |
|---|---|---|
| `stdin` | daemon → hook | One JSON object terminated by EOF |
| `stdout` | hook → daemon | Ignored by the daemon (captured to `hook.log`) |
| `stderr` | hook → daemon | Captured to `hook.log`; last 4 KB stored in catalog on non-zero exit |
| exit code | hook → daemon | `0` = success; non-zero = failure |
| timeout | daemon enforces | `hook.timeout_secs` (default 30) |
| env vars | daemon sets | `PHONEME_ID`, `PHONEME_AUDIO_PATH`, `PHONEME_TRANSCRIPT` |

> [!WARNING]
> Security Risk: The `PHONEME_TRANSCRIPT` environment variable contains raw, unsanitized user voice-to-text. While environment variables are generally safe, using `$env:PHONEME_TRANSCRIPT` inside `Invoke-Expression` or similar shell eval wrappers exposes your hook to command injection. **Always prefer parsing the JSON payload via stdin (`$payload.transcript`) instead of relying on the environment variable.**

## The JSON payload

```json
{
  "id": "20260519T143500823",
  "timestamp": "2026-05-19T14:35:00.823-05:00",
  "transcript": "The cleaned transcription text",
  "audio_path": "C:\\Users\\matt\\Documents\\phoneme\\audio\\2026-05-19\\143500823.wav",
  "duration_ms": 8470,
  "model": "ggml-base.en",
  "metadata": {
    "phoneme_version": "1.5.0",
    "hook_version": 1
  }
}
```

**Stability commitment:** while `metadata.hook_version` is `1`, surrounding
fields will not be renamed or removed. v1.x may add fields. v2.x may bump and
break.

## Configuration

Set the hook in `%APPDATA%\phoneme\config.toml`:

```toml
[hook]
# Run one or more commands, in order, after every transcription.
commands = ["powershell -File %APPDATA%/phoneme/hooks/to-file.ps1"]
timeout_secs = 30
webhook_url = "https://your-webhook.app/api/ingest"
# When false, transcription just saves the text and does NOT fire hooks/webhook;
# run them on demand with the "⚡ Re-fire hook" button. Default: true.
run_on_transcribe = true

# Conditional hooks: run an extra command only when the transcript matches.
[[hook.keyword_rules]]
pattern = "action item:"        # case-insensitive by default
command = "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-todoist.ps1"

[[hook.keyword_rules]]
pattern = "summarize:"
command = "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/summarize-with-ollama.ps1"
case_sensitive = false

[llm_post_process]
enabled = true
provider = "openai" # none | ollama | openai | groq | anthropic
api_url = ""        # Leave empty to use the provider default (OpenAI:
                    # https://api.openai.com/v1/chat/completions,
                    # Ollama: http://127.0.0.1:11434/api/generate;
                    # Groq and Anthropic use their standard endpoints)
model = "gpt-4o"    # e.g. gpt-4o (OpenAI), llama-3.1-8b-instant (Groq),
                    # claude-3-5-haiku-latest (Anthropic), llama3.2:3b (Ollama)
api_key = "sk-..."  # Required for cloud providers; ignored by Ollama
prompt = "Clean up this voice transcript, removing stutters and filler words. Reply ONLY with the cleaned text."

```

Path expansion (`%VAR%`, `~`) is performed at config load.

## Discovery and invocation

Hooks are not on PATH. The full command string is invoked via the system shell:
- `.ps1` → `powershell -ExecutionPolicy Bypass -File <path>`
- `.exe` / `.bat` / `.cmd` → invoked directly
- Anything else → invoked directly (you're responsible for making it executable)

## Reference hooks

Phoneme ships nine reference hooks. On first run they're copied to
`%APPDATA%\phoneme\hooks\`. **The installer never overwrites them**, so feel free
to edit. Every shipped hook uses `Set-StrictMode` + `$ErrorActionPreference =
'Stop'`, so a real failure reports as a failed hook instead of a silent success.

### General-purpose

| Hook | What it does |
|---|---|
| `to-stdout.ps1` | The default. Echoes the transcript to stdout — use it to verify the pipeline works. |
| `to-clipboard.ps1` | Copies the transcript to the Windows clipboard, ready to paste anywhere. |
| `to-file.ps1` | Appends every transcript (timestamped) to one running Markdown file. Destination defaults to `~/Documents/VoiceNotes.md`; override with the `PHONEME_NOTES_FILE` env var. |
| `to-markdown-daily.ps1` | Obsidian-style daily note at `~/Documents/notes/YYYY-MM-DD.md`: `- **14:35** — … ^20260519T143500823` |

### Showcase / integrations

| Hook | What it does |
|---|---|
| `to-webhook.ps1` | POSTs the transcript as JSON to a webhook (Discord/Slack/n8n/your own server). Set `PHONEME_WEBHOOK_URL`. A spoken note can hit a team channel or automation the instant you stop talking. |
| `summarize-with-ollama.ps1` | Sends the transcript to a **local** Ollama model and saves a summary + action items to `~/Documents/notes/YYYY-MM-DD-summaries.md` — fully offline, no API keys. Set `PHONEME_OLLAMA_MODEL` (default `llama3.2:3b`). |
| `to-todoist.ps1` | Creates a Todoist task from the note. Designed to be **keyword-triggered** on `"action item:"` so only your action items become tasks. Set `PHONEME_TODOIST_TOKEN`. |

### Advanced (Emacs / Org)

| Hook | What it does |
|---|---|
| `to-org-journal.ps1` | Appends to `~/Documents/org/journal.org` under today's "Log" section — a worked example of a richer Org integration; adapt to your own journal layout. |
| `to-denote.ps1` | Creates a Denote-flavoured note (`20260519T143500--slug__voice.org`) under `~/Documents/org/notes/`. |

> [!TIP]
> Pair a showcase hook with a **keyword-triggered rule** (Settings → Action Hook):
> e.g. only run `summarize-with-ollama.ps1` when the transcript contains
> `"summarize:"`, or fire a Todoist webhook only on `"action item:"`.

## Keyword-triggered hooks

Run an extra command **only when the transcript matches a phrase** — on top of
the always-on `commands`. Configure them in **Settings → Action Hook**, or in
`config.toml`:

```toml
[[hook.keyword_rules]]
pattern = "action item:"   # matched case-insensitively unless case_sensitive = true
command = "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-todoist.ps1"

[[hook.keyword_rules]]
pattern = "TODO"
command = "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-file.ps1"
case_sensitive = true
```

Now saying *"…action item: send Sarah the contract"* runs `to-todoist.ps1`
(which strips the `action item:` prefix and files the task), while ordinary notes
are left alone. Each matching rule receives the **same JSON payload on stdin** as
a normal hook, so any of the reference hooks works as a rule target. An empty
`pattern` never matches.

## Run hooks only on demand

By default hooks fire after **every** transcription, including re-transcriptions.
If you'd rather a re-transcribe just fix the text *without* re-running side
effects (re-appending to a note, re-posting a webhook), turn hooks off:

```toml
[hook]
run_on_transcribe = false
```

Transcription still saves the transcript; you then fire hooks deliberately with
the **⚡ Re-fire hook** button on a recording.

## Putting it all together

A config that uses every hook feature at once — chain two always-on hooks, POST
to a webhook, and route action items to Todoist via a keyword rule:

```toml
[hook]
commands = [
  "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-clipboard.ps1",
  "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-file.ps1",
]
webhook_url = "https://hooks.slack.com/services/XXX/YYY/ZZZ"
run_on_transcribe = true
timeout_secs = 30

[[hook.keyword_rules]]
pattern = "action item:"
command = "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-todoist.ps1"
```

Every note is copied to the clipboard, appended to your notes file, and POSTed to
Slack; notes containing *"action item:"* additionally become Todoist tasks.
Subprocess hooks run serially in order; the webhook fires concurrently.

## Writing your own

A minimal PowerShell hook:

```powershell
$payload = $input | Out-String | ConvertFrom-Json
Write-Output $payload.transcript
```

A minimal bash hook (Git Bash / WSL):

```bash
#!/usr/bin/env bash
read -r -d '' payload
echo "$payload" | jq -r '.transcript' >> ~/Documents/notes.txt
```

A minimal Python hook:

```python
#!/usr/bin/env python3
import json, sys
payload = json.load(sys.stdin)
with open("notes.txt", "a") as f:
    f.write(payload["transcript"] + "\n")
```

## Testing your hook

```bash
phoneme hook test
```

This runs your configured hook with a sample payload. Prints exit code,
duration, stdout, stderr.

## Common gotchas

- **PowerShell execution policy**: signed hooks aren't required; the
  installer launches PowerShell with `-ExecutionPolicy Bypass`.
- **Timeouts**: If your hook does network I/O, bump `hook.timeout_secs`.
- **Working directory**: hooks run with cwd set to `%USERPROFILE%`. Use
  absolute paths or `~` if you depend on a specific location.
- **Multi-destination delivery**: Add multiple hook commands to the `commands = [...]` array. They will be executed serially.
- **Webhooks**: Provide a `webhook_url` in your `config.toml` to instantly POST the JSON payload to a web service. Phoneme executes both subprocess hooks and webhooks concurrently.
- **Encoding**: PowerShell defaults to UTF-16 for `Out-File`. Use
  `Set-Content -Encoding UTF8` (or `[System.IO.File]::WriteAllText`) when
  writing files that other tools will read.

