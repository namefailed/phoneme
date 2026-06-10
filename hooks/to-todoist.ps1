# to-todoist.ps1 — turn a spoken note into a Todoist task.
#
# This hook shines when paired with a KEYWORD-TRIGGERED RULE so only your
# action items become tasks. In Settings -> Action Hook, add a rule:
#
#     pattern  = "action item:"
#     command  = powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-todoist.ps1
#
# Now say "...action item: email Sarah the contract" and it lands in Todoist,
# while your other notes are untouched. (Equivalent config.toml form:)
#
#     [[hook.keyword_rules]]
#     pattern = "action item:"
#     command = "powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-todoist.ps1"
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_TODOIST_TOKEN   (required) Todoist API token, from
#                           Todoist -> Settings -> Integrations -> Developer.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$token = $env:PHONEME_TODOIST_TOKEN
if ([string]::IsNullOrWhiteSpace($token)) {
    Write-Error 'PHONEME_TODOIST_TOKEN is not set. Set it to your Todoist API token.'
}

# Strip a leading "action item:" prefix (if present) so the task text is clean.
$task = ($text -replace '(?i)^\s*action item:\s*', '').Trim()
if ([string]::IsNullOrWhiteSpace($task)) { $task = $text }

$body = @{ content = $task } | ConvertTo-Json -Depth 4
$headers = @{ Authorization = "Bearer $token" }
Invoke-RestMethod -Uri 'https://api.todoist.com/rest/v2/tasks' -Method Post -Headers $headers -Body $body -ContentType 'application/json' | Out-Null
Write-Output "Added Todoist task: $task"
