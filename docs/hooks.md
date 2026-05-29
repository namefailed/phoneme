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
    "phoneme_version": "1.4.1",
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
commands = ["powershell -File %APPDATA%/phoneme/hooks/to-org-journal.ps1"]
timeout_secs = 30
webhook_url = "https://your-webhook.app/api/ingest"

[llm_post_process]
enabled = true
provider = "openai" # "openai", "ollama", or "none"
api_url = ""        # Leave empty to use the provider default (OpenAI:
                    # https://api.openai.com/v1/chat/completions,
                    # Ollama: http://127.0.0.1:11434/api/generate)
model = "gpt-4o"    # gpt-4o-mini / gpt-4o for OpenAI; llama3.2:3b for Ollama
api_key = "sk-..."  # Required for OpenAI, ignored by Ollama
prompt = "Clean up this voice transcript, removing stutters and filler words. Reply ONLY with the cleaned text."

```

Path expansion (`%VAR%`, `~`) is performed at config load.

## Discovery and invocation

Hooks are not on PATH. The full command string is invoked via the system shell:
- `.ps1` → `powershell -ExecutionPolicy Bypass -File <path>`
- `.exe` / `.bat` / `.cmd` → invoked directly
- Anything else → invoked directly (you're responsible for making it executable)

## Reference hooks

Phoneme ships four reference hooks. On first run they're copied to
`%APPDATA%\phoneme\hooks\`. **The installer never overwrites them**, so feel
free to edit.

### to-stdout.ps1

The default. Echoes the transcript to stdout. Use this to verify the pipeline
works.

### to-org-journal.ps1

Appends each transcript to `~/Documents/org/journal.org` under today's date
heading. Matches Doom Emacs / Denote workflows.

```org
* 2026-05-19 Tue
** 14:35 The cleaned transcription text
   :PROPERTIES:
   :PHONEME_ID: 20260519T143500823
   :AUDIO: C:/.../143500823.wav
   :END:
```

### to-markdown-daily.ps1

Obsidian-style daily note at `~/Documents/notes/YYYY-MM-DD.md`:

```markdown
# 2026-05-19

- **14:35** — The cleaned transcription text ^20260519T143500823
```

### to-denote.ps1

Creates a Denote-flavored note file under `~/Documents/org/notes/`:

```
20260519T143500--the-cleaned-transcription-text__voice.org
```

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

