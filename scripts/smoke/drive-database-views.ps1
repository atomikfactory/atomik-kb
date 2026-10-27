# Database views (ADR-0054) real-app pass on the dev app (`pnpm tauri dev`), over the WebView2 DevTools port.
#
#   . .\scripts\dev-env.ps1; .\scripts\smoke\drive-database-views.ps1 [-Step all|launch|build|stop|verify] [-Keep]
#
# What it proves (drive-database-views.mjs, trusted input over CDP; the unit tests cover the logic):
#   build:  a page "Projects" with a table view of its children, Date properties "Due" and "Until" and a Status select in the
#           space schema, five child pages created through the view's "New page" (Due dates in the current month, one with
#           Until two days later), an image block (a PNG uploaded through the native file dialog, Win32 pattern of
#           drive-m4.ps1) and a colour cover; a Calendar view (Due / Until: pages on their days, a three-day span, "Change
#           date...", the "No date" tray), a Gallery view (first-image preview from the core's index, a cover, icons, card size
#           L), the side peek (open, rename, Esc, Center peek, Open as page) and a filter group
#           "Status is X OR (Due before <date> AND title contains Y)".
#   verify: after a stop and a start on the same data directory: the views, the active tab and the filter group survive.
# `-Step all` = launch, build, stop, launch (-Keep), verify, stop. The data directory is out\dbv-data (KB_DATA_DIR); a fresh one
# unless -Keep. Output: out\dbv-*.png / dbv-report*.json / dbv*.log. Never SendKeys (UI Automation, Win32 messages, CDP).
param([string]$Step = "all", [int]$Port = 9333, [string]$Result = "", [string]$Arg = "", [switch]$Keep)
$ErrorActionPreference = "Continue"
$dbvStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
$dataDir = Join-Path $out "dbv-data"
New-Item -ItemType Directory -Force $out | Out-Null

function Write-Result($obj) {
  $json = ConvertTo-Json -InputObject $obj -Depth 10 -Compress
  [System.IO.File]::WriteAllText($Result, $json, (New-Object System.Text.UTF8Encoding($false)))
}

# ---- helper steps, called by drive-database-views.mjs --------------------------------------------------------------------
if ($dbvStep -in "capture-main", "file-dialog") {
  $p = Get-Content -Raw -Encoding UTF8 -Path $Arg | ConvertFrom-Json
  if (-not ("DbvWin" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class DbvWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr dlg, int id);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public static string Cls(IntPtr h) { var c = new StringBuilder(256); GetClassName(h, c, 256); return c.ToString(); }
  public static string Txt(IntPtr h) { var t = new StringBuilder(512); GetWindowText(h, t, 512); return t.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  // The first visible common dialog (#32770) of the process, whatever its title ("Open", "Select a file", ...).
  public static IntPtr Dialog(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { if (Pid(h) == pid && IsWindowVisible(h) && Cls(h) == "#32770") { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static IntPtr FileNameEdit(IntPtr dlg) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(dlg, (h, l) => {
      int id = GetDlgCtrlID(h);
      if (Cls(h) == "Edit" && Cls(GetParent(h)) == "ComboBox" && IsWindowVisible(h) && (id == 1001 || id == 1148)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@
  }
  $app = Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
  if ($dbvStep -eq "capture-main") {
    . "$PSScriptRoot\capture.ps1"
    $msg = Capture-Window $app.MainWindowHandle $p.path
    Write-Result @{ ok = (Test-Path $p.path); path = $p.path; message = "$msg" }
    exit 0
  }
  # file-dialog: waits for the native Open dialog, types $p.path into its file-name box (WM_SETTEXT), presses IDOK (BM_CLICK).
  $res = [ordered]@{ ok = $false; title = ""; waitedMs = 0; closed = $false }
  $started = Get-Date
  $dlg = [IntPtr]::Zero
  $limitMs = if ($p.timeoutMs) { [int]$p.timeoutMs } else { 20000 }
  while ($dlg -eq [IntPtr]::Zero -and ((Get-Date) - $started).TotalMilliseconds -lt $limitMs) {
    $dlg = [DbvWin]::Dialog([uint32]$app.Id)
    if ($dlg -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 250 }
  }
  $res.waitedMs = [int]((Get-Date) - $started).TotalMilliseconds
  if ($dlg -eq [IntPtr]::Zero) { $res.error = "no file dialog appeared"; Write-Result $res; exit 1 }
  $res.title = [DbvWin]::Txt($dlg)
  Start-Sleep -Milliseconds 900
  $edit = [DbvWin]::FileNameEdit($dlg)
  if ($edit -eq [IntPtr]::Zero) { $res.error = "no file-name edit in the dialog"; Write-Result $res; exit 1 }
  [DbvWin]::SetForegroundWindow($dlg) | Out-Null
  [DbvWin]::SendMessage($edit, 0x000C, [IntPtr]::Zero, [string]$p.path) | Out-Null
  Start-Sleep -Milliseconds 500
  $ok = [DbvWin]::GetDlgItem($dlg, 1)
  [DbvWin]::SendMessage($ok, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
  for ($i = 0; $i -lt 30; $i++) { if ([DbvWin]::Dialog([uint32]$app.Id) -eq [IntPtr]::Zero) { $res.closed = $true; break }; Start-Sleep -Milliseconds 250 }
  $res.ok = $res.closed
  Write-Result $res
  exit $(if ($res.ok) { 0 } else { 1 })
}

# ---- launch / build / verify / stop ---------------------------------------------------------------------------------------
. (Join-Path $repo "scripts\dev-env.ps1")

function Launch-App([switch]$KeepData) {
  & "$PSScriptRoot\stop.ps1" | Out-Null
  if ((Test-Path $dataDir) -and -not $KeepData) { Remove-Item -Recurse -Force $dataDir }
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  $err = "$out\dbv.err.log"; $stdout = "$out\dbv.out.log"
  $env:KB_DATA_DIR = $dataDir
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
  $proc = Start-Process -FilePath "cmd.exe" -ArgumentList "/c", "pnpm", "tauri", "dev" -WorkingDirectory (Join-Path $repo "apps\desktop") `
    -PassThru -WindowStyle Hidden -RedirectStandardError $err -RedirectStandardOutput $stdout
  Remove-Item Env:KB_DATA_DIR, Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
  "launcher pid $($proc.Id)"
  $deadline = (Get-Date).AddSeconds(900)
  $app = $null
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 3
    $app = Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if ($app) { break }
  }
  if (-not $app) { "no app window within 900 s"; Get-Content $err -Tail 30; exit 1 }
  "app window up: pid=$($app.Id)"
  Start-Sleep -Seconds 5
}

function Run-Mjs([string]$phase) {
  $png = Join-Path $out "dbv-image.png"
  & node "$PSScriptRoot\drive-database-views.mjs" $phase "$out\dbv-report-$phase.json" $png 2>&1 | ForEach-Object { "$_" } | Out-Host
  return [int]$LASTEXITCODE
}

$code = 0
if ($dbvStep -eq "all") {
  Launch-App
  $code = Run-Mjs "build"
  & "$PSScriptRoot\stop.ps1"
  Launch-App -KeepData
  $c2 = Run-Mjs "verify"
  if ($c2) { $code = $c2 }
  & "$PSScriptRoot\stop.ps1"
} else {
  if ($dbvStep -eq "launch") { Launch-App -KeepData:$Keep }
  if ($dbvStep -in "build", "verify") { $code = Run-Mjs $dbvStep }
  if ($dbvStep -eq "stop") { & "$PSScriptRoot\stop.ps1" }
}
if ($code) { exit $code }
