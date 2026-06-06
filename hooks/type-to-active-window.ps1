$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

# Phoneme passes the transcript as a JSON object on standard input.
$payload = $input | Out-String | ConvertFrom-Json
$transcript = $payload.transcript

# If the transcript is empty, do nothing
if ([string]::IsNullOrWhiteSpace($transcript)) {
    exit 0
}

# Add a trailing space to make continuous dictation flow better
$textToType = $transcript + " "

# Add the required assembly for SendKeys
Add-Type -AssemblyName System.Windows.Forms

# SendKeys has special characters that need to be escaped: + ^ % ~ ( ) { } [ ]
# We escape them by wrapping them in curly braces e.g., {+}
$escapedText = $textToType -replace '([+^%~()[\]{}])', '{$1}'

# Simulate typing the text into the currently focused window
[System.Windows.Forms.SendKeys]::SendWait($escapedText)

# Wait briefly for keys to dispatch
Start-Sleep -Milliseconds 50
