# Tabs pass (ADR-0034) on the BUILT desktop app, the real WebView2 under the production CSP:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   .\drive-tabs.ps1 -Step launch    fresh data directory out\tabs-data, onboarding skipped; the marker of the document is set
#   .\drive-tabs.ps1 -Step pages     four pages (Alpha, Beta, Gamma, Long) through the sidebar; Long is made taller than the window
#   .\drive-tabs.ps1 -Step tabs      click, Ctrl+click, Ctrl+Shift+click, a click on an open page, Pin tab, a middle click on a tab
#   .\drive-tabs.ps1 -Step keys      Ctrl+T, W, Shift+T, Tab, PageUp/Down, 1 to 9, Shift+PageUp/Down, Alt+arrows, in the editor and in the title field
#   .\drive-tabs.ps1 -Step mouse     the mouse back and forward buttons go back and forward in the tab and not in the webview's history
#   .\drive-tabs.ps1 -Step scroll    the scroll position of a long page comes back after a tab switch and after back and forward
#   .\drive-tabs.ps1 -Step drag      a tab dragged to a new place (HTML5 drag and drop, the drag intercepted over the DevTools port)
#   .\drive-tabs.ps1 -Step views     12 more tabs (the strip overflows), the sidebar hidden with Ctrl+\, the light and the dark theme
#   .\drive-tabs.ps1 -Step restart   stops this binary and starts it again on the same data: the tabs come back
#   .\drive-tabs.ps1 -Step verify    the summary of every phase in out\tabs-report.json
#   .\drive-tabs.ps1 -Step all       launch, pages, tabs, keys, mouse, scroll, drag, views, restart, verify
#
# Like drive-whiteboard.ps1 the app is started with --remote-debugging-port, and drive-tabs.mjs sends trusted
# mouse and key events to the page over the WebView2 DevTools port (cdp.mjs), reads what is on screen, and
# records every request, console message and content-security-policy violation: a phase fails on a request to
# another host, a violation, a failed request, a console error, or when the document of the webview was replaced
# (a marker set on `window` after the launch, and the address, are checked at the end of every phase).
# The onboarding is driven with UI Automation (never SendKeys), and the window is captured with PrintWindow
# (by the .mjs, at the moments the pass needs: a context menu open, an overflowing strip, ...).
# Output lands in out\ (git-ignored): tabs-*.png, tabs-*.json, tabs.err.log. A failing phase does not stop the
# others (the report says which failed); the exit code is 1 when any failed.
param([string]$Step = "all", [int]$Port = 9333)
$ErrorActionPreference = "Continue"
$tabsStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$failedPhases = @()

function Launch-Built([switch]$Keep) {
  . (Join-Path $repo "scripts\dev-env.ps1")
  $out = Join-Path $PSScriptRoot "out"
  New-Item -ItemType Directory -Force $out | Out-Null
  # Only this binary is stopped (never node: a Vite dev server may be running for another session).
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $data = Join-Path $out "tabs-data"
  if ((Test-Path $data) -and -not $Keep) { Remove-Item -Recurse -Force $data }
  New-Item -ItemType Directory -Force $data | Out-Null
  $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
  if (-not (Test-Path $exe)) { "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop"; exit 1 }
  $env:KB_DATA_DIR = $data
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=info"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
  $proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") `
    -RedirectStandardError "$out\tabs.err.log" -RedirectStandardOutput "$out\tabs.out.log"
  Remove-Item Env:KB_DATA_DIR
  Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 2
    $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if ($p -and $p.MainWindowHandle -ne 0) { "app window up: pid=$($p.Id)"; Start-Sleep -Seconds 5; return }
  }
  "no app window"; exit 1
}
# The summary reads the reports of the phases: no app needed.
if ($tabsStep -eq "verify") {
  $env:TABS_OUT_DIR = Join-Path $PSScriptRoot "out"
  & node "$PSScriptRoot\drive-tabs.mjs" "verify" "$env:TABS_OUT_DIR\tabs-report.json" 2>&1 | ForEach-Object { "$_" }
  exit $LASTEXITCODE
}
if ($tabsStep -eq "launch" -or $tabsStep -eq "all") { Launch-Built }
if ($tabsStep -eq "restart") { Launch-Built -Keep }

# Dot-sourcing the helpers runs their own `param`, which overwrites $Step: ours is in $tabsStep.
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$env:CDP_PORT = "$Port"
# The .mjs captures the window itself, at the moments it needs (it runs this helper in PowerShell).
$env:TABS_CAPTURE_PS1 = Join-Path $PSScriptRoot "capture.ps1"
$env:TABS_OUT_DIR = $out

function Run-Phase([string]$phase) {
  $report = "$out\tabs-$phase.json"
  $lines = & node "$PSScriptRoot\drive-tabs.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $phase
    if (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue) { Capture-Window $hwnd "$out\tabs-$phase-failed.png" }
    "PHASE FAILED: $phase (see $report)"
  }
}

if ($tabsStep -in "launch", "all") {
  # Onboarding: name the workspace, create it, skip the recovery kit for now.
  $name = Find-El "Edit" "Workspace name" 20
  if ($name) {
    Set-El $name "Smoke workspace"
    Start-Sleep -Milliseconds 300
    Invoke-El (Find-El "Button" "Create workspace" 5)
    Start-Sleep -Seconds 3
  }
  $later = Find-El "Button" "Do this later" 6
  if ($later) { Invoke-El $later; Start-Sleep -Seconds 2 }
  Run-Phase "launch"
  Capture-Window $hwnd "$out\tabs-launched.png"
}

if ($tabsStep -in "pages", "all") { Run-Phase "pages" }
if ($tabsStep -in "tabs", "all") { Run-Phase "tabs" }
if ($tabsStep -in "keys", "all") { Run-Phase "keys" }
if ($tabsStep -in "mouse", "all") { Run-Phase "mouse" }
if ($tabsStep -in "scroll", "all") { Run-Phase "scroll" }
if ($tabsStep -in "drag", "all") { Run-Phase "drag" }
if ($tabsStep -in "views", "all") { Run-Phase "views" }

if ($tabsStep -in "restart", "all") {
  if ($tabsStep -eq "all") {
    Launch-Built -Keep
    . "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
    $env:CDP_PORT = "$Port"
    $env:TABS_CAPTURE_PS1 = Join-Path $PSScriptRoot "capture.ps1"
    $env:TABS_OUT_DIR = $out
  }
  Run-Phase "restart"
}

if ($tabsStep -in "verify", "all") {
  & node "$PSScriptRoot\drive-tabs.mjs" "verify" "$out\tabs-report.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "verify" }
}

if ($tabsStep -eq "all") {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  "stopped the app (node and Vite were left alone)"
}
if ($failedPhases.Count -gt 0) { "FAILED: $($failedPhases -join ', ')"; exit 1 }
"tabs pass: all phases ok"
