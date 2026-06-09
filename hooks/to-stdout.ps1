# to-stdout.ps1 — the default hook. Echoes the transcript (and a short summary
# of the recording) to stdout, so you can confirm the pipeline works end to end.
#
# ── How hooks work ──────────────────────────────────────────────────────────
# After every transcription Phoneme runs your hook command and pipes the
# recording to it as a single JSON object on STDIN, terminated by EOF. The shape
# is (see docs/developer-guide/plugins_and_hooks.md for the authoritative list):
#
#   {
#     "id":          "20260519T143500823",          # recording id (sortable)
#     "timestamp":   "2026-05-19T14:35:00.823-05:00", # ISO-8601 local time
#     "transcript":  "the final transcript text",
#     "audio_path":  "C:\\...\\143500823.wav",
#     "duration_ms": 8470,
#     "model":       "ggml-base.en",
#     "metadata":    { "phoneme_version": "1.8.0", "hook_version": 1 }
#   }
#
# Phoneme treats a NON-ZERO exit code as a FAILED hook, so every bundled script
# enables strict error handling: any error stops the script with a non-zero exit
# instead of silently "succeeding". stdout/stderr are captured to hook.log.
#
# ── Configure ───────────────────────────────────────────────────────────────
# This is the out-of-the-box default. Point a hook at it from config.toml:
#   commands = ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-stdout.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Read the whole payload from stdin. [Console]::In.ReadToEnd() is more robust
# than `$input | Out-String` — it captures the raw stream even when the hook is
# invoked in contexts where the automatic `$input` enumerator is empty.
$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) {
    Write-Error 'No payload received on stdin.'
}

$payload = $raw | ConvertFrom-Json
$secs = [math]::Round($payload.duration_ms / 1000.0, 1)

Write-Output "[$($payload.timestamp)] $($payload.model), ${secs}s"
Write-Output $payload.transcript
