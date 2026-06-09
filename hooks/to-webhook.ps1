# to-webhook.ps1 — POST the transcript to a webhook (Discord, Slack, n8n, Zapier,
# Make.com, your own server — anything that accepts a JSON body).
#
# A spoken note can land in a team channel, a task queue, or an automation
# pipeline the instant you stop talking.
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_WEBHOOK_URL     (required) the target URL.
#   PHONEME_WEBHOOK_FORMAT  body shape to send. One of:
#                             "discord" (default) -> { "content": <transcript> }
#                             "slack"             -> { "text":    <transcript> }
#                             "full"              -> the entire Phoneme payload,
#                                                    forwarded verbatim (best for
#                                                    n8n / your own endpoint)
#
# Point a hook at it from config.toml:
#   commands = ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-webhook.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$url = $env:PHONEME_WEBHOOK_URL
if ([string]::IsNullOrWhiteSpace($url)) {
    Write-Error 'PHONEME_WEBHOOK_URL is not set. Set it to your webhook URL.'
}

$format = $env:PHONEME_WEBHOOK_FORMAT
if ([string]::IsNullOrWhiteSpace($format)) { $format = 'discord' }

switch ($format.ToLower()) {
    'slack'   { $body = @{ text = $text } | ConvertTo-Json -Depth 4 }
    'full'    { $body = $raw }  # forward the original payload byte-for-byte
    default   { $body = @{ content = $text } | ConvertTo-Json -Depth 4 }  # discord
}

# Send as UTF-8 so non-ASCII transcripts arrive intact.
$bytes = [System.Text.Encoding]::UTF8.GetBytes($body)
Invoke-RestMethod -Uri $url -Method Post -Body $bytes -ContentType 'application/json; charset=utf-8' | Out-Null
Write-Output "Posted transcript to webhook ($format)."
