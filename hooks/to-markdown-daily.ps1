# to-markdown-daily.ps1 — Obsidian-style daily note. Appends each transcript as
# a timestamped bullet to ~/Documents/notes/YYYY-MM-DD.md (the block-ref `^id`
# lets you link to the exact note in Obsidian).

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$payload = $input | Out-String | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$date = Get-Date -Date $payload.timestamp -Format "yyyy-MM-dd"
$time = Get-Date -Date $payload.timestamp -Format "HH:mm"
$dir  = Join-Path $env:USERPROFILE "Documents\notes"
$file = Join-Path $dir "$date.md"

if (-not (Test-Path $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}
if (-not (Test-Path $file)) {
    Set-Content -Path $file -Value "# $date`n`n" -Encoding UTF8
}

Add-Content -Path $file -Value "- **$time** - $text ^$($payload.id)" -Encoding UTF8
Write-Output "appended to $file"
