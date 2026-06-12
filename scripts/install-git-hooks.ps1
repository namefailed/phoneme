# One-time setup: point this repo at scripts/git-hooks so commit-msg blocks
# AI-tool attribution lines. Safe to re-run.
$ErrorActionPreference = "Stop"
$repoRoot = (git -C $PSScriptRoot rev-parse --show-toplevel)
Set-Location $repoRoot
git config core.hooksPath scripts/git-hooks
Write-Host "Installed git hooks from scripts/git-hooks (core.hooksPath)."
