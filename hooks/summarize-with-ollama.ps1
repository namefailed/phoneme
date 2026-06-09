# summarize-with-ollama.ps1 — turn a rambling voice note into a tidy summary +
# action items using a LOCAL Ollama model, then save it. 100% offline.
#
# Speak freely, and a local LLM distills it — no cloud, no API keys, no data
# leaving your machine.
#
# Reads the recording as a JSON object on STDIN (see to-stdout.ps1 or
# docs/developer-guide/plugins_and_hooks.md for the full payload shape).
#
# Requires Ollama running locally (https://ollama.com). Output goes to
# <PHONEME_DAILY_DIR>/YYYY-MM-DD-summaries.md.
#
# ── Configure ───────────────────────────────────────────────────────────────
#   PHONEME_OLLAMA_MODEL   model to use. Default: llama3.2:3b
#                          (pull it once with `ollama pull llama3.2:3b`)
#   PHONEME_OLLAMA_URL     Ollama base URL. Default: http://127.0.0.1:11434
#   PHONEME_DAILY_DIR      output folder. Default: ~/Documents/notes
#
# Note: Phoneme also has built-in LLM post-processing (Settings → Post-Processing)
# that runs BEFORE hooks. This hook is a standalone example you can adapt — e.g.
# summarize to a different file, or only when a keyword matches (see
# keyword-triggered hooks in Settings → Action Hook).

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { Write-Error 'No payload received on stdin.' }

$payload = $raw | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$model = $env:PHONEME_OLLAMA_MODEL
if ([string]::IsNullOrWhiteSpace($model)) { $model = 'llama3.2:3b' }

$base = $env:PHONEME_OLLAMA_URL
if ([string]::IsNullOrWhiteSpace($base)) { $base = 'http://127.0.0.1:11434' }

$prompt = @"
Summarize the following voice note in 1-2 sentences, then list any action items
as a bullet list (or write "No action items."). Be concise.

Voice note:
$text
"@

$reqBody = @{ model = $model; prompt = $prompt; stream = $false } | ConvertTo-Json -Depth 4
try {
    $resp = Invoke-RestMethod -Uri "$base/api/generate" -Method Post -Body $reqBody -ContentType 'application/json'
} catch {
    Write-Error "Could not reach Ollama at $base. Is 'ollama serve' running and is '$model' pulled? ($_)"
}
$summary = [string]$resp.response

$dir = $env:PHONEME_DAILY_DIR
if ([string]::IsNullOrWhiteSpace($dir)) { $dir = Join-Path $env:USERPROFILE 'Documents\notes' }
if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

$date = Get-Date -Date $payload.timestamp -Format 'yyyy-MM-dd'
$time = Get-Date -Date $payload.timestamp -Format 'HH:mm'
$file = Join-Path $dir "$date-summaries.md"
Add-Content -LiteralPath $file -Value "## $time`n$($summary.Trim())`n" -Encoding UTF8
Write-Output "Summarized to $file"
