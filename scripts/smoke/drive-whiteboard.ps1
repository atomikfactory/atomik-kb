# Whiteboard pass (ADR-0033) on the real desktop app under the PRODUCTION CSP. `tauri dev` loads the
# page from Vite and does not enforce the CSP of tauri.conf.json, so this runs the built binary:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   .\drive-whiteboard.ps1 -Step launch    fresh data directory out\whiteboard-data, onboarding skipped, "Smoke page"
#   .\drive-whiteboard.ps1 -Step insert    /whiteboard in the page: an empty block with its hint, no library mounted
#   .\drive-whiteboard.ps1 -Step draw      Edit: the full-screen editor opens and a rectangle and the text
#                                          "Hello atomik" are drawn with real mouse and key input (left open)
#   .\drive-whiteboard.ps1 -Step finish    Done: the editor closes and the picture in the page shows the drawing
#   .\drive-whiteboard.ps1 -Step restart   stops the app and starts it again on the same data
#   .\drive-whiteboard.ps1 -Step verify    the picture is still there, the editor opens on the saved drawing
#   .\drive-whiteboard.ps1 -Step lock      turns the app lock on (drive-m4.ps1 kit, lock, unlock), opens the editor with a change just
#                                          made, "Lock now": the lock screen shows and nothing of the drawing is left in the page;
#                                          after the unlock both rectangles are back
#   .\drive-whiteboard.ps1 -Step all       launch, insert, draw, finish, restart, verify
#   (stop with stop.ps1, which also kills this binary)
#
# The app is started with --remote-debugging-port (WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS) so that
# drive-whiteboard.mjs can send trusted mouse and key events to Excalidraw's canvas, read the page, and
# record every request, console message and content-security-policy violation the page makes: a phase
# fails on any request to another host, any violation or failed request, and any console error.
# The onboarding and the page tree are driven with UI Automation and the window is captured with
# PrintWindow, like the other passes (never SendKeys). Screenshots and reports land in out\ (git-ignored):
# whiteboard-*.png, whiteboard-*.json, whiteboard.err.log.
param([string]$Step = "all", [int]$Port = 9333)
$ErrorActionPreference = "Continue"
$wbStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

function Launch-Built([switch]$Keep) {
  . (Join-Path $repo "scripts\dev-env.ps1")
  $out = Join-Path $PSScriptRoot "out"
  New-Item -ItemType Directory -Force $out | Out-Null
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $data = Join-Path $out "whiteboard-data"
  if ((Test-Path $data) -and -not $Keep) { Remove-Item -Recurse -Force $data }
  New-Item -ItemType Directory -Force $data | Out-Null
  $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
  if (-not (Test-Path $exe)) { "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop"; exit 1 }
  $env:KB_DATA_DIR = $data
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=info"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
  $proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") `
    -RedirectStandardError "$out\whiteboard.err.log" -RedirectStandardOutput "$out\whiteboard.out.log"
  Remove-Item Env:KB_DATA_DIR
  Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 2
    $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if ($p -and $p.MainWindowHandle -ne 0) { "app window up: pid=$($p.Id)"; Start-Sleep -Seconds 5; return }
  }
  "no app window"; exit 1
}
if ($wbStep -eq "launch" -or $wbStep -eq "all") { Launch-Built }
if ($wbStep -eq "restart") { Launch-Built -Keep }

# Dot-sourcing the helpers runs their own `param`, which overwrites $Step: ours is in $wbStep.
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$env:CDP_PORT = "$Port"

function Open-SmokePage() {
  $item = Find-El "TreeItem" $pageTitle 10
  if (-not $item) { Fail "no '$pageTitle' in the tree" }
  Select-El $item
  Start-Sleep -Seconds 2
}

function Run-Phase([string]$phase) {
  $report = "$out\whiteboard-$phase.json"
  $lines = & node "$PSScriptRoot\drive-whiteboard.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    Capture-Window $hwnd "$out\whiteboard-$phase-failed.png"
    Fail "phase '$phase' failed (see $report)"
  }
}

switch ($wbStep) {
  { $_ -in "launch", "all" } {
    # Onboarding: name the workspace, create it, skip the recovery kit for now, then a first page.
    $name = Find-El "Edit" "Workspace name" 20
    if ($name) {
      Set-El $name "Smoke workspace"
      Start-Sleep -Milliseconds 300
      Invoke-El (Find-El "Button" "Create workspace" 5)
      Start-Sleep -Seconds 3
    }
    $later = Find-El "Button" "Do this later" 6
    if ($later) { Invoke-El $later; Start-Sleep -Seconds 2 }
    Invoke-El (Find-El "Button" "New page" 10)
    Start-Sleep -Seconds 2
    & "$PSScriptRoot\drive-m4.ps1" -Step content
    Capture-Window $hwnd "$out\whiteboard-launched.png"
  }
}

switch ($wbStep) {
  { $_ -in "insert", "all" } {
    Open-SmokePage
    Run-Phase "insert"
    Capture-Window $hwnd "$out\whiteboard-inserted.png"
  }
}

switch ($wbStep) {
  { $_ -in "draw", "all" } {
    if ($wbStep -ne "all") { Open-SmokePage }
    Run-Phase "draw"
    Capture-Window $hwnd "$out\whiteboard-drawn.png"
  }
}

switch ($wbStep) {
  { $_ -in "finish", "all" } {
    Run-Phase "finish"
    Capture-Window $hwnd "$out\whiteboard-in-page.png"
  }
}

switch ($wbStep) {
  { $_ -in "restart", "all" } {
    if ($wbStep -eq "all") {
      Launch-Built -Keep
      . "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
      $env:CDP_PORT = "$Port"
    }
    Open-SmokePage
  }
}

switch ($wbStep) {
  { $_ -in "verify", "restart", "all" } {
    if ($wbStep -eq "verify") { Open-SmokePage }
    Run-Phase "verify"
    Capture-Window $hwnd "$out\whiteboard-restarted.png"
  }
}

switch ($wbStep) {
  "lock" {
    & "$PSScriptRoot\drive-m4.ps1" -Step kit
    & "$PSScriptRoot\drive-m4.ps1" -Step lock
    & "$PSScriptRoot\drive-m4.ps1" -Step unlock
    Open-SmokePage
    Run-Phase "open"
    Capture-Window $hwnd "$out\whiteboard-before-lock.png"
    $lockNow = Find-El "Button" "Lock now" 10
    if (-not $lockNow) { Fail "no 'Lock now' button (app lock off?)" }
    Invoke-El $lockNow
    Start-Sleep -Seconds 3
    if (-not (Find-El "Button" "Unlock" 20)) { Fail "no lock screen after Lock now" }
    Capture-Window $hwnd "$out\whiteboard-locked.png"
    Run-Phase "locked"
    & "$PSScriptRoot\drive-m4.ps1" -Step unlock
    Open-SmokePage
    Run-Phase "unlocked"
    Capture-Window $hwnd "$out\whiteboard-unlocked.png"
  }
}
