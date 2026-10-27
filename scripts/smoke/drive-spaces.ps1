# Spaces pass (ADR-0038 moving pages between spaces, ADR-0039 importing an AppFlowy workspace) on the BUILT desktop app, the
# real WebView2 under the production CSP, the real core and shell, the native dialogs:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   .\drive-spaces.ps1 -Step launch          fresh data directory out\spaces-data, onboarding by UI Automation, the marker of the document is set
#   .\drive-spaces.ps1 -Step seed            first space: "Garden" (child "Tomatoes", a tag, an image through the native Open dialog),
#                                            "Plans" with a chip to Garden; a second space "Work"; Garden is a favourite
#   .\drive-spaces.ps1 -Step move            Garden's row menu -> "Move to..." -> Work -> Top level -> Next -> "Move 2 pages to "Work"" -> Move;
#                                            trees, image, link, search, tabs, favourite, tag, backlinks
#   .\drive-spaces.ps1 -Step move-back       Garden's page menu -> "Move to..." -> the first space -> under "Plans" -> Move; the same checks
#   .\drive-spaces.ps1 -Step appflowy-no     a SYNTHETIC AppFlowy backup made at run time: space menu -> "Import AppFlowy workspace..." ->
#                                            "Choose backup .zip..." -> Open dialog -> the shell's "Download images?" -> No: 1 new space, the
#                                            image stays a link, nothing was fetched
#   .\drive-spaces.ps1 -Step appflowy-real   a real AppFlowy backup (-RealZip <path>, required for this step, read-only): the
#                                            same flow, Yes to "Download images?" (its owner agreed to the downloads): counts only
#   .\drive-spaces.ps1 -Step appflowy-big    a second, larger backup (-BigZip <path>, required for this step, read-only): No to
#                                            "Download images?"; counts only
#   .\drive-spaces.ps1 -Step move-imported   the first root page (by position) of the first new space of appflowy-real moved to the first space
#   .\drive-spaces.ps1 -Step move-board      the board page of the real import (38 rows, a select property, its saved view) moved to the first space: the property
#                                            is created there, the rows keep their values, the board shows the same columns and cards
#   .\drive-spaces.ps1 -Step move-files      the first page of the real import with file blocks moved to the first space: the attachments follow, names unchanged
#   .\drive-spaces.ps1 -Step restart         the binary is started again on the same data: every space's tree is as before
#   .\drive-spaces.ps1 -Step logs            the log of the shell and the core: no panic, no ERROR, no page text, no file names of the archives
#   .\drive-spaces.ps1 -Step verify          the summary of every phase (out\spaces-report.json)
#   .\drive-spaces.ps1 -Step all             every phase above, then stops the binary
#
# Like drive-ai.ps1 the app is started with --remote-debugging-port, and drive-spaces.mjs sends trusted mouse and key events to
# the page over the WebView2 DevTools port, reads the DOM and the core through the page's own IPC, and records every request,
# console message and content-security-policy violation: a phase fails on a request to another host (the page never downloads
# anything: the shell does), a violation, a failed request, a console error, or when the document of the webview was replaced.
# The native dialogs (the Open dialog and the shell's own Yes/No message box) are driven with Win32 by this script's helper steps,
# which the .mjs calls (never SendKeys). Output lands in out\ (git-ignored): spaces-*.png, spaces-*.json, spaces.err.log (and
# spaces-r1.err.log of the restart); the data directory is out\spaces-data (never the person's own: KB_DATA_DIR). The two real
# archives are only read: nothing of them is copied, and the reports hold counts and codes only (no titles, no names). A failing
# phase does not stop the others; the exit code is 1 when any failed. Only this binary is stopped (never node).
param([string]$Step = "all", [int]$Port = 9333, [string]$Result = "", [string]$Arg = "",
      [string]$RealZip = "", [string]$BigZip = "", [switch]$DownloadsBlocked)
$ErrorActionPreference = "Continue"
$spStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$failedPhases = @()
# The real archives are personal: their paths are never written here, they are given on the command line (-RealZip, -BigZip);
# the steps that need one fail with a clear message without it.

function Write-Result($obj) {
  $json = ConvertTo-Json -InputObject $obj -Depth 10 -Compress
  [System.IO.File]::WriteAllText($Result, $json, (New-Object System.Text.UTF8Encoding($false)))
}

