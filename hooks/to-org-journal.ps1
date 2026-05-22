# to-org-journal.ps1 - appends each transcript to ~/Documents/org/journal.org
# under today's date heading. Matches Doom Emacs / Denote workflow.

$payload = $input | Out-String | ConvertFrom-Json
$journal = Join-Path $env:USERPROFILE "Documents\org\journal.org"
$today   = (Get-Date -Date $payload.timestamp -Format "yyyy-MM-dd ddd")
$ts      = (Get-Date -Date $payload.timestamp -Format "HH:mm")

# Ensure the file and today's heading exist.
# IMPORTANT: -Encoding UTF8 keeps the file as UTF-8 without BOM so Emacs/Org
# (and other Unix tools) don't see a BOM at the top.
if (-not (Test-Path $journal)) {
    New-Item -ItemType File -Path $journal -Force | Out-Null
}
$content = Get-Content $journal -Raw -ErrorAction SilentlyContinue
if ($null -eq $content -or $content -notmatch "(?m)^\* $today") {
    Add-Content -Path $journal -Value "`n* $today`n" -Encoding UTF8
}

# Append the entry.
$id    = $payload.id
$audio = $payload.audio_path
$text  = $payload.transcript

$entry = @"
** $ts $text
   :PROPERTIES:
   :PHONEME_ID: $id
   :AUDIO: $audio
   :END:
"@

Add-Content -Path $journal -Value $entry -Encoding UTF8
Write-Host "appended to $journal"
