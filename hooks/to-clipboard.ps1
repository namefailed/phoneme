# to-clipboard.ps1 — Copies the transcript to the Windows clipboard.
# Useful for immediately pasting a voice note anywhere.
#
# Usage: set as your hook command in config.toml:
#   commands = ["powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-clipboard.ps1"]

$payload = $input | Out-String | ConvertFrom-Json
Set-Clipboard -Value $payload.transcript
Write-Output "Copied to clipboard: $($payload.transcript.Substring(0, [Math]::Min(60, $payload.transcript.Length)))..."
