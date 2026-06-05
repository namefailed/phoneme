# to-clipboard.ps1 — copies the transcript to the Windows clipboard, so you can
# paste a voice note straight into any app.
#
# Set as your hook command in config.toml:
#   commands = ["powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-clipboard.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$payload = $input | Out-String | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) {
    Write-Output "Empty transcript; nothing copied."
    exit 0
}

Set-Clipboard -Value $text
$preview = $text.Substring(0, [Math]::Min(60, $text.Length))
Write-Output "Copied to clipboard: $preview"
