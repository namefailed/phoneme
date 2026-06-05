# to-webhook.ps1 — POST the transcript to a webhook (Discord, Slack, n8n, Zapier,
# your own server — anything that accepts a JSON body).
#
# Set the target URL in the PHONEME_WEBHOOK_URL environment variable. The body
# below is Discord-shaped (`{ "content": ... }`); for Slack use `{ "text": ... }`,
# and for your own service shape it however you like.
#
# This shows off Phoneme's reach: a spoken note can land in a team channel, a
# task queue, or an automation pipeline the instant you stop talking.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$payload = $input | Out-String | ConvertFrom-Json
$text = [string]$payload.transcript
if ([string]::IsNullOrWhiteSpace($text)) { exit 0 }

$url = $env:PHONEME_WEBHOOK_URL
if ([string]::IsNullOrWhiteSpace($url)) {
    Write-Error 'PHONEME_WEBHOOK_URL is not set. Set it to your webhook URL.'
}

$body = @{ content = $text } | ConvertTo-Json -Depth 4
Invoke-RestMethod -Uri $url -Method Post -Body $body -ContentType 'application/json' | Out-Null
Write-Output "Posted transcript to webhook."
