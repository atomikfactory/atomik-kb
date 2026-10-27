# Page width pass (ADR-0035: every page fills the main area, text and blocks alike) on the BUILT desktop app, the real
# WebView2 at real window sizes under the production CSP:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   .\drive-width.ps1 -Step launch    fresh data directory out\width-data, onboarding skipped; the marker of the document is set
#   .\drive-width.ps1 -Step page      a page "Wide": paragraph, heading, table, two columns (a table inside the left one), gallery,
#                                     whiteboard, table view (three pages inside it), board view and a closing paragraph, typed with trusted input
#   .\drive-width.ps1 -Step sizes     1100x760, 1500x900, maximised, restored and 800x600: the boxes of the section, the header, every
#                                     top-level block, the nested table and the table view's grid, horizontal scroll; F11 probed for a real full screen
#   .\drive-width.ps1 -Step handle    the block handle over a paragraph, the table, the columns and the table view
#   .\drive-width.ps1 -Step floating  the format menu, the table and columns toolbars and the slash menu sit beside their blocks
#   .\drive-width.ps1 -Step home      the recovery-kit banner and the Home screen start where the page's text does
#   .\drive-width.ps1 -Step dark      the maximised page in the dark theme
#   .\drive-width.ps1 -Step menu      page options has no "Full width" item; the maximised layout is recorded
#   .\drive-width.ps1 -Step restart   stops this binary and starts it again on the same data: "Wide" has the same layout
#   .\drive-width.ps1 -Step verify    the summary of every phase in out\width-report.json
#   .\drive-width.ps1 -Step all       launch, page, sizes, handle, floating, home, dark, menu, restart, verify; then stops the binary
#
# Like drive-tabs.ps1 the app is started with --remote-debugging-port, and drive-width.mjs sends trusted mouse and key
# events to the page over the WebView2 DevTools port (cdp.mjs), reads the boxes of the page, and records every
# request, console message and content-security-policy violation: a phase fails on a request to another host, a
# violation, a failed request, a console error, or when the document of the webview was replaced.
# The window is resized with Win32 (this script's own `-Step window`, called by the .mjs: MoveWindow for a size,
# ShowWindow SW_MAXIMIZE / SW_RESTORE, a posted F11), and captured with PrintWindow (capture.ps1). The onboarding is
# driven with UI Automation (never SendKeys).
# Output lands in out\ (git-ignored): width-*.png, width-*.json, width.err.log. A failing phase does not stop the
# others (the report says which failed); the exit code is 1 when any failed.
param([string]$Step = "all", [int]$Port = 9333, [string]$Mode = "info", [int]$Width = 0, [int]$Height = 0)
$ErrorActionPreference = "Continue"
$widthStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$failedPhases = @()

