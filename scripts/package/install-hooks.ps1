# Enable (or disable) the versioned git hooks in .githooks/ for this clone.
#   .\scripts\package\install-hooks.ps1
#   .\scripts\package\install-hooks.ps1 -Uninstall
[CmdletBinding()]
param([switch]$Uninstall)
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if ($Uninstall) {
  git -C $root config --unset core.hooksPath 2>$null
  Write-Host "core.hooksPath unset; hooks disabled."
} else {
  git -C $root config core.hooksPath .githooks
  Write-Host "core.hooksPath = $(git -C $root config --get core.hooksPath)"
  Write-Host "post-commit will rebuild the installer in the background after commits touching apps/, packages/, crates/, adapters/ or the lockfiles."
  Write-Host "Skip once: set KB_SKIP_INSTALLER=1. Auto-install: git config atomik.autoInstall true"
}
