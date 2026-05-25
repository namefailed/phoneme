# to-org-journal.ps1 - appends each transcript to ~/Documents/org/journal.org
# under today's Log section. Matches the specific user's daily journal format.

$payload = $input | Out-String | ConvertFrom-Json
$journal = Join-Path $env:USERPROFILE "Documents\org\journal.org"

# Ensure the file exists
if (-not (Test-Path $journal)) {
    New-Item -ItemType File -Path $journal -Force | Out-Null
}

$dateObj = Get-Date -Date $payload.timestamp
$dayHeadingStr = "*** " + $dateObj.ToString("yyyy-MM-dd dddd")
$timestampStr  = "- [" + $dateObj.ToString("yyyy-MM-dd ddd HH:mm") + "]"
$transcript    = $payload.transcript
$audioPath     = $payload.audio_path -replace '\\', '/'

$newEntry = "$timestampStr $transcript [[file:$audioPath][(Audio)]]"

$lines = Get-Content $journal -ErrorAction SilentlyContinue
if ($null -eq $lines) { $lines = @() }

$dayFoundIndex = -1
$logFoundIndex = -1
$insertIndex = -1

# Scan the file to find the insertion point
for ($i = 0; $i -lt $lines.Count; $i++) {
    $line = $lines[$i]
    if ($line -match "^\*\*\* \d{4}-\d{2}-\d{2}") {
        if ($line -eq $dayHeadingStr) {
            $dayFoundIndex = $i
        } else {
            # Reset if we somehow found another day after (shouldn't happen in chronological journal, but just in case)
            $dayFoundIndex = -1
            $logFoundIndex = -1
        }
    }
    
    if ($dayFoundIndex -ne -1 -and $line -match "^\*\*\*\* Log") {
        $logFoundIndex = $i
    }

    if ($logFoundIndex -ne -1 -and $i -gt $logFoundIndex) {
        if ($line -match "^\*\*\*\* ") {
            # Hit the next section (like **** Tasks or **** EOD)
            $insertIndex = $i
            break
        }
    }
}

if ($dayFoundIndex -eq -1) {
    # The day doesn't exist at all. Append the whole day block.
    $lines += ""
    $lines += $dayHeadingStr
    $lines += ""
    $lines += "**** Trackers"
    $lines += "- Sleep:: /10"
    $lines += "- Mood:: /10"
    $lines += "- Energy:: /10"
    $lines += ""
    $lines += "**** Log"
    $lines += $newEntry
    $lines += ""
    $lines += "**** Tasks"
    $lines += ""
    $lines += "**** EOD"
    $lines += "***** Done"
    $lines += "-"
    $lines += ""
    $lines += "***** Next"
    $lines += "-"
    $lines += ""
} else {
    # Day exists
    if ($logFoundIndex -eq -1) {
        # Somehow day exists but Log doesn't. Just append Log to the end of the day block.
        # This is an edge case. For simplicity, we just append to the file.
        $lines += ""
        $lines += "**** Log"
        $lines += $newEntry
    } else {
        # Day and Log exist. Insert right before the next `**** ` section or at the end of the file.
        if ($insertIndex -ne -1) {
            # Walk back from $insertIndex to skip any empty lines before the heading
            $actualInsert = $insertIndex
            while ($actualInsert -gt $logFoundIndex -and [string]::IsNullOrWhiteSpace($lines[$actualInsert - 1])) {
                $actualInsert--
            }
            
            $newLines = @()
            for ($i = 0; $i -lt $actualInsert; $i++) { $newLines += $lines[$i] }
            $newLines += $newEntry
            for ($i = $actualInsert; $i -lt $lines.Count; $i++) { $newLines += $lines[$i] }
            $lines = $newLines
        } else {
            # No subsequent heading found, just append to the end.
            $lines += $newEntry
        }
    }
}

# Write back as UTF8 without BOM
[System.IO.File]::WriteAllLines($journal, $lines, [System.Text.Encoding]::UTF8)
Write-Host "appended to $journal"