# ---- the window helper, called by drive-width.mjs ---------------------------------------------------------------
#   -Step window -Mode info                  the rectangle (physical pixels), whether it is maximised, its frame
#   -Step window -Mode size -Width W -Height H   restores it if maximised, then sets the OUTER size (physical pixels)
#   -Step window -Mode maximise | restore    ShowWindow SW_MAXIMIZE / SW_RESTORE
#   -Step window -Mode f11                   posts F11 (key down, key up) to the webview's window
# Prints one JSON line.
if ($widthStep -eq "window") {
  Add-Type -AssemblyName System.Windows.Forms
  if (-not ("WidthWin" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class WidthWin {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool repaint);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int index);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  public static IntPtr Browser(IntPtr root) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(root, (h, l) => { var c = new StringBuilder(256); GetClassName(h, c, 256); if (c.ToString() == "Chrome_WidgetWin_1") { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@
  }
  # Physical pixels, whatever the scale of the display (-4 = per-monitor aware v2).
  [WidthWin]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
  $app = Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if (-not $app) { '{"error":"no app window"}'; exit 1 }
  $h = $app.MainWindowHandle
  $r = New-Object WidthWin+RECT
  [WidthWin]::GetWindowRect($h, [ref]$r) | Out-Null
  switch ($Mode) {
    "size" {
      if ([WidthWin]::IsZoomed($h)) { [WidthWin]::ShowWindow($h, 9) | Out-Null; Start-Sleep -Milliseconds 400; [WidthWin]::GetWindowRect($h, [ref]$r) | Out-Null }
      # Keep the top left corner where it is, but on screen.
      $x = [Math]::Max(0, $r.Left); $y = [Math]::Max(0, $r.Top)
      [WidthWin]::MoveWindow($h, $x, $y, $Width, $Height, $true) | Out-Null
    }
    "maximise" { [WidthWin]::ShowWindow($h, 3) | Out-Null }
    "restore" { [WidthWin]::ShowWindow($h, 9) | Out-Null }
    "f11" {
      $web = [WidthWin]::Browser($h)
      if ($web -eq [IntPtr]::Zero) { $web = $h }
      [WidthWin]::PostMessage($web, 0x0100, [IntPtr]0x7A, [IntPtr]0x00570001) | Out-Null
      [WidthWin]::PostMessage($web, 0x0101, [IntPtr]0x7A, [IntPtr]::new(-1068236799)) | Out-Null
    }
  }
  Start-Sleep -Milliseconds 500
  [WidthWin]::GetWindowRect($h, [ref]$r) | Out-Null
  $style = [WidthWin]::GetWindowLong($h, -16)
  $screen = [System.Windows.Forms.Screen]::FromHandle($h)
  [pscustomobject]@{
    left = $r.Left; top = $r.Top; width = $r.Right - $r.Left; height = $r.Bottom - $r.Top
    zoomed = [WidthWin]::IsZoomed($h)
    caption = (($style -band 0x00C00000) -eq 0x00C00000)
    monitor = @{ width = $screen.Bounds.Width; height = $screen.Bounds.Height; workWidth = $screen.WorkingArea.Width; workHeight = $screen.WorkingArea.Height }
  } | ConvertTo-Json -Compress
  exit 0
}

$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null

function Launch-Built([switch]$Keep) {
  . (Join-Path $repo "scripts\dev-env.ps1")
  # Only this binary is stopped (never node: a Vite dev server may be running for another session).
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $data = Join-Path $out "width-data"
  if ((Test-Path $data) -and -not $Keep) { Remove-Item -Recurse -Force $data }
  New-Item -ItemType Directory -Force $data | Out-Null
  $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
  if (-not (Test-Path $exe)) { "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop"; exit 1 }
  $env:KB_DATA_DIR = $data
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=info"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
  $proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") `
    -RedirectStandardError "$out\width.err.log" -RedirectStandardOutput "$out\width.out.log"
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
if ($widthStep -eq "verify") {
  $env:WIDTH_OUT_DIR = $out
  & node "$PSScriptRoot\drive-width.mjs" "verify" "$out\width-report.json" 2>&1 | ForEach-Object { "$_" }
  exit $LASTEXITCODE
}
if ($widthStep -eq "launch" -or $widthStep -eq "all") { Launch-Built }
if ($widthStep -eq "restart") { Launch-Built -Keep }

# Dot-sourcing the helpers runs their own `param`, which overwrites $Step: ours is in $widthStep.
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$env:CDP_PORT = "$Port"
# The .mjs captures the window and resizes it itself, at the moments it needs (it runs this script's helpers in PowerShell).
$env:WIDTH_CAPTURE_PS1 = Join-Path $PSScriptRoot "capture.ps1"
$env:WIDTH_PS1 = $PSCommandPath
$env:WIDTH_OUT_DIR = $out

function Run-Phase([string]$phase) {
  $report = "$out\width-$phase.json"
  $lines = & node "$PSScriptRoot\drive-width.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $phase
    if (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue) { Capture-Window $hwnd "$out\width-$phase-failed.png" }
    "PHASE FAILED: $phase (see $report)"
  }
}

if ($widthStep -in "launch", "all") {
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
  Capture-Window $hwnd "$out\width-launched.png"
}

if ($widthStep -in "page", "all") { Run-Phase "page" }
if ($widthStep -in "sizes", "all") { Run-Phase "sizes" }
if ($widthStep -in "handle", "all") { Run-Phase "handle" }
if ($widthStep -in "floating", "all") { Run-Phase "floating" }
if ($widthStep -in "home", "all") { Run-Phase "home" }
if ($widthStep -in "dark", "all") { Run-Phase "dark" }
if ($widthStep -in "menu", "all") { Run-Phase "menu" }

if ($widthStep -in "restart", "all") {
  if ($widthStep -eq "all") {
    Launch-Built -Keep
    . "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
    $env:CDP_PORT = "$Port"
    $env:WIDTH_CAPTURE_PS1 = Join-Path $PSScriptRoot "capture.ps1"
    $env:WIDTH_PS1 = Join-Path $PSScriptRoot "drive-width.ps1"
    $env:WIDTH_OUT_DIR = $out
  }
  Run-Phase "restart"
}

if ($widthStep -in "verify", "all") {
  & node "$PSScriptRoot\drive-width.mjs" "verify" "$out\width-report.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "verify" }
}

if ($widthStep -eq "all") {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  "stopped the app (node and Vite were left alone)"
}
if ($failedPhases.Count -gt 0) { "FAILED: $($failedPhases -join ', ')"; exit 1 }
"width pass: all phases ok"
