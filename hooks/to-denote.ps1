# to-denote.ps1 - creates a Denote-flavored note file with proper filename
# slug under ~/Documents/org/notes/.

$payload = $input | Out-String | ConvertFrom-Json
$id    = Get-Date -Date $payload.timestamp -Format "yyyyMMddTHHmmss"
$title = (($payload.transcript -split '\s+' | Select-Object -First 6) -join ' ').Trim()
if ([string]::IsNullOrWhiteSpace($title)) { $title = "voice-note" }
$slug = ($title -replace '[^a-zA-Z0-9 ]', '' -replace '\s+', '-').ToLower()
$dir  = Join-Path $env:USERPROFILE "Documents\org\notes"
$path = Join-Path $dir "$id--$slug__voice.org"

if (-not (Test-Path $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
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
