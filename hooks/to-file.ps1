# to-file.ps1 — the simplest "just save my voice notes somewhere" hook.
# Appends every transcript, with a timestamp heading, to one running Markdown
# file.
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_NOTES_FILE   destination file. Default: ~/Documents/VoiceNotes.md
#                        Point it inside an Obsidian vault, a synced folder, etc.
#
# Point a hook at it from config.toml:
#   commands = ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-file.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$file = $env:PHONEME_NOTES_FILE
if ([string]::IsNullOrWhiteSpace($file)) {
    $file = Join-Path $env:USERPROFILE 'Documents\VoiceNotes.md'
}

$dir = Split-Path -Parent $file
if ($dir -and -not (Test-Path -LiteralPath $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

$stamp = Get-Date -Date $payload.timestamp -Format 'yyyy-MM-dd HH:mm'
Add-Content -LiteralPath $file -Value "## $stamp`n$text`n" -Encoding UTF8
Write-Output "Appended to $file"
