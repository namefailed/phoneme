# summarize-with-ollama.ps1 — turn a rambling voice note into a tidy summary +
# action items using a LOCAL Ollama model, then save it. 100% offline.
#
# This is the kind of thing Phoneme makes trivial: speak freely, and a local LLM
# distills it — no cloud, no API keys, no data leaving your machine.
#
# Requires Ollama running locally (https://ollama.ai). Pick the model with the
# PHONEME_OLLAMA_MODEL env var (default: llama3.2:3b — pull it with
# `ollama pull llama3.2:3b`). Output goes to ~/Documents/notes/YYYY-MM-DD-summaries.md.
#
# Note: Phoneme also has built-in LLM post-processing (Settings → Post-Processing).
# This hook is a standalone example you can adapt — e.g. summarize to a different
# file, or only when a keyword matches (see keyword-triggered hooks in settings).

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$payload = $input | Out-String | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$model = $env:PHONEME_OLLAMA_MODEL
if ([string]::IsNullOrWhiteSpace($model)) { $model = 'llama3.2:3b' }

$prompt = @"
Summarize the following voice note in 1-2 sentences, then list any action items
as a bullet list (or write "No action items."). Be concise.

Voice note:
$text
"@

$reqBody = @{ model = $model; prompt = $prompt; stream = $false } | ConvertTo-Json -Depth 4
$resp = Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/generate' -Method Post -Body $reqBody -ContentType 'application/json'
$summary = [string]$resp.response

$dir = Join-Path $env:USERPROFILE 'Documents\notes'
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
$date = Get-Date -Date $payload.timestamp -Format 'yyyy-MM-dd'
$time = Get-Date -Date $payload.timestamp -Format 'HH:mm'
$file = Join-Path $dir "$date-summaries.md"
Add-Content -Path $file -Value "## $time`n$($summary.Trim())`n" -Encoding UTF8
Write-Output "Summarized to $file"
