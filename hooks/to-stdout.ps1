# to-stdout.ps1 - default hook. Echoes the transcript to stdout.
# Useful for testing the hook pipeline.

$payload = $input | Out-String | ConvertFrom-Json
Write-Output $payload.transcript
