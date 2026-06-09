# notify-desktop.ps1 — shows a Windows desktop notification with a snippet of the
# transcript, so you get visible confirmation that a recording landed (handy
# when dictating with the window minimized).
#
# Uses the built-in System.Windows.Forms balloon notification — no extra modules
# to install. (If you have the BurntToast module and prefer modern toasts, this
# is an easy script to adapt.)
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_NOTIFY_CHARS   max characters of the transcript to show.
#                          Default: 120
#
# Point a hook at it from config.toml:
#   commands = ["powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/notify-desktop.ps1"]

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$max = 120
if (-not [string]::IsNullOrWhiteSpace($env:PHONEME_NOTIFY_CHARS)) {
    [int]::TryParse($env:PHONEME_NOTIFY_CHARS, [ref]$max) | Out-Null
    if ($max -le 0) { $max = 120 }
}

$snippet = ($text -replace '\r?\n', ' ').Trim()
if ($snippet.Length -gt $max) {
    $snippet = $snippet.Substring(0, $max).TrimEnd() + '...'
}

Add-Type -AssemblyName System.Windows.Forms
$notify = New-Object System.Windows.Forms.NotifyIcon
try {
    $notify.Icon = [System.Drawing.SystemIcons]::Information
    $notify.BalloonTipTitle = 'Phoneme — transcript ready'
    $notify.BalloonTipText = $snippet
    $notify.Visible = $true
    $notify.ShowBalloonTip(5000)
    # The balloon is delivered to the shell asynchronously; give it a moment to
    # register before we dispose the icon, or it may never appear.
    Start-Sleep -Milliseconds 800
} finally {
    $notify.Dispose()
}

Write-Output "Notified: $snippet"
