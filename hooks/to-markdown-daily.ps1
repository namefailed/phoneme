# to-markdown-daily.ps1 - Obsidian-style daily note.
# Appends to ~/Documents/notes/YYYY-MM-DD.md

$payload = $input | Out-String | ConvertFrom-Json
$date    = Get-Date -Date $payload.timestamp -Format "yyyy-MM-dd"
$time    = Get-Date -Date $payload.timestamp -Format "HH:mm"
$dir     = Join-Path $env:USERPROFILE "Documents\notes"
$file    = Join-Path $dir "$date.md"

if (-not (Test-Path $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}
if (-not (Test-Path $file)) {
    Set-Content -Path $file -Value "# $date`n`n" -Encoding UTF8
}

$id   = $payload.id
$text = $payload.transcript
Add-Content -Path $file -Value "- **$time** - $text ^$id" -Encoding UTF8
Write-Host "appended to $file"
