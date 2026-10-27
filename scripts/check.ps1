# The verification run of this repository (there is no hosted CI for now).
#
#   . .\scripts\dev-env.ps1; .\scripts\check.ps1            # everything
#   .\scripts\check.ps1 -SkipWeb                            # Rust side only
#   .\scripts\check.ps1 -SkipRust                           # TypeScript side only
#
# Steps, in order: cargo fmt (check), clippy with warnings denied, every Rust test,
# wasm32 check of the platform-neutral crates (the full `cargo check-wasm` alias when
# clang is installed, the clang-free crates otherwise), cargo deny, ts-rs output up to
# date, pnpm lint, typecheck, test and build. Stops at the first failure.

[CmdletBinding()]
param(
  [switch]$SkipRust,
  [switch]$SkipWeb
)

# Not "Stop": in Windows PowerShell 5.1 a native command's stderr line becomes an error
# record whenever output is redirected, and cargo prints its progress to stderr. Every
# step checks the exit code instead.
$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "dev-env.ps1")
Set-Location (Join-Path $PSScriptRoot "..")

function Step {
  param([string]$Name, [scriptblock]$Command)
  Write-Host ""
  Write-Host "== $Name" -ForegroundColor Cyan
  & $Command
  if ($LASTEXITCODE -ne 0) {
    Write-Host "FAILED: $Name" -ForegroundColor Red
    exit 1
  }
}

if (-not $SkipRust) {
  Step "cargo fmt" { cargo fmt --all --check }
  # Lint flags go to clippy only (RUSTFLAGS would invalidate every cached artifact).
  Step "cargo clippy (warnings denied)" { cargo clippy --workspace --all-targets -- -D warnings }
  Step "cargo test" { cargo test --workspace }
  if (Get-Command clang -ErrorAction SilentlyContinue) {
    Step "wasm32 check (all core crates)" { cargo check-wasm }
  } else {
    Write-Host "(no clang on this machine: checking the clang-free crates only)" -ForegroundColor Yellow
    Step "wasm32 check (clang-free crates)" {
      cargo check --target wasm32-unknown-unknown -p kb-types -p kb-crypto -p kb-keys -p kb-doc -p kb-model -p kb-sync -p kb-ai -p kb-mcp -p kb-wasm -p kb-speech -p kb-drive
    }
  }
  Step "cargo deny" { cargo deny check }
  Step "ts-rs output is up to date" {
    # Regenerate and compare with what was on disk: the generated TypeScript must be
    # the output of the current kb-types and kb-speech (whether or not it is committed yet).
    $dir = "packages/core-client/src/generated"
    $before = Get-ChildItem $dir -Recurse -File | Get-FileHash | Sort-Object Path | ForEach-Object { "$($_.Path) $($_.Hash)" }
    cargo test -p kb-types -p kb-speech --features ts --quiet
    if ($LASTEXITCODE -eq 0) {
      $after = Get-ChildItem $dir -Recurse -File | Get-FileHash | Sort-Object Path | ForEach-Object { "$($_.Path) $($_.Hash)" }
      if (Compare-Object $before $after) {
        Write-Host "The generated TypeScript in $dir was stale; it has been regenerated, review and commit it." -ForegroundColor Red
        cmd.exe /c exit 1
      }
    }
  }
}

if (-not $SkipWeb) {
  # `cmd /c` because pnpm is a .cmd shim that PowerShell's process start cannot run directly.
  Step "pnpm install (frozen lockfile)" { cmd.exe /c "pnpm install --frozen-lockfile" }
  Step "pnpm lint" { cmd.exe /c "pnpm lint" }
  Step "pnpm typecheck" { cmd.exe /c "pnpm typecheck" }
  Step "pnpm test" { cmd.exe /c "pnpm test" }
  Step "pnpm build" { cmd.exe /c "pnpm build" }
}

Write-Host ""
Write-Host "All checks passed." -ForegroundColor Green