# ---- helper steps, called by drive-spaces.mjs --------------------------------------------------------------------------------
# Every one reads its parameters from the JSON file named by -Arg and writes one JSON document to -Result.
$helpers = @("dialog", "message-dialog", "windows", "uia", "capture-hwnd", "zip-text")
if ($helpers -contains $spStep) {
  $p = Get-Content -Raw -Encoding UTF8 -Path $Arg | ConvertFrom-Json
  Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem, UIAutomationClient, UIAutomationTypes
  if (-not ("SpacesWin" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class SpacesWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr dlg, int id);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, StringBuilder l);
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
  // The file-name box of a common dialog: the visible Edit inside a ComboBox, control id 1001 (Save) or 1148 (Open); in the
  // Select Folder dialog it is the visible Edit with id 1152 (its parent is not a ComboBox).
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
  // A button of a message box or task dialog by its text ("Yes", "No"): BM_CLICK.
  public static bool PressText(IntPtr dlg, string text) {
    bool done = false;
    EnumChildWindows(dlg, (h, l) => {
      if (Cls(h) == "Button" && Txt(h).Replace("&", "") == text) { SendMessage(h, 0x00F5, IntPtr.Zero, IntPtr.Zero); done = true; return false; }
      return true;
    }, IntPtr.Zero);
    return done;
  }
}
"@
  }

  function Get-AppProcess() {
    return Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  }
  # The app's process and every process below it (the WebView2 browser process owns some windows).
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
  function Describe-Window([IntPtr]$h) {
    $r = New-Object SpacesWin+RECT
    [SpacesWin]::GetWindowRect($h, [ref]$r) | Out-Null
    return [pscustomobject]@{
      hwnd = [int64]$h; pid = [int][SpacesWin]::Pid($h); class = [SpacesWin]::Cls($h); title = [SpacesWin]::Txt($h)
      visible = [SpacesWin]::IsWindowVisible($h); owner = [int64][SpacesWin]::GetWindow($h, 4)
      left = $r.Left; top = $r.Top; width = $r.Right - $r.Left; height = $r.Bottom - $r.Top
    }
  }
  # Every text and button name that UI Automation shows under a window.
  function Read-Uia([IntPtr]$h, [int]$limit = 200) {
    $texts = @(); $buttons = @()
    try {
      $el = [System.Windows.Automation.AutomationElement]::FromHandle($h)
      $all = $el.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
      foreach ($e in $all) {
        $c = $e.Current
        $type = $c.ControlType.ProgrammaticName -replace "ControlType.", ""
        if ($type -eq "Button" -and $c.Name -ne "") { $buttons += $c.Name }
        elseif ($type -eq "Text" -and $c.Name -ne "") { $texts += $c.Name }
        if (($texts.Count + $buttons.Count) -ge $limit) { break }
      }
    } catch { }
    return @{ texts = $texts; buttons = $buttons }
  }

  switch ($spStep) {
    "dialog" {
      # $p.title: the dialog's title ("Open"); $p.steps: the texts to put in the file name box, each followed by IDOK (the last
      # one closes the dialog); $p.cancel: IDCANCEL instead; $p.timeoutMs: how long to wait for it (default 30 s).
      $app = Get-AppProcess
      if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $find = {
        foreach ($h in [SpacesWin]::TopLevel($pids)) {
          if ([SpacesWin]::IsWindowVisible($h) -and [SpacesWin]::Cls($h) -eq "#32770" -and [SpacesWin]::Txt($h) -eq $p.title) { return $h }
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
      $res = [ordered]@{ ok = $false; title = $p.title; waitedMs = [int]((Get-Date) - $started).TotalMilliseconds; edit = $null; initial = $null; seen = @(); controls = @(); closed = $false; confirmed = $false; cancelled = $false }
      if ($dlg -eq [IntPtr]::Zero) {
        $res.error = "no '$($p.title)' dialog appeared"
        $res.seen = @(foreach ($h in [SpacesWin]::TopLevel($pids)) { if ([SpacesWin]::IsWindowVisible($h)) { "$([SpacesWin]::Cls($h)) '$([SpacesWin]::Txt($h))'" } })
        Write-Result $res; exit 1
      }
      Start-Sleep -Milliseconds 800
      if ($p.cancel) {
        [SpacesWin]::SetForegroundWindow($dlg) | Out-Null
        $res.cancelled = [SpacesWin]::PressId($dlg, 2)
        for ($i = 0; $i -lt 40; $i++) {
          if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }
          Start-Sleep -Milliseconds 250
        }
        $res.ok = $res.closed
        if (-not $res.closed) { $res.error = "the '$($p.title)' dialog did not close when cancelled" }
        Write-Result $res
        exit $(if ($res.ok) { 0 } else { 1 })
      }
      $edit = [SpacesWin]::FileNameEdit($dlg)
      $res.controls = @(foreach ($c in [SpacesWin]::Children($dlg)) { if ([SpacesWin]::IsWindowVisible($c)) { "$([SpacesWin]::Cls($c)) id=$([SpacesWin]::GetDlgCtrlID($c))" } })
      if ($edit -eq [IntPtr]::Zero) { $res.error = "no file-name edit in the '$($p.title)' dialog"; Write-Result $res; exit 1 }
      $res.edit = [SpacesWin]::GetDlgCtrlID($edit)
      $res.initial = [SpacesWin]::GetText($edit)
      [SpacesWin]::SetForegroundWindow($dlg) | Out-Null
      $steps = @($p.steps)
      for ($s = 0; $s -lt $steps.Count; $s++) {
        [SpacesWin]::SetText($edit, [string]$steps[$s])
        Start-Sleep -Milliseconds 500
        [SpacesWin]::PressOk($dlg)
        Start-Sleep -Milliseconds 900
        foreach ($h in [SpacesWin]::TopLevel($pids)) {
          if ([SpacesWin]::IsWindowVisible($h) -and [SpacesWin]::Cls($h) -eq "#32770" -and [SpacesWin]::Txt($h) -like "Confirm*") {
            $res.confirmed = [SpacesWin]::PressId($h, 6)
            Start-Sleep -Milliseconds 600
          }
        }
      }
      for ($i = 0; $i -lt 40; $i++) {
        if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }
        Start-Sleep -Milliseconds 250
      }
      $res.ok = $res.closed
      if (-not $res.closed) { $res.error = "the '$($p.title)' dialog did not close (the path may have been refused)" }
      Write-Result $res
      exit $(if ($res.ok) { 0 } else { 1 })
    }
    "message-dialog" {
      # A confirmation of the shell (a native message box with custom buttons): $p.title (the window's title), $p.press (the
      # button's text: "Yes" or "No"), $p.capture (a PNG path, or empty), $p.timeoutMs. The texts and button names that UI
      # Automation and Win32 show are returned, so the caller can check the question that was asked.
      $app = Get-AppProcess
      if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $find = {
        foreach ($h in [SpacesWin]::TopLevel($pids)) {
          if ([SpacesWin]::IsWindowVisible($h) -and [SpacesWin]::Txt($h) -eq $p.title) { return $h }
        }
        return [IntPtr]::Zero
      }
      $dlg = [IntPtr]::Zero
      $started = Get-Date
      $limitMs = if ($p.timeoutMs) { [int]$p.timeoutMs } else { 30000 }
      while ($dlg -eq [IntPtr]::Zero -and ((Get-Date) - $started).TotalMilliseconds -lt $limitMs) {
        $dlg = & $find
        if ($dlg -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 200 }
      }
      $res = [ordered]@{ ok = $false; title = $p.title; waitedMs = [int]((Get-Date) - $started).TotalMilliseconds; class = $null; texts = @(); buttons = @(); children = @(); pressed = $null; method = $null; closed = $false; captured = $null }
      if ($dlg -eq [IntPtr]::Zero) {
        $res.error = "no '$($p.title)' dialog appeared"
        $res.seen = @(foreach ($h in [SpacesWin]::TopLevel($pids)) { if ([SpacesWin]::IsWindowVisible($h)) { "$([SpacesWin]::Cls($h)) '$([SpacesWin]::Txt($h))'" } })
        Write-Result $res; exit 1
      }
      Start-Sleep -Milliseconds 700
      $res.class = [SpacesWin]::Cls($dlg)
      $res.children = @(foreach ($c in [SpacesWin]::Children($dlg)) { if ([SpacesWin]::IsWindowVisible($c)) { "$([SpacesWin]::Cls($c)) '$([SpacesWin]::Txt($c))'" } })
      $uia = Read-Uia $dlg
      $res.texts = @($uia.texts)
      $res.buttons = @($uia.buttons)
      if ($p.capture) {
        . "$PSScriptRoot\capture.ps1"
        Capture-Window $dlg $p.capture | Out-Null
        $res.captured = if (Test-Path $p.capture) { $p.capture } else { $null }
      }
      if ($p.noPress) {
        # Only look at it (the caller presses later).
        $res.ok = $true
        Write-Result $res
        exit 0
      }
      [SpacesWin]::SetForegroundWindow($dlg) | Out-Null
      # First the UI Automation Invoke of the button, then BM_CLICK on a Button window of that text.
      $pressed = $false
      try {
        $el = [System.Windows.Automation.AutomationElement]::FromHandle($dlg)
        $cond = New-Object System.Windows.Automation.AndCondition(
          (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)),
          (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, [string]$p.press)))
        $button = $el.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
        if ($button) {
          $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
          $pressed = $true; $res.method = "uia-invoke"
        }
      } catch { $res.uiaError = "$($_.Exception.Message)" }
      if (-not $pressed) {
        $pressed = [SpacesWin]::PressText($dlg, [string]$p.press)
        if ($pressed) { $res.method = "bm-click" }
      }
      $res.pressed = $pressed
      for ($i = 0; $i -lt 60; $i++) {
        if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }
        Start-Sleep -Milliseconds 250
      }
      $res.ok = ($pressed -and $res.closed)
      if (-not $pressed) { $res.error = "no '$($p.press)' button in the '$($p.title)' dialog" }
      elseif (-not $res.closed) { $res.error = "the '$($p.title)' dialog did not close after '$($p.press)'" }
      Write-Result $res
      exit $(if ($res.ok) { 0 } else { 1 })
    }
    "windows" {
      $app = Get-AppProcess
      if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $top = @(foreach ($h in [SpacesWin]::TopLevel($pids)) { Describe-Window $h })
      Write-Result @{ ok = $true; main = [int64]$app.MainWindowHandle; processes = $tree.names; top = $top }
      exit 0
    }
    "uia" {
      $app = Get-AppProcess
      $h = if ($p.hwnd) { [IntPtr][int64]$p.hwnd } elseif ($app) { $app.MainWindowHandle } else { [IntPtr]::Zero }
      if ($h -eq [IntPtr]::Zero) { Write-Result @{ ok = $false; error = "no window" }; exit 1 }
      $el = [System.Windows.Automation.AutomationElement]::FromHandle($h)
      $found = @()
      $walker = $el.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
      $limit = if ($p.limit) { [int]$p.limit } else { 600 }
      foreach ($e in $walker) {
        $c = $e.Current
        if ($c.Name -ne "" -or $c.ControlType.ProgrammaticName -match "Button|Edit|ComboBox|Window|Dialog|Pane") {
          $found += [pscustomobject]@{ type = ($c.ControlType.ProgrammaticName -replace "ControlType.", ""); name = $c.Name; cls = $c.ClassName; id = $c.AutomationId }
          if ($found.Count -ge $limit) { break }
        }
      }
      Write-Result @{ ok = $true; name = $el.Current.Name; cls = $el.Current.ClassName; count = $found.Count; elements = $found }
      exit 0
    }
    "capture-hwnd" {
      . "$PSScriptRoot\capture.ps1"
      $line = Capture-Window ([IntPtr][int64]$p.hwnd) $p.path
      Write-Result @{ ok = (Test-Path $p.path); detail = "$line" }
      exit 0
    }
    "zip-text" {
      # The text of one entry of a .zip ($p.zip, $p.entry), read-only. Used only to learn which words must never reach the log
      # (the page names of a real archive): the caller keeps them in memory and deletes this result file at once.
      try {
        $zip = [System.IO.Compression.ZipFile]::OpenRead($p.zip)
        try {
          $entry = $zip.Entries | Where-Object { $_.FullName -eq $p.entry } | Select-Object -First 1
          if (-not $entry) { Write-Result @{ ok = $false; error = "no such entry" }; exit 1 }
          $reader = New-Object System.IO.StreamReader($entry.Open(), (New-Object System.Text.UTF8Encoding($false)), $true)
          try { $text = $reader.ReadToEnd() } finally { $reader.Dispose() }
        } finally { $zip.Dispose() }
        Write-Result @{ ok = $true; text = $text }
        exit 0
      } catch {
        Write-Result @{ ok = $false; error = "$($_.Exception.Message)" }
        exit 1
      }
    }
  }
  exit 0
}

