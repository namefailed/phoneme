# generate-icons.ps1
# Converts icon-source.svg into all required Phoneme icon sizes.
#
# Prerequisites: Inkscape or rsvg-convert on PATH, OR use the Tauri CLI approach.
#
# Tauri approach (recommended — handles all sizes automatically):
#   pnpm tauri icon scripts/icon-source.svg
#
# This script is a manual fallback using Inkscape:

$ErrorActionPreference = "Stop"
$script_dir  = Split-Path -Parent $MyInvocation.MyCommand.Path
$src         = Join-Path $script_dir "icon-source.svg"
$icons_dir   = Join-Path $script_dir "..\src-tauri\icons"

if (-not (Get-Command inkscape -ErrorAction SilentlyContinue)) {
    Write-Host "Inkscape not found. Run: pnpm tauri icon scripts/icon-source.svg instead." -ForegroundColor Yellow
    exit 1
}

# App icons
foreach ($size in @(32, 128, 256, 512, 1024)) {
    $out = Join-Path $icons_dir "${size}x${size}.png"
    inkscape --export-type=png --export-filename="$out" --export-width=$size --export-height=$size "$src"
    Write-Host "  Generated $out"
}

# icon.png (512x512 main)
inkscape --export-type=png --export-filename="$(Join-Path $icons_dir 'icon.png')" --export-width=512 --export-height=512 "$src"

# Tray icons — 32x32 variants with different accent colours via sed
# For now copy the same base icon at 32px; tray-recording/transcribing/error
# use the same shape — Tauri tray colours them via the OS on Windows.
foreach ($name in @("tray-idle", "tray-recording", "tray-transcribing", "tray-error")) {
    $out = Join-Path $icons_dir "${name}.png"
    inkscape --export-type=png --export-filename="$out" --export-width=32 --export-height=32 "$src"
    Write-Host "  Generated $out"
}

Write-Host "Done. Run 'pnpm tauri icon scripts/icon-source.svg' to also regenerate the .ico and macOS .icns." -ForegroundColor Green
