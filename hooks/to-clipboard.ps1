# to-clipboard.ps1 — copies the transcript to the Windows clipboard, so you can
# paste a voice note straight into any app.
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
# Point a hook at it from config.toml:
#   commands = ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-clipboard.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) {
    Write-Output 'Empty transcript; nothing copied.'
    exit 0
}

Set-Clipboard -Value $text
$preview = $text.Substring(0, [Math]::Min(60, $text.Length))
Write-Output "Copied to clipboard: $preview"