$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$script:launches = 0

# Starts the built app on out\spaces-data with the DevTools port. -Keep keeps the data (a restart); the log of a restart goes to
# spaces-rN.err.log so that the first one is kept for the scan.
function Launch-Built([switch]$Keep) {
  . (Join-Path $repo "scripts\dev-env.ps1")
  # Only this binary is stopped (never node: a Vite dev server may be running for another session).
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $data = Join-Path $out "spaces-data"
  if ((Test-Path $data) -and -not $Keep) { Remove-Item -Recurse -Force $data }
  if (-not $Keep) {
    # A fresh pass: the reports, captures and state of an earlier one go (the logs are written again by this run).
    Get-ChildItem $out -Filter "spaces-*" -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne "spaces-data" -and $_.Name -ne "spaces-build.log" } | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    Get-ChildItem $out -Filter "spaces.*" -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    $script:launches = 0
  } else {
    $script:launches = @(Get-ChildItem $out -Filter "spaces*.err.log" -ErrorAction SilentlyContinue).Count
  }
  New-Item -ItemType Directory -Force $data | Out-Null
  $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
  if (-not (Test-Path $exe)) { "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop"; exit 1 }
  $log = if ($script:launches -eq 0) { "spaces" } else { "spaces-r$($script:launches)" }
  $script:launches += 1
  $env:KB_DATA_DIR = $data
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug,kb_import=debug"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
  $proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") `
    -RedirectStandardError "$out\$log.err.log" -RedirectStandardOutput "$out\$log.out.log"
  Remove-Item Env:KB_DATA_DIR
  Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 2
    $q = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if ($q -and $q.MainWindowHandle -ne 0) {
      "app window up: pid=$($q.Id) log=$log.err.log"
      $script:hwnd = $q.MainWindowHandle
      Start-Sleep -Seconds 5
      return
    }
  }
  "no app window"; exit 1
}

