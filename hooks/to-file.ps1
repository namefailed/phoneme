# to-file.ps1 — the simplest "just save my voice notes somewhere" hook.
# Appends every transcript, with a timestamp, to a single running Markdown file.
#
# Destination defaults to ~/Documents/VoiceNotes.md; override it by setting the
# PHONEME_NOTES_FILE environment variable (e.g. to a file inside your vault).

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$payload = $input | Out-String | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$file = $env:PHONEME_NOTES_FILE
if ([string]::IsNullOrWhiteSpace($file)) {
    $file = Join-Path $env:USERPROFILE 'Documents\VoiceNotes.md'
}

$dir = Split-Path -Parent $file
if ($dir -and -not (Test-Path $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

$stamp = Get-Date -Date $payload.timestamp -Format 'yyyy-MM-dd HH:mm'
Add-Content -Path $file -Value "## $stamp`n$text`n" -Encoding UTF8
Write-Output "appended to $file"
