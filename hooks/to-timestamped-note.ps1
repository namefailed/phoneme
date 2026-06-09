# to-timestamped-note.ps1 — saves EACH transcript to its own timestamped file,
# one file per recording (great for a flat "voice memos" archive you can grep,
# sync, or feed into another tool). Writes Markdown with a small front-matter
# header by default.
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_NOTES_DIR   folder to write notes into.
#                       Default: ~/Documents/phoneme-notes
#   PHONEME_NOTES_EXT   "md" (default) writes Markdown with a front-matter header;
#                       "txt" writes the bare transcript with no header.
#
# Files are named <id>.<ext> (e.g. 20260519T143500823.md). The recording id is
# unique per recording, so notes never collide or overwrite each other.
#
# Point a hook at it from config.toml:
#   commands = ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-timestamped-note.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$dir = $env:PHONEME_NOTES_DIR
if ([string]::IsNullOrWhiteSpace($dir)) {
    $dir = Join-Path $env:USERPROFILE 'Documents\phoneme-notes'
}
if (-not (Test-Path -LiteralPath $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

$ext = $env:PHONEME_NOTES_EXT
if ([string]::IsNullOrWhiteSpace($ext)) { $ext = 'md' }
$ext = $ext.TrimStart('.').ToLower()

$file = Join-Path $dir "$($payload.id).$ext"
$secs = [math]::Round($payload.duration_ms / 1000.0, 1)

if ($ext -eq 'md') {
    # YAML front matter keeps the metadata machine-readable while staying
    # human-friendly in Obsidian/static-site generators.
    $content = @"
---
id: $($payload.id)
date: $($payload.timestamp)
model: $($payload.model)
duration_s: $secs
audio: "$($payload.audio_path)"
---

$text
"@
} else {
    $content = $text
}

Set-Content -LiteralPath $file -Value $content -Encoding UTF8
Write-Output "Wrote $file"
