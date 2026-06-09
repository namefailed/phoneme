# to-markdown-daily.ps1 — Obsidian-style daily note. Appends each transcript as
# a timestamped bullet to <folder>/YYYY-MM-DD.md, creating the file with a
# top-level date heading the first time it sees a new day. The trailing block
# reference `^id` lets you link to the exact note from elsewhere in Obsidian.
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_DAILY_DIR   folder to write daily notes into.
#                       Default: ~/Documents/notes
#                       Point it at your Obsidian vault's daily-notes folder,
#                       e.g.  C:\Users\you\Obsidian\Vault\Daily
#
# Point a hook at it from config.toml:
#   commands = ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-markdown-daily.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

# Collapse newlines so a multi-line transcript stays on one bullet.
$text = ($text -replace '\r?\n', ' ').Trim()

$date = Get-Date -Date $payload.timestamp -Format 'yyyy-MM-dd'
$time = Get-Date -Date $payload.timestamp -Format 'HH:mm'

$dir = $env:PHONEME_DAILY_DIR
if ([string]::IsNullOrWhiteSpace($dir)) {
    $dir = Join-Path $env:USERPROFILE 'Documents\notes'
}
$file = Join-Path $dir "$date.md"

if (-not (Test-Path -LiteralPath $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}
if (-not (Test-Path -LiteralPath $file)) {
    Set-Content -LiteralPath $file -Value "# $date`n`n" -Encoding UTF8
}

Add-Content -LiteralPath $file -Value "- **$time** - $text ^$($payload.id)" -Encoding UTF8
Write-Output "Appended to $file"
