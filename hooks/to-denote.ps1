# to-denote.ps1 — ADVANCED example. Creates a Denote-flavoured Org note (with a
# proper `ID--slug__tags.org` filename) under <PHONEME_ORG_DIR>/notes/. For
# Emacs/Denote users; a template to adapt rather than a general default.
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_ORG_DIR   org root. Default: ~/Documents/org (notes go in <dir>/notes)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }
$payload = $raw | ConvertFrom-Json
$id    = Get-Date -Date $payload.timestamp -Format "yyyyMMddTHHmmss"
$title = (($payload.transcript -split '\s+' | Select-Object -First 6) -join ' ').Trim()
if ([string]::IsNullOrWhiteSpace($title)) { $title = "voice-note" }
$slug = ($title -replace '[^a-zA-Z0-9 ]', '' -replace '\s+', '-').ToLower()
$orgRoot = $env:PHONEME_ORG_DIR
if ([string]::IsNullOrWhiteSpace($orgRoot)) { $orgRoot = Join-Path $env:USERPROFILE 'Documents\org' }
$dir  = Join-Path $orgRoot "notes"

if (-not (Test-Path $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

# The timestamp id is only second-precision, so two recordings in the same
# second with similar opening words would map to one path. Uniquify the slug
# instead of silently overwriting the earlier note.
$path = Join-Path $dir "${id}--${slug}__voice.org"
$n = 1
while (Test-Path $path) {
    $path = Join-Path $dir "${id}--${slug}-$n__voice.org"
    $n++
}

$body = @"
#+title: $title
#+date: [$($payload.timestamp)]
#+identifier: $id
#+filetags: :voice:

$($payload.transcript)

* Source
- audio :: [[file:$($payload.audio_path)][$($payload.audio_path)]]
- phoneme_id :: $($payload.id)
- duration :: $([math]::Round($payload.duration_ms / 1000.0, 1))s
"@

Set-Content -Path $path -Value $body -Encoding UTF8
Write-Host "created $path"