# The summary and the scan of the log read what the phases left: no app needed.
$env:SPACES_OUT_DIR = $out
$env:SPACES_PS1 = $PSCommandPath
$env:SPACES_REAL_ZIP = $RealZip
$env:SPACES_BIG_ZIP = $BigZip
# This machine refuses outbound connections to Rust-built executables (os error 10013): with -DownloadsBlocked the pass expects the 28
# image downloads of appflowy-real to fail and checks how the product reports that, instead of expecting them to succeed.
if ($DownloadsBlocked) { $env:SPACES_DOWNLOADS_BLOCKED = "1" }
if ($spStep -in "verify", "logs") {
  & node "$PSScriptRoot\drive-spaces.mjs" $spStep "$out\spaces-$(if ($spStep -eq 'verify') { 'report' } else { 'logs' }).json" 2>&1 | ForEach-Object { "$_" }
  exit $LASTEXITCODE
}
if ($spStep -eq "launch" -or $spStep -eq "all") { Launch-Built }

# Dot-sourcing the helpers runs their own `param`, which overwrites $Step: ours is in $spStep.
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$env:CDP_PORT = "$Port"
# The .mjs captures the window and drives the dialogs itself, at the moments it needs (it runs this script's helpers).
$env:SPACES_CAPTURE_PS1 = Join-Path $PSScriptRoot "capture.ps1"

