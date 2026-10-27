# Small-gaps pass (ADR-0040: page path, page info, duplicate, trash expiry, toggle headings, column calculations, covers, page
# appearance, accent colour, search, deep links, tray and quick capture, native menu bar, French and Spanish) on the BUILT desktop
# app, the real WebView2 under the production CSP, the real core and shell, the native dialogs, menus, tray and hotkey:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR and KB_SINGLE_INSTANCE)
#   .\drive-s-gaps.ps1 -Step launch      fresh data directory out\s-gaps-data, onboarding by UI Automation, seed: pages (one nested three deep, one
#                                        very long), a table view with a Cost number column, a toggle, a second space
#   .\drive-s-gaps.ps1 -Step ui          page path, Page info, Duplicate (row and page menu), trash expiry, toggle headings and turn into, the
#                                        table footer's Sum, covers (gradient, Upload through the native Open dialog, reposition, remove), font,
#                                        small text, lock, accent, search (spread words, a typo, the chips), French, Spanish and back with the
#                                        words of the native menu bar and tray read with GetMenuString
#   .\drive-s-gaps.ps1 -Step menu        a lone Alt shows the native menu bar (GetMenu, the client height), Escape hides it, Alt+Left, Ctrl+Alt+N and
#                                        Alt+arrows do not show it, two items driven with WM_COMMAND (Toggle sidebar, About), one undo step for
#                                        one Ctrl+Z, Ctrl+C and Ctrl+V once each
#   .\drive-s-gaps.ps1 -Step deeplink    cold start with atomik-kb://page/<id>#<block>, an invalid link ignored, a second launch forwarded to the
#                                        running instance (KB_SINGLE_INSTANCE=1; the scheme is never registered in the registry)
#   .\drive-s-gaps.ps1 -Step tray        the tray icon (UI Automation, the shell's own click, its menu), the close button hides the window, the
#                                        quick capture shortcut (RegisterHotKey proves the app owns Ctrl+Alt+N, WM_HOTKEY to the hotkey window),
#                                        the locked app, File > Quit, close to the tray off and the close button exits
#   .\drive-s-gaps.ps1 -Step restart     the binary is started again on the same data: covers, appearance, accent, language, calculations stay
#   .\drive-s-gaps.ps1 -Step logs        the log of the shell and the core: no panic, no ERROR, no page text, no link, no id
#   .\drive-s-gaps.ps1 -Step verify      the summary of every phase (out\s-gaps-report.json)
#   .\drive-s-gaps.ps1 -Step all         every step above, then stops the binary
#
# Like drive-spaces.ps1 the app is started with --remote-debugging-port and drive-s-gaps.mjs sends trusted mouse and key events to
# the page over the WebView2 DevTools port, reads the DOM and the core through the page's own IPC, and records every request, console
# message and content-security-policy violation: a phase fails on a request to another host, a violation, a failed request, a console
# error, or when the document of the webview was replaced. The native side (the Open dialog, the menu bar, the tray, the hotkey, the
# window state, process exits) is driven with Win32 by this script's helper steps, which the .mjs calls (never SendKeys; the only
# injected keys are the quick capture shortcut, after RegisterHotKey has proved that the app owns it, and a Ctrl+Z / Ctrl+C / Ctrl+V
# check that is sent only while the app's window is the foreground window). Output lands in out\ (git-ignored): s-gaps-*.png,
# s-gaps-*.json, s-gaps*.err.log; the data directory is out\s-gaps-data (never the person's own: KB_DATA_DIR). The atomik-kb scheme is
# never registered (no KB_REGISTER_DEEP_LINK): a link is handed to the binary as its argument. Only this binary is stopped (never node).
param([string]$Step = "all", [int]$Port = 9333, [string]$Result = "", [string]$Arg = "")
$ErrorActionPreference = "Continue"
$sgStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
$dataDir = Join-Path $out "s-gaps-data"
$pidFile = Join-Path $out "s-gaps-pid.txt"
$exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
$failedPhases = @()
New-Item -ItemType Directory -Force $out | Out-Null

function Write-Result($obj) {
  $json = ConvertTo-Json -InputObject $obj -Depth 12 -Compress
  [System.IO.File]::WriteAllText($Result, $json, (New-Object System.Text.UTF8Encoding($false)))
}

