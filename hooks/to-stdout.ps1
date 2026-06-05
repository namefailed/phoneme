# to-stdout.ps1 — default hook. Echoes the transcript to stdout.
#
# Every hook receives the recording as a JSON object on stdin (see docs/hooks.md
# for the schema). Phoneme treats a non-zero exit code as a FAILED hook, so we
# enable strict error handling: any error stops the script with a non-zero exit
# instead of silently "succeeding".

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$payload = $input | Out-String | ConvertFrom-Json
Write-Output $payload.transcript