function Stop-App() {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 3
}

function Run-Mjs([string]$phase, [string]$reportName = "") {
  if ($reportName -eq "") { $reportName = $phase }
  $report = "$out\spaces-$reportName.json"
  $lines = & node "$PSScriptRoot\drive-spaces.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $reportName
    if (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue) { Capture-Window $hwnd "$out\spaces-$reportName-failed.png" }
    "PHASE FAILED: $reportName (see $report)"
  }
}

if ($spStep -in "launch", "all") {
  # Onboarding: name the workspace and create it.
  $name = Find-El "Edit" "Workspace name" 20
  if ($name) {
    Set-El $name "Spaces workspace"
    Start-Sleep -Milliseconds 300
    Invoke-El (Find-El "Button" "Create workspace" 5)
    Start-Sleep -Seconds 3
  }
  # The recovery kit is not what this pass is about: "Do this later".
  $later = Find-El "" "Do this later" 12
  if ($later) { Invoke-El $later; Start-Sleep -Seconds 3 } else { "no 'Do this later' on the recovery-kit screen (continuing)" }
  Run-Mjs "launch"
  Capture-Window $hwnd "$out\spaces-launched.png"
}

foreach ($phase in "seed", "move", "move-back", "appflowy-no", "appflowy-real", "appflowy-big", "move-imported", "move-board", "move-files") {
  if ($spStep -in $phase, "all") {
    # A phase run alone is on the app that is running: the marker of its document is (re)set first.
    if ($spStep -eq $phase) { Run-Mjs "mark" }
    Run-Mjs $phase
  }
}

if ($spStep -in "restart", "all") {
  # The same data again: every space's tree as the earlier phases left it.
  Stop-App
  Launch-Built -Keep
  Run-Mjs "mark"
  Run-Mjs "restart"
}

if ($spStep -in "logs", "all") {
  & node "$PSScriptRoot\drive-spaces.mjs" "logs" "$out\spaces-logs.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "logs" }
}
if ($spStep -in "verify", "all") {
  & node "$PSScriptRoot\drive-spaces.mjs" "verify" "$out\spaces-report.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "verify" }
}

if ($spStep -eq "all") {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  "stopped the app (node and Vite were left alone)"
}
if ($failedPhases.Count -gt 0) { "FAILED: $($failedPhases -join ', ')"; exit 1 }
"spaces pass: all phases ok"