# ---- starting the app ------------------------------------------------------------------------------------------------------------
# -Keep keeps the data directory (a restart); -AppArgs are the app's command line (a deep link); -SingleInstance sets KB_SINGLE_INSTANCE=1
# (debug builds with KB_DATA_DIR otherwise start beside a running app); -Second starts another process beside the running one, waits for
# it to exit (the single-instance plugin forwards its arguments to the first and the second exits) and reports how it ended.
function Start-App {
  param([switch]$Keep, [string[]]$AppArgs = @(), [switch]$SingleInstance, [switch]$Second)
  if (-not (Test-Path $exe)) { return @{ ok = $false; error = "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop" } }
  $logs = @(Get-ChildItem $out -Filter "s-gaps*.err.log" -ErrorAction SilentlyContinue | Where-Object { $_.Name -notmatch "second" })
  if (-not $Second) {
    # Only this binary is stopped (never node: a Vite dev server may be running for another session).
    Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    if ((Test-Path $dataDir) -and -not $Keep) { Remove-Item -Recurse -Force $dataDir }
    if (-not $Keep) {
      # A fresh pass: the reports, captures and state of an earlier one go (the logs are written again by this run).
      Get-ChildItem $out -Filter "s-gaps*" -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne "s-gaps-data" -and $_.Name -ne "s-gaps-build.log" } | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
      $logs = @()
    }
  } else {
    $others = @(Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue)
    if ($others.Count -ne 1) { return @{ ok = $false; error = "expected exactly one running atomik-kb-desktop to forward to, found $($others.Count): another app (the person's own?) would take the link" } }
  }
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  $log = if ($Second) { "s-gaps-second" } elseif ($logs.Count -eq 0) { "s-gaps" } else { "s-gaps-r$($logs.Count)" }
  if ($Second) {
    $n = @(Get-ChildItem $out -Filter "s-gaps-second*.err.log" -ErrorAction SilentlyContinue).Count
    if ($n -gt 0) { $log = "s-gaps-second$($n + 1)" }
  }
  $env:KB_DATA_DIR = $dataDir
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug,kb_import=debug"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
  if ($SingleInstance -or $Second) { $env:KB_SINGLE_INSTANCE = "1" }
  $started = Get-Date
  $startArgs = @{ FilePath = $exe; PassThru = $true; WorkingDirectory = (Join-Path $repo "apps\desktop\src-tauri"); RedirectStandardError = "$out\$log.err.log"; RedirectStandardOutput = "$out\$log.out.log" }
  if ($AppArgs.Count -gt 0) { $startArgs.ArgumentList = $AppArgs }
  $proc = Start-Process @startArgs
  Remove-Item Env:KB_DATA_DIR, Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
  Remove-Item Env:KB_SINGLE_INSTANCE -ErrorAction SilentlyContinue
  if ($Second) {
    # Windows PowerShell 5.1 reads ExitCode only if the handle was fetched while the process ran.
    $null = $proc.Handle
    $exited = $proc.WaitForExit(40000)
    $code = if ($exited) { $proc.ExitCode } else { $null }
    return @{ ok = $exited; exited = $exited; exitCode = $code; ms = [int]((Get-Date) - $started).TotalMilliseconds; log = "$log.err.log"; pid = $proc.Id }
  }
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 2
    $q = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if (-not $q) { return @{ ok = $false; error = "the app exited at start (see $log.err.log)"; log = "$log.err.log" } }
    if ($q.MainWindowHandle -ne 0) {
      [System.IO.File]::WriteAllText($pidFile, "$($q.Id)")
      Start-Sleep -Seconds 5
      return @{ ok = $true; pid = $q.Id; hwnd = [int64]$q.MainWindowHandle; log = "$log.err.log" }
    }
  }
  return @{ ok = $false; error = "no app window"; log = "$log.err.log" }
}

# ---- helper steps, called by drive-s-gaps.mjs ----------------------------------------------------------------------------------------
# Every one reads its parameters from the JSON file named by -Arg and writes one JSON document to -Result.
$helpers = @("dialog", "windows", "uia", "capture-main", "menu-read", "menu-command", "win-state", "win-close", "win-minimize", "tray-find", "tray-menu", "tray-click", "hotkey-probe", "hotkey-post", "hotkey-send", "keys-real", "proc-wait", "proc-count", "clip-get", "clip-set", "reg-scheme", "win-post")
if ($helpers -contains $sgStep) {
  $p = Get-Content -Raw -Encoding UTF8 -Path $Arg | ConvertFrom-Json
  Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem, UIAutomationClient, UIAutomationTypes
  if (-not ("SGWin" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class SGWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr dlg, int id);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr m, int pos);
  [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr m);
  [DllImport("user32.dll")] public static extern uint GetMenuItemID(IntPtr m, int pos);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetMenuString(IntPtr m, uint item, StringBuilder sb, int max, uint flags);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool RegisterHotKey(IntPtr h, int id, uint mods, uint vk);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool UnregisterHotKey(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  public static string Cls(IntPtr h) { var c = new StringBuilder(256); GetClassName(h, c, 256); return c.ToString(); }
  public static string Txt(IntPtr h) { var t = new StringBuilder(512); GetWindowText(h, t, 512); return t.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static List<IntPtr> TopLevel(uint[] pids) {
    var found = new List<IntPtr>();
    EnumWindows((h, l) => { if (Array.IndexOf(pids, Pid(h)) >= 0) { found.Add(h); } return true; }, IntPtr.Zero);
    return found;
  }
  public static List<IntPtr> Children(IntPtr parent) {
    var found = new List<IntPtr>();
    EnumChildWindows(parent, (h, l) => { found.Add(h); return true; }, IntPtr.Zero);
    return found;
  }
  // The app's main window: a top-level window of the process titled "atomik-kb" with no owner (hidden or not).
  public static IntPtr FindMain(uint pid) {
    IntPtr found = IntPtr.Zero;
    foreach (var h in TopLevel(new uint[] { pid })) {
      if (Txt(h) == "atomik-kb" && GetWindow(h, 4) == IntPtr.Zero) { found = h; break; }
    }
    return found;
  }
  // A top-level window of the process by class (the tray's "tray_icon_app", the hotkey's "global_hotkey_app", a menu "#32768").
  public static IntPtr FindClass(uint pid, string cls, bool visibleOnly) {
    foreach (var h in TopLevel(new uint[] { pid })) {
      if (Cls(h) == cls && (!visibleOnly || IsWindowVisible(h))) { return h; }
    }
    return IntPtr.Zero;
  }
  public static string MenuText(IntPtr m, int pos) { var sb = new StringBuilder(256); GetMenuString(m, (uint)pos, sb, 256, 0x400); return sb.ToString(); }
  public static string FileNameEdit_(IntPtr dlg) { return ""; }
  public static IntPtr FileNameEdit(IntPtr dlg) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(dlg, (h, l) => {
      int id = GetDlgCtrlID(h);
      if (Cls(h) == "Edit" && IsWindowVisible(h) && ((Cls(GetParent(h)) == "ComboBox" && (id == 1001 || id == 1148)) || id == 1152)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static void SetText(IntPtr edit, string text) { SendMessage(edit, 0x000C, IntPtr.Zero, text); }
  public static string GetText(IntPtr edit) { var sb = new StringBuilder(2048); SendMessage(edit, 0x000D, (IntPtr)2048, sb); return sb.ToString(); }
  public static void PressOk(IntPtr dlg) { IntPtr ok = GetDlgItem(dlg, 1); SendMessage(ok, 0x00F5, IntPtr.Zero, IntPtr.Zero); }
  public static bool PressId(IntPtr dlg, int id) { IntPtr b = GetDlgItem(dlg, id); if (b == IntPtr.Zero) { return false; } SendMessage(b, 0x00F5, IntPtr.Zero, IntPtr.Zero); return true; }
  // One key press with modifiers held (keybd_event): only used after the caller has proved where the keys will go.
  public static void Chord(byte[] mods, byte key) {
    foreach (var m in mods) { keybd_event(m, 0, 0, UIntPtr.Zero); }
    keybd_event(key, 0, 0, UIntPtr.Zero);
    keybd_event(key, 0, 2, UIntPtr.Zero);
    for (int i = mods.Length - 1; i >= 0; i--) { keybd_event(mods[i], 0, 2, UIntPtr.Zero); }
  }
}
"@
  }

  function Get-AppPid() {
    if ($p.pid) { return [int]$p.pid }
    if (Test-Path $pidFile) { return [int](Get-Content $pidFile) }
    return 0
  }
  # The app's process and every process below it (the WebView2 browser process owns some windows, the file chooser among them).
  function Get-Tree([int]$root) {
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name)
    $ids = @($root)
    $grew = $true
    while ($grew) {
      $grew = $false
      foreach ($q in $all) {
        if (($ids -contains [int]$q.ParentProcessId) -and ($ids -notcontains [int]$q.ProcessId)) { $ids += [int]$q.ProcessId; $grew = $true }
      }
    }
    return @{ ids = $ids; names = @($all | Where-Object { $ids -contains [int]$_.ProcessId } | ForEach-Object { "$($_.ProcessId):$($_.Name)" }) }
  }
  function Rect-Of([IntPtr]$h, [bool]$client) {
    $r = New-Object SGWin+RECT
    if ($client) { [SGWin]::GetClientRect($h, [ref]$r) | Out-Null } else { [SGWin]::GetWindowRect($h, [ref]$r) | Out-Null }
    return [pscustomobject]@{ left = $r.Left; top = $r.Top; width = $r.Right - $r.Left; height = $r.Bottom - $r.Top }
  }
  # The window that has the keyboard and the process it belongs to; "locked" when it is the lock screen (the session is locked: keys and the
  # clipboard do not reach the app's desktop, and injecting keys would go to the lock screen).
  function Foreground-Info() {
    $h = [SGWin]::GetForegroundWindow()
    $name = $null
    try { $name = (Get-Process -Id ([int][SGWin]::Pid($h)) -ErrorAction Stop).ProcessName } catch { }
    $title = [SGWin]::Txt($h)
    return @{ title = $title; process = $name; locked = (($name -in @("LockApp", "LogonUI")) -or ($title -like "*Lock Screen*")) }
  }
  function Describe-Window([IntPtr]$h) {
    $r = Rect-Of $h $false
    return [pscustomobject]@{
      hwnd = [int64]$h; pid = [int][SGWin]::Pid($h); class = [SGWin]::Cls($h); title = [SGWin]::Txt($h)
      visible = [SGWin]::IsWindowVisible($h); owner = [int64][SGWin]::GetWindow($h, 4)
      left = $r.left; top = $r.top; width = $r.width; height = $r.height
    }
  }
  # The items of a native menu: [{ text, id, sub }], separators as { text = "", id = 0 }; submenus are read one level down.
  function Read-Menu([IntPtr]$hm, [int]$depth) {
    $items = @()
    $count = [SGWin]::GetMenuItemCount($hm)
    for ($i = 0; $i -lt $count; $i++) {
      $text = [SGWin]::MenuText($hm, $i)
      $sub = [SGWin]::GetSubMenu($hm, $i)
      $id = [SGWin]::GetMenuItemID($hm, $i)
      $item = [ordered]@{ text = $text; id = $(if ($sub -ne [IntPtr]::Zero) { -1 } else { [int64]$id }); separator = ($text -eq "" -and $sub -eq [IntPtr]::Zero) }
      if ($sub -ne [IntPtr]::Zero -and $depth -gt 0) { $item.items = @(Read-Menu $sub ($depth - 1)) }
      $items += [pscustomobject]$item
    }
    return $items
  }

  $appPid = Get-AppPid
  switch ($sgStep) {
    "dialog" {
      # $p.title: the dialog's title ("Open"); $p.steps: the texts to put in the file name box, each followed by IDOK (the last one
      # closes the dialog); $p.cancel: IDCANCEL instead; $p.timeoutMs: how long to wait for it (default 30 s).
      $tree = Get-Tree $appPid
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $find = {
        foreach ($h in [SGWin]::TopLevel($pids)) {
          if ([SGWin]::IsWindowVisible($h) -and [SGWin]::Cls($h) -eq "#32770" -and [SGWin]::Txt($h) -eq $p.title) { return $h }
        }
        return [IntPtr]::Zero
      }
      $dlg = [IntPtr]::Zero
      $started = Get-Date
      $limitMs = if ($p.timeoutMs) { [int]$p.timeoutMs } else { 30000 }
      while ($dlg -eq [IntPtr]::Zero -and ((Get-Date) - $started).TotalMilliseconds -lt $limitMs) {
        $dlg = & $find
        if ($dlg -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 250 }
      }
      $res = [ordered]@{ ok = $false; title = $p.title; waitedMs = [int]((Get-Date) - $started).TotalMilliseconds; edit = $null; initial = $null; seen = @(); controls = @(); closed = $false; cancelled = $false }
      if ($dlg -eq [IntPtr]::Zero) {
        $res.error = "no '$($p.title)' dialog appeared"
        $res.seen = @(foreach ($h in [SGWin]::TopLevel($pids)) { if ([SGWin]::IsWindowVisible($h)) { "$([SGWin]::Cls($h)) '$([SGWin]::Txt($h))'" } })
        Write-Result $res; exit 1
      }
      Start-Sleep -Milliseconds 800
      if ($p.cancel) {
        [SGWin]::SetForegroundWindow($dlg) | Out-Null
        $res.cancelled = [SGWin]::PressId($dlg, 2)
        for ($i = 0; $i -lt 40; $i++) { if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }; Start-Sleep -Milliseconds 250 }
        $res.ok = $res.closed
        Write-Result $res
        exit $(if ($res.ok) { 0 } else { 1 })
      }
      $edit = [SGWin]::FileNameEdit($dlg)
      $res.controls = @(foreach ($c in [SGWin]::Children($dlg)) { if ([SGWin]::IsWindowVisible($c)) { "$([SGWin]::Cls($c)) id=$([SGWin]::GetDlgCtrlID($c))" } })
      if ($edit -eq [IntPtr]::Zero) { $res.error = "no file-name edit in the '$($p.title)' dialog"; Write-Result $res; exit 1 }
      $res.edit = [SGWin]::GetDlgCtrlID($edit)
      $res.initial = [SGWin]::GetText($edit)
      [SGWin]::SetForegroundWindow($dlg) | Out-Null
      foreach ($text in @($p.steps)) {
        [SGWin]::SetText($edit, [string]$text)
        Start-Sleep -Milliseconds 500
        [SGWin]::PressOk($dlg)
        Start-Sleep -Milliseconds 900
      }
      for ($i = 0; $i -lt 40; $i++) { if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }; Start-Sleep -Milliseconds 250 }
      $res.ok = $res.closed
      if (-not $res.closed) { $res.error = "the '$($p.title)' dialog did not close (the path may have been refused)" }
      Write-Result $res
      exit $(if ($res.ok) { 0 } else { 1 })
    }
    "windows" {
      $tree = Get-Tree $appPid
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      Write-Result @{ ok = $true; processes = $tree.names; top = @(foreach ($h in [SGWin]::TopLevel($pids)) { Describe-Window $h }) }
      exit 0
    }
    "uia" {
      $main = [SGWin]::FindMain([uint32]$appPid)
      $h = if ($p.hwnd) { [IntPtr][int64]$p.hwnd } else { $main }
      if ($h -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no window" }; exit 1 }
      $el = [System.Windows.Automation.AutomationElement]::FromHandle($h)
      $found = @()
      foreach ($e in $el.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
        $c = $e.Current
        if ($c.Name -ne "" -or $c.ControlType.ProgrammaticName -match "Button|Edit|ComboBox|Window|Dialog|Pane") {
          $found += [pscustomobject]@{ type = ($c.ControlType.ProgrammaticName -replace "ControlType.", ""); name = $c.Name; cls = $c.ClassName; id = $c.AutomationId }
          if ($found.Count -ge 600) { break }
        }
      }
      Write-Result @{ ok = $true; name = $el.Current.Name; count = $found.Count; elements = $found }
      exit 0
    }
    "capture-main" {
      . "$PSScriptRoot\capture.ps1"
      $main = [SGWin]::FindMain([uint32]$appPid)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no main window" }; exit 1 }
      $line = Capture-Window $main $p.path
      Write-Result @{ ok = (Test-Path $p.path); detail = "$line" }
      exit 0
    }
    "win-state" {
      # The state of the main window; $p.want ("visible", "hidden", "restored" = visible and not minimised, "foreground") waits up to
      # $p.timeoutMs for it.
      $main = [SGWin]::FindMain([uint32]$appPid)
      $started = Get-Date
      $limit = if ($p.timeoutMs) { [int]$p.timeoutMs } else { 0 }
      while ($p.want -and $limit -gt 0 -and ((Get-Date) - $started).TotalMilliseconds -lt $limit) {
        $main = [SGWin]::FindMain([uint32]$appPid)
        if ($main -ne [IntPtr]::Zero) {
          $v = [SGWin]::IsWindowVisible($main); $i = [SGWin]::IsIconic($main); $f = ([SGWin]::GetForegroundWindow() -eq $main)
          if (($p.want -eq "visible" -and $v) -or ($p.want -eq "hidden" -and -not $v) -or ($p.want -eq "restored" -and $v -and -not $i) -or ($p.want -eq "foreground" -and $f)) { break }
        }
        Start-Sleep -Milliseconds 100
      }
      $alive = $null -ne (Get-Process -Id $appPid -ErrorAction SilentlyContinue)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $true; alive = $alive; main = 0 }; exit 0 }
      $tray = [SGWin]::FindClass([uint32]$appPid, "tray_icon_app", $false)
      $hot = [SGWin]::FindClass([uint32]$appPid, "global_hotkey_app", $false)
      Write-Result @{
        ok = $true; alive = $alive; main = [int64]$main; visible = [SGWin]::IsWindowVisible($main); iconic = [SGWin]::IsIconic($main)
        foreground = ([SGWin]::GetForegroundWindow() -eq $main); hasMenu = ([SGWin]::GetMenu($main) -ne [IntPtr]::Zero)
        client = (Rect-Of $main $true); window = (Rect-Of $main $false); trayWindow = [int64]$tray; hotkeyWindow = [int64]$hot
        foregroundTitle = [SGWin]::Txt([SGWin]::GetForegroundWindow()); sessionLocked = (Foreground-Info).locked
      }
      exit 0
    }
    "win-close" {
      $main = [SGWin]::FindMain([uint32]$appPid)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no main window" }; exit 1 }
      # $p.waitExitMs: the process is held first, so that its exit code can be read when it ends.
      $proc = if ($p.waitExitMs) { Get-Process -Id $appPid -ErrorAction SilentlyContinue } else { $null }
      # Windows PowerShell 5.1 reads ExitCode only if the handle was fetched while the process ran.
      if ($proc) { $null = $proc.Handle }
      # WM_CLOSE, or (how = "syscommand") WM_SYSCOMMAND SC_CLOSE, which is what the title bar's close button sends the window.
      if ($p.how -eq "syscommand") { [SGWin]::PostMessage($main, 0x0112, [IntPtr]0xF060, [IntPtr]::Zero) | Out-Null }
      else { [SGWin]::PostMessage($main, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }
      $res = [ordered]@{ ok = $true }
      if ($proc) { $exited = $proc.WaitForExit([int]$p.waitExitMs); $res.exited = $exited; $res.exitCode = $(if ($exited) { $proc.ExitCode } else { $null }) }
      Write-Result $res
      exit 0
    }
    "win-minimize" {
      $main = [SGWin]::FindMain([uint32]$appPid)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no main window" }; exit 1 }
      [SGWin]::ShowWindow($main, 6) | Out-Null
      Start-Sleep -Milliseconds 700
      Write-Result @{ ok = $true; iconic = [SGWin]::IsIconic($main); visible = [SGWin]::IsWindowVisible($main) }
      exit 0
    }
    "menu-read" {
      # The native menu bar of the main window: GetMenu is null while it is hidden. $p.want: "shown" or "hidden" waits up to $p.timeoutMs for it.
      $main = [SGWin]::FindMain([uint32]$appPid)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no main window" }; exit 1 }
      $started = Get-Date
      $limit = if ($p.timeoutMs) { [int]$p.timeoutMs } else { 0 }
      $hm = [SGWin]::GetMenu($main)
      while ($p.want -and $limit -gt 0 -and ((Get-Date) - $started).TotalMilliseconds -lt $limit) {
        $shown = ($hm -ne [IntPtr]::Zero)
        if (($p.want -eq "shown" -and $shown) -or ($p.want -eq "hidden" -and -not $shown)) { break }
        Start-Sleep -Milliseconds 150
        $hm = [SGWin]::GetMenu($main)
      }
      $res = [ordered]@{ ok = $true; hasMenu = ($hm -ne [IntPtr]::Zero); client = (Rect-Of $main $true); window = (Rect-Of $main $false); visible = [SGWin]::IsWindowVisible($main); waitedMs = [int]((Get-Date) - $started).TotalMilliseconds; menu = @() }
      if ($hm -ne [IntPtr]::Zero) { $res.menu = @(Read-Menu $hm 1) }
      Write-Result $res
      exit 0
    }
    "menu-command" {
      # WM_COMMAND with the item's id (read from the menu): what choosing it sends the window.
      $main = [SGWin]::FindMain([uint32]$appPid)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no main window" }; exit 1 }
      $proc = if ($p.waitExitMs) { Get-Process -Id $appPid -ErrorAction SilentlyContinue } else { $null }
      # Windows PowerShell 5.1 reads ExitCode only if the handle was fetched while the process ran.
      if ($proc) { $null = $proc.Handle }
      $posted = [SGWin]::PostMessage($main, 0x0111, [IntPtr][int64]$p.id, [IntPtr]::Zero)
      $res = [ordered]@{ ok = $posted }
      if ($proc) { $exited = $proc.WaitForExit([int]$p.waitExitMs); $res.exited = $exited; $res.exitCode = $(if ($exited) { $proc.ExitCode } else { $null }) }
      Write-Result $res
      exit 0
    }
    "tray-find" {
      # Is there a tray icon? (1) the tray library's hidden window in the app, (2) the shell's own record of the icon, (3) UI Automation over
      # the notification area and its overflow, looking for the tooltip "atomik-kb".
      $tray = [SGWin]::FindClass([uint32]$appPid, "tray_icon_app", $false)
      $res = [ordered]@{ ok = $true; trayWindow = [int64]$tray; uia = @(); roots = @(); registry = @() }
      try {
        $reg = "HKCU:\Control Panel\NotifyIconSettings"
        if (Test-Path $reg) {
          foreach ($k in Get-ChildItem $reg) {
            $v = Get-ItemProperty $k.PSPath
            if ($v.ExecutablePath -and $v.ExecutablePath -like "*atomik-kb-desktop*") {
              $res.registry += [pscustomobject]@{ path = $v.ExecutablePath; promoted = $v.IsPromoted; tooltip = $v.InitialTooltip }
            }
          }
        }
      } catch { $res.registryError = "$($_.Exception.Message)" }
      try {
        $root = [System.Windows.Automation.AutomationElement]::RootElement
        $candidates = @()
        foreach ($cls in @("Shell_TrayWnd", "NotifyIconOverflowWindow", "TopLevelWindowForOverflowXamlIsland", "Shell_SecondaryTrayWnd")) {
          $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ClassNameProperty, $cls)
          foreach ($w in $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)) { $candidates += $w; $res.roots += $cls }
        }
        foreach ($w in $candidates) {
          foreach ($e in $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
            $c = $e.Current
            # The taskbar's own buttons for running windows ("atomik-kb - 2 running windows") are not the tray icon.
            if ($c.Name -like "*atomik-kb*" -and $c.ClassName -notlike "*TaskList*" -and $c.ClassName -notlike "*TaskbarButton*") {
              $b = $c.BoundingRectangle
              $res.uia += [pscustomobject]@{ name = $c.Name; type = ($c.ControlType.ProgrammaticName -replace "ControlType.", ""); cls = $c.ClassName; offscreen = $c.IsOffscreen; x = [int]$b.X; y = [int]$b.Y; w = [int]$b.Width; h = [int]$b.Height; root = $w.Current.ClassName }
            }
          }
        }
      } catch { $res.uiaError = "$($_.Exception.Message)" }
      Write-Result $res
      exit 0
    }
    "tray-click" {
      # What the shell sends the tray library's window for a click of the icon: WM_USER + 1 (6002) with the mouse message in lParam.
      # The library asks the shell where the icon is (Shell_NotifyIconGetRect) before it raises its event, so a click that gets through
      # proves the icon is registered. $p.button: left (WM_LBUTTONUP) or right (WM_RBUTTONUP).
      $tray = [SGWin]::FindClass([uint32]$appPid, "tray_icon_app", $false)
      if ($tray -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no tray window" }; exit 1 }
      $msg = if ($p.button -eq "right") { 0x0205 } else { 0x0202 }
      Write-Result @{ ok = [SGWin]::PostMessage($tray, 6002, [IntPtr]::Zero, [IntPtr]$msg) }
      exit 0
    }
    "tray-menu" {
      # Opens the tray's menu as a right click does (the popup is modal in the app's thread), reads its items from the popup's HMENU
      # (MN_GETHMENU) and then: $p.press (an id) posts WM_COMMAND with it to the tray window, and the popup is closed (Escape, then
      # WM_CANCELMODE).
      $tray = [SGWin]::FindClass([uint32]$appPid, "tray_icon_app", $false)
      if ($tray -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no tray window" }; exit 1 }
      [SGWin]::PostMessage($tray, 6002, [IntPtr]::Zero, [IntPtr]0x0205) | Out-Null
      $popup = [IntPtr]::Zero
      for ($i = 0; $i -lt 40 -and $popup -eq [IntPtr]::Zero; $i++) {
        Start-Sleep -Milliseconds 150
        $popup = [SGWin]::FindClass([uint32]$appPid, "#32768", $true)
      }
      $res = [ordered]@{ ok = $false; popup = [int64]$popup; items = @(); pressed = $false; closed = $false }
      if ($popup -eq [IntPtr]::Zero) { $res.error = "no popup menu appeared"; Write-Result $res; exit 1 }
      $hm = [SGWin]::SendMessage($popup, 0x01E1, [IntPtr]::Zero, [IntPtr]::Zero)
      if ($hm -ne [IntPtr]::Zero) { $res.items = @(Read-Menu $hm 0) }
      if ($p.press) { $res.pressed = [SGWin]::PostMessage($tray, 0x0111, [IntPtr][int64]$p.press, [IntPtr]::Zero); Start-Sleep -Milliseconds 400 }
      for ($i = 0; $i -lt 20; $i++) {
        $now = [SGWin]::FindClass([uint32]$appPid, "#32768", $true)
        if ($now -eq [IntPtr]::Zero) { $res.closed = $true; break }
        if ($i -eq 0 -or $i -eq 8) { [SGWin]::PostMessage($now, 0x0100, [IntPtr]0x1B, [IntPtr]1) | Out-Null; [SGWin]::PostMessage($now, 0x0101, [IntPtr]0x1B, [IntPtr]1) | Out-Null }
        if ($i -eq 4 -or $i -eq 12) { [SGWin]::SendMessage($tray, 0x001F, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }
        Start-Sleep -Milliseconds 200
      }
      $res.ok = ($res.items.Count -gt 0)
      Write-Result $res
      exit 0
    }
    "hotkey-probe" {
      # Tries to register Ctrl+Alt+N (MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK 'N') for this script: it must fail with
      # ERROR_HOTKEY_ALREADY_REGISTERED (1409) while the app owns the combination. $p.mods and $p.vk say another one.
      $mods = if ($p.mods) { [uint32]$p.mods } else { [uint32](0x0002 -bor 0x0001 -bor 0x4000) }
      $vk = if ($p.vk) { [uint32]$p.vk } else { [uint32]0x4E }
      $ok = [SGWin]::RegisterHotKey([IntPtr]::Zero, 0x5A5A, $mods, $vk)
      $err = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
      if ($ok) { [SGWin]::UnregisterHotKey([IntPtr]::Zero, 0x5A5A) | Out-Null }
      Write-Result @{ ok = $true; registeredByUs = $ok; error = $(if ($ok) { 0 } else { $err }) }
      exit 0
    }
    "hotkey-post" {
      # WM_HOTKEY to the hidden window of the global-hotkey library, with the id the library gave the combination:
      # (modifiers << 16) | the key's code, here Ctrl (0x8) + Alt (0x1) and KeyN (32): 0x90020 (a documented rule of global-hotkey 0.8, checked
      # by the check that follows: the window comes back with a new page).
      $hot = [SGWin]::FindClass([uint32]$appPid, "global_hotkey_app", $false)
      if ($hot -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no global_hotkey_app window" }; exit 1 }
      $id = if ($p.id) { [int64]$p.id } else { 0x90020 }
      $lparam = [int64](((0x4E) -shl 16) -bor 0x0003)
      Write-Result @{ ok = [SGWin]::PostMessage($hot, 0x0312, [IntPtr]$id, [IntPtr]$lparam); window = [int64]$hot; id = $id }
      exit 0
    }
    "hotkey-send" {
      # The real key press, Ctrl+Alt+N, injected with keybd_event: only called after hotkey-probe proved that the app owns the combination
      # (so the keys do not reach another program), whichever window is in front.
      $front = Foreground-Info
      if ($front.locked) { Write-Result @{ ok = $true; sent = $false; reason = "the session is locked (the lock screen has the keyboard)"; foregroundTitle = $front.title }; exit 0 }
      [SGWin]::Chord([byte[]]@(0x11, 0x12), 0x4E)
      Write-Result @{ ok = $true; sent = $true }
      exit 0
    }
    "keys-real" {
      # A real keyboard chord (Ctrl+letter) sent with keybd_event, ONLY while the app's main window is the foreground window: $p.letter,
      # $p.count. The window is brought to the front first; when it cannot be, nothing is sent.
      $main = [SGWin]::FindMain([uint32]$appPid)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; sent = $false; error = "no main window" }; exit 1 }
      $front0 = Foreground-Info
      if ($front0.locked) { Write-Result @{ ok = $true; sent = $false; reason = "the session is locked (the lock screen has the keyboard)"; foregroundTitle = $front0.title }; exit 0 }
      for ($i = 0; $i -lt 10 -and [SGWin]::GetForegroundWindow() -ne $main; $i++) {
        [SGWin]::ShowWindow($main, 9) | Out-Null
        [SGWin]::SetForegroundWindow($main) | Out-Null
        Start-Sleep -Milliseconds 300
      }
      $front = ([SGWin]::GetForegroundWindow() -eq $main)
      if (-not $front) { Write-Result @{ ok = $true; sent = $false; reason = "the app's window cannot be brought to the front"; foregroundTitle = [SGWin]::Txt([SGWin]::GetForegroundWindow()) }; exit 0 }
      $vk = [byte][char]([string]$p.letter).ToUpper()
      for ($n = 0; $n -lt [int]$p.count; $n++) {
        if ([SGWin]::GetForegroundWindow() -ne $main) { Write-Result @{ ok = $true; sent = $true; interrupted = $n }; exit 0 }
        [SGWin]::Chord([byte[]]@(0x11), $vk)
        Start-Sleep -Milliseconds 400
      }
      Write-Result @{ ok = $true; sent = $true; count = [int]$p.count }
      exit 0
    }
    "proc-wait" {
      # Waits for the process to exit (up to $p.timeoutMs): whether it did and its exit code.
      $proc = Get-Process -Id $appPid -ErrorAction SilentlyContinue
      if ($proc) { $null = $proc.Handle }
      if (-not $proc) { Write-Result @{ ok = $true; exited = $true; exitCode = $null; note = "already gone" }; exit 0 }
      $limit = if ($p.timeoutMs) { [int]$p.timeoutMs } else { 20000 }
      $exited = $proc.WaitForExit($limit)
      $code = $null
      if ($exited) { try { $code = $proc.ExitCode } catch { } }
      Write-Result @{ ok = $true; exited = $exited; exitCode = $code }
      exit 0
    }
    "clip-get" {
      # The text on the clipboard (a pass that copies and pastes puts the person's text back afterwards).
      $text = $null
      try { $text = Get-Clipboard -Raw -ErrorAction Stop } catch { }
      Write-Result @{ ok = $true; text = $text }
      exit 0
    }
    "clip-set" {
      try { Set-Clipboard -Value ([string]$p.text); Write-Result @{ ok = $true } } catch { Write-Result @{ ok = $false; error = "$($_.Exception.Message)" } }
      exit 0
    }
    "win-post" {
      # Posts window messages to the main window: $p.msgs is a list of [message, wParam, lParam].
      $main = [SGWin]::FindMain([uint32]$appPid)
      if ($main -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no main window" }; exit 1 }
      $posted = 0
      foreach ($m in @($p.msgs)) {
        if ([SGWin]::PostMessage($main, [uint32]$m[0], [IntPtr][int64]$m[1], [IntPtr][int64]$m[2])) { $posted += 1 }
        Start-Sleep -Milliseconds 150
      }
      Write-Result @{ ok = ($posted -eq @($p.msgs).Count); posted = $posted }
      exit 0
    }
    "reg-scheme" {
      # Is the atomik-kb scheme registered for this user (HKCU\Software\Classes\atomik-kb)? The pass never registers it; an installed
      # app may have, so the pass compares this answer before and after.
      $key = "HKCU:\Software\Classes\atomik-kb"
      $exists = Test-Path $key
      $command = $null
      if ($exists) { try { $command = (Get-ItemProperty "$key\shell\open\command" -ErrorAction Stop).'(default)' } catch { } }
      Write-Result @{ ok = $true; exists = $exists; command = $command }
      exit 0
    }
    "proc-count" {
      $names = @(Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
      Write-Result @{ ok = $true; count = $names.Count; ids = $names }
      exit 0
    }
  }
  exit 0
}

# ---- the steps that read what the phases left (no app needed) ------------------------------------------------------------------------
$env:SG_OUT_DIR = $out
$env:SG_PS1 = $PSCommandPath
$env:CDP_PORT = "$Port"
if ($sgStep -in "verify", "logs") {
  & node "$PSScriptRoot\drive-s-gaps.mjs" $sgStep "$out\s-gaps-$(if ($sgStep -eq 'verify') { 'report' } else { 'logs' }).json" 2>&1 | ForEach-Object { "$_" }
  exit $LASTEXITCODE
}

# ---- the steps that drive the app -------------------------------------------------------------------------------------------------------

function Stop-App() {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 3
}

function Run-Mjs([string]$phase, [string]$reportName = "") {
  if ($reportName -eq "") { $reportName = $phase }
  $report = "$out\s-gaps-$reportName.json"
  $lines = & node "$PSScriptRoot\drive-s-gaps.mjs" $phase $report 2>&1
  $code = $LASTEXITCODE
  # Out-Host: a function's output is its return value, and the callers test that.
  $lines | ForEach-Object { "$_" } | Out-Host
  if ($code -ne 0) {
    $script:failedPhases += $reportName
    if (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue) {
      Capture-Main "$out\s-gaps-$reportName-failed.png"
    }
    "PHASE FAILED: $reportName (see $report)"
  }
}

# The running binary's window, captured with PrintWindow (what the failed phases leave behind).
function Capture-Main([string]$path) {
  . "$PSScriptRoot\capture.ps1"
  $running = Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if ($running) { Capture-Window $running.MainWindowHandle $path | Out-Null }
}

# Starts the binary on the data of the pass; the window is up and the marker of its document is set, or the step fails.
function Start-And-Mark([switch]$Keep, [string[]]$AppArgs = @(), [switch]$SingleInstance) {
  $started = Start-App -Keep:$Keep -AppArgs $AppArgs -SingleInstance:$SingleInstance
  if (-not $started.ok) { Write-Host "the app did not start: $($started.error)"; $script:failedPhases += "start"; return $false }
  Write-Host "app window up: pid=$($started.pid) log=$($started.log)"
  Run-Mjs "mark"
  return $true
}

function Ensure-Running() {
  if (-not (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue)) {
    "the app is not running: starting it on the data of the pass"
    Start-And-Mark -Keep | Out-Null
  } else {
    Run-Mjs "mark"
  }
}

if ($sgStep -in "launch", "all") {
  # The first start has no workspace: the page shows the onboarding, not the main layout, so the marker is set after it.
  $first = Start-App
  if (-not $first.ok) { "the app did not start: $($first.error)"; exit 1 }
  "app window up: pid=$($first.pid) log=$($first.log)"
  # Onboarding by UI Automation (the helpers of drive-m4.ps1 find the window of the running binary): name the workspace and create it.
  . "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
  $name = Find-El "Edit" "Workspace name" 20
  if ($name) {
    Set-El $name "Small gaps workspace"
    Start-Sleep -Milliseconds 300
    Invoke-El (Find-El "Button" "Create workspace" 5)
    Start-Sleep -Seconds 3
  } else { "no 'Workspace name' field (continuing)" }
  # The recovery kit is not what this pass is about: "Do this later".
  $later = Find-El "" "Do this later" 12
  if ($later) { Invoke-El $later; Start-Sleep -Seconds 3 } else { "no 'Do this later' on the recovery-kit screen (continuing)" }
  Run-Mjs "mark"
  Run-Mjs "launch"
  Capture-Main "$out\s-gaps-launched.png"
  Run-Mjs "seed"
}

if ($sgStep -in "ui", "all") {
  if ($sgStep -eq "ui") { Ensure-Running }
  Run-Mjs "ui"
}

if ($sgStep -in "menu", "all") {
  if ($sgStep -eq "menu") { Ensure-Running }
  Run-Mjs "menu"
}

if ($sgStep -in "deeplink", "all") {
  # (1) a cold start with a link, (2) a cold start with an invalid one, (3) a second launch with another link while the app runs. The
  # scheme is not registered: the link is the binary's argument, as the OS gives it when a registered link is opened. KB_SINGLE_INSTANCE=1
  # keeps "one running app" on beside KB_DATA_DIR (the plugin is not registered for a debug build with a data directory otherwise).

  $plan = Get-Content -Raw (Join-Path $out "s-gaps-state.json") | ConvertFrom-Json
  if ($plan.deepLinks) {
    if (Start-And-Mark -Keep -SingleInstance -AppArgs @($plan.deepLinks.cold)) { Run-Mjs "deeplink-cold" }
    if (Start-And-Mark -Keep -SingleInstance -AppArgs @($plan.deepLinks.invalid)) { Run-Mjs "deeplink-invalid" }
    # The running app (started with the invalid link) gets another launch: its window is minimised first, so that a focus is visible.
    Run-Mjs "deeplink-before"
    $second = Start-App -Second -AppArgs @($plan.deepLinks.forward)
    [System.IO.File]::WriteAllText((Join-Path $out "s-gaps-second.json"), (ConvertTo-Json -InputObject $second -Depth 5), (New-Object System.Text.UTF8Encoding($false)))
    "second launch: exited=$($second.exited) exitCode=$($second.exitCode) after $($second.ms) ms"
    Run-Mjs "deeplink-forward"
    # The same link again, to a second launch with NO link (a plain relaunch): only the window comes to the front.
    Run-Mjs "deeplink-before" "deeplink-before2"
    $third = Start-App -Second
    [System.IO.File]::WriteAllText((Join-Path $out "s-gaps-second.json"), (ConvertTo-Json -InputObject $third -Depth 5), (New-Object System.Text.UTF8Encoding($false)))
    "plain second launch: exited=$($third.exited) exitCode=$($third.exitCode) after $($third.ms) ms"
    Run-Mjs "deeplink-plain"
    # (4) a link that arrives while the app is locked: the lock screen first, the page after the passphrase.
    Run-Mjs "deeplink-lockprep"
    $fourth = Start-App -Second -AppArgs @($plan.deepLinks.cold)
    [System.IO.File]::WriteAllText((Join-Path $out "s-gaps-second.json"), (ConvertTo-Json -InputObject $fourth -Depth 5), (New-Object System.Text.UTF8Encoding($false)))
    "second launch while locked: exited=$($fourth.exited) exitCode=$($fourth.exitCode) after $($fourth.ms) ms"
    Run-Mjs "deeplink-locked"
  } else { "no deep-link plan in the state (the seed has not run)"; $failedPhases += "deeplink-plan" }
  Stop-App
}

if ($sgStep -in "tray", "all") {
  # The binary is started again without KB_SINGLE_INSTANCE (the pass of the tray needs nothing of it).
  if ($sgStep -eq "all" -or -not (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue)) {
    Start-And-Mark -Keep | Out-Null
  } else { Run-Mjs "mark" }
  Run-Mjs "tray"
  # File > Quit (WM_COMMAND) while close to the tray is on: the process must exit by itself, with code 0.
  Run-Mjs "tray-quit"
  # Close to the tray off, then the close button (WM_CLOSE) exits too.
  if (Start-And-Mark -Keep) { Run-Mjs "tray-closeoff" }
}

if ($sgStep -in "restart", "all") {
  if ($sgStep -eq "restart" -or $sgStep -eq "all") {
    # Before the restart: the language is French (a device setting that must survive), and the facts are saved.
    if (-not (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue)) { Start-And-Mark -Keep | Out-Null } else { Run-Mjs "mark" }
    Run-Mjs "prerestart"
    Stop-App
    if (Start-And-Mark -Keep) { Run-Mjs "restart" }
  }
}

if ($sgStep -in "logs", "all") {
  & node "$PSScriptRoot\drive-s-gaps.mjs" "logs" "$out\s-gaps-logs.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "logs" }
}
if ($sgStep -in "verify", "all") {
  & node "$PSScriptRoot\drive-s-gaps.mjs" "verify" "$out\s-gaps-report.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "verify" }
}

if ($sgStep -eq "all") {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  "stopped the app (node and Vite were left alone)"
}
if ($failedPhases.Count -gt 0) { "FAILED: $($failedPhases -join ', ')"; exit 1 }
"s-gaps pass: all phases ok"
