# Import / export pass (ADR-0036) on the BUILT desktop app, the real WebView2 under the production CSP:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   .\drive-interop.ps1 -Step launch         fresh data directory out\interop-data, onboarding and a recovery kit by UI Automation
#                                            (the backups need one), then the marker of the document is set
#   .\drive-interop.ps1 -Step seed           a page "Source": paragraph, heading, to-do list, an image (the native Open dialog), a table view
#                                            with two pages inside it that carry a select property
#   .\drive-interop.ps1 -Step fixtures       out\interop-src: a Notion-style Export-abc.zip, a Markdown folder notes\, loose\list.csv
#   .\drive-interop.ps1 -Step import-zip     space menu -> Import... -> Choose a .zip file... -> Open dialog; report, Open, the tree
#   .\drive-interop.ps1 -Step import-folder  page menu -> Import into this page... -> Choose a folder... -> Select Folder dialog
#   .\drive-interop.ps1 -Step import-files   space menu -> Import... -> Choose files... -> Open dialog (a multi-select: quoted names)
#   .\drive-interop.ps1 -Step export-zip     "Source": page menu -> Export... -> Markdown, Include sub-pages -> Save As; the zip is read back
#   .\drive-interop.ps1 -Step reimport       that zip imported again: same titles, values, table view and image
#   .\drive-interop.ps1 -Step export-html    a page without attachments as one .html file
#   .\drive-interop.ps1 -Step export-space   space menu -> Export space... -> Markdown: every page in one .zip
#   .\drive-interop.ps1 -Step csv            the imported Tasks database: view actions -> Export CSV -> Save As (BOM, CRLF, header, rows)
#   .\drive-interop.ps1 -Step print          Ctrl+P (a trusted key event): print mode, the print preview of the webview, Escape
#   .\drive-interop.ps1 -Step backup         Settings -> Scheduled backups: folder, on, Back up now (three runs, keep 2)
#   .\drive-interop.ps1 -Step guard          SettingsSet of app.backup.folder through the page's own IPC is refused
#   .\drive-interop.ps1 -Step cancel         every native dialog cancelled (Import's three sources, both Exports, the CSV, the backup folder): nothing is said
#   .\drive-interop.ps1 -Step import-hostile a zip with zip-slip and absolute names, a nested zip, scripts and remote pictures: nothing escapes, nothing runs
#   .\drive-interop.ps1 -Step verify         the summary of every phase (out\interop-report.json) and the scan of the log
#   .\drive-interop.ps1 -Step all            every phase above, then stops the binary
#
# Like drive-width.ps1 the app is started with --remote-debugging-port, and drive-interop.mjs sends trusted mouse and
# key events to the page over the WebView2 DevTools port (the calls of cdp.mjs, with a filter on the target's address: the
# print preview and the downloads flyout are page targets too), reads the DOM and the core through the page's own
# IPC, and records every request, console message and content-security-policy violation: a phase fails on a request
# to another host, a violation, a failed request, a console error, or when the document of the webview was replaced.
# The native file dialogs (#32770 windows titled "Save As", "Open" or "Select Folder" of the app's process) are driven
# with Win32 by this script's helper steps, which the .mjs calls (never SendKeys): `-Step dialog` sets the file name
# edit (control 1001 in the Save dialog, 1148 in the Open and Select Folder dialogs) with WM_SETTEXT and presses
# IDOK with BM_CLICK; `-Step zip-build` and `-Step zip-read` use System.IO.Compression; `-Step windows`, `-Step uia`,
# `-Step capture-hwnd` and `-Step postkey` look at the print preview of the webview. The onboarding and the recovery
# kit are driven with UI Automation (the helpers of drive-m4.ps1).
# Output lands in out\ (git-ignored): interop-*.png, interop-*.json, interop.err.log; the data directory is
# out\interop-data (never the person's own: KB_DATA_DIR). A failing phase does not stop the others (the report says
# which failed); the exit code is 1 when any failed. Only this binary is stopped (never node: a Vite dev server may
# be running for another session).
param([string]$Step = "all", [int]$Port = 9333, [string]$Result = "", [string]$Arg = "")
$ErrorActionPreference = "Continue"
$interopStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$failedPhases = @()

function Write-Result($obj) {
  $json = ConvertTo-Json -InputObject $obj -Depth 10 -Compress
  [System.IO.File]::WriteAllText($Result, $json, (New-Object System.Text.UTF8Encoding($false)))
}

# ---- helper steps, called by drive-interop.mjs -------------------------------------------------------------------
# Every one reads its parameters from the JSON file named by -Arg and writes one JSON document to -Result.
$helpers = @("dialog", "zip-build", "zip-read", "windows", "uia", "capture-hwnd", "postkey")
if ($helpers -contains $interopStep) {
  $p = Get-Content -Raw -Encoding UTF8 -Path $Arg | ConvertFrom-Json
  Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem, UIAutomationClient, UIAutomationTypes
  if (-not ("InteropWin" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class InteropWin {
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
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr dlg, int id);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
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
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, StringBuilder l);
  // WM_GETTEXT: what the edit says before it is overwritten (the name a Save As dialog proposes).
  public static string GetText(IntPtr edit) { var sb = new StringBuilder(2048); SendMessage(edit, 0x000D, (IntPtr)2048, sb); return sb.ToString(); }
  public static void PressOk(IntPtr dlg) { IntPtr ok = GetDlgItem(dlg, 1); SendMessage(ok, 0x00F5, IntPtr.Zero, IntPtr.Zero); }
  public static bool PressId(IntPtr dlg, int id) { IntPtr b = GetDlgItem(dlg, id); if (b == IntPtr.Zero) { return false; } SendMessage(b, 0x00F5, IntPtr.Zero, IntPtr.Zero); return true; }
}
"@
  }

  function Get-AppProcess() {
    return Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  }
  # The app's process and every process below it (the WebView2 browser process owns the print preview).
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
    $r = New-Object InteropWin+RECT
    [InteropWin]::GetWindowRect($h, [ref]$r) | Out-Null
    return [pscustomobject]@{
      hwnd = [int64]$h; pid = [int][InteropWin]::Pid($h); class = [InteropWin]::Cls($h); title = [InteropWin]::Txt($h)
      visible = [InteropWin]::IsWindowVisible($h); owner = [int64][InteropWin]::GetWindow($h, 4)
      left = $r.Left; top = $r.Top; width = $r.Right - $r.Left; height = $r.Bottom - $r.Top
    }
  }

  switch ($interopStep) {
    "dialog" {
      # $p.title: the dialog's title ("Save As", "Open" or "Select Folder"); $p.steps: the texts to put in the file name
      # box, each followed by IDOK (the last one closes the dialog; the earlier ones navigate in a multi-select Open).
      $app = Get-AppProcess
      if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $find = {
        foreach ($h in [InteropWin]::TopLevel($pids)) {
          if ([InteropWin]::IsWindowVisible($h) -and [InteropWin]::Cls($h) -eq "#32770" -and [InteropWin]::Txt($h) -eq $p.title) { return $h }
        }
        return [IntPtr]::Zero
      }
      $dlg = [IntPtr]::Zero
      $started = Get-Date
      for ($i = 0; $i -lt 120 -and $dlg -eq [IntPtr]::Zero; $i++) {
        $dlg = & $find
        if ($dlg -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 250 }
      }
      $res = [ordered]@{ ok = $false; title = $p.title; waitedMs = [int]((Get-Date) - $started).TotalMilliseconds; edit = $null; initial = $null; seen = @(); controls = @(); closed = $false; confirmed = $false; cancelled = $false }
      if ($dlg -eq [IntPtr]::Zero) {
        $res.error = "no '$($p.title)' dialog appeared"
        $res.seen = @(foreach ($h in [InteropWin]::TopLevel($pids)) { if ([InteropWin]::IsWindowVisible($h)) { "$([InteropWin]::Cls($h)) '$([InteropWin]::Txt($h))'" } })
        Write-Result $res; exit 1
      }
      Start-Sleep -Milliseconds 800
      if ($p.cancel) {
        # The person closes the dialog: IDCANCEL (the Cancel button).
        [InteropWin]::SetForegroundWindow($dlg) | Out-Null
        $res.cancelled = [InteropWin]::PressId($dlg, 2)
        for ($i = 0; $i -lt 40; $i++) {
          if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }
          Start-Sleep -Milliseconds 250
        }
        $res.ok = $res.closed
        if (-not $res.closed) { $res.error = "the '$($p.title)' dialog did not close when cancelled" }
        Write-Result $res
        exit $(if ($res.ok) { 0 } else { 1 })
      }
      $edit = [InteropWin]::FileNameEdit($dlg)
      $res.controls = @(foreach ($c in [InteropWin]::Children($dlg)) { if ([InteropWin]::IsWindowVisible($c)) { "$([InteropWin]::Cls($c)) id=$([InteropWin]::GetDlgCtrlID($c))" } })
      if ($edit -eq [IntPtr]::Zero) { $res.error = "no file-name edit in the '$($p.title)' dialog"; Write-Result $res; exit 1 }
      $res.edit = [InteropWin]::GetDlgCtrlID($edit)
      $res.initial = [InteropWin]::GetText($edit)
      [InteropWin]::SetForegroundWindow($dlg) | Out-Null
      $steps = @($p.steps)
      for ($s = 0; $s -lt $steps.Count; $s++) {
        [InteropWin]::SetText($edit, [string]$steps[$s])
        Start-Sleep -Milliseconds 500
        [InteropWin]::PressOk($dlg)
        Start-Sleep -Milliseconds 900
        # "Confirm Save As" (the file exists): Yes.
        foreach ($h in [InteropWin]::TopLevel($pids)) {
          if ([InteropWin]::IsWindowVisible($h) -and [InteropWin]::Cls($h) -eq "#32770" -and [InteropWin]::Txt($h) -like "Confirm*") {
            $res.confirmed = [InteropWin]::PressId($h, 6)
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
    "zip-build" {
      # $p.dir: the folder whose files go into $p.zip, paths relative to it with "/" (a Notion export has no top-level folder).
      if (Test-Path $p.zip) { Remove-Item -Force $p.zip }
      $names = @()
      if ($p.entries) {
        # Entries given by name and text (or base64 bytes): the names are used exactly as written, hostile ones included.
        $zip = [System.IO.Compression.ZipFile]::Open($p.zip, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
          foreach ($e in @($p.entries)) {
            $entry = $zip.CreateEntry([string]$e.name, [System.IO.Compression.CompressionLevel]::Optimal)
            $bytes = if ($e.base64) { [System.Convert]::FromBase64String([string]$e.base64) } else { (New-Object System.Text.UTF8Encoding($false)).GetBytes([string]$e.text) }
            $stream = $entry.Open()
            try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
            $names += [string]$e.name
          }
        } finally { $zip.Dispose() }
        Write-Result @{ ok = $true; entries = $names; bytes = (Get-Item $p.zip).Length }
        exit 0
      }
      $root = (Resolve-Path $p.dir).Path.TrimEnd("\")
      $zip = [System.IO.Compression.ZipFile]::Open($p.zip, [System.IO.Compression.ZipArchiveMode]::Create)
      try {
        foreach ($f in (Get-ChildItem -Recurse -File -Path $root | Sort-Object FullName)) {
          $name = $f.FullName.Substring($root.Length + 1).Replace("\", "/")
          $entry = $zip.CreateEntry($name, [System.IO.Compression.CompressionLevel]::Optimal)
          $in = [System.IO.File]::OpenRead($f.FullName)
          $outStream = $entry.Open()
          try { $in.CopyTo($outStream) } finally { $outStream.Dispose(); $in.Dispose() }
          $names += $name
        }
      } finally { $zip.Dispose() }
      Write-Result @{ ok = $true; entries = $names; bytes = (Get-Item $p.zip).Length }
      exit 0
    }
    "zip-read" {
      # Every entry (name, sizes) and the text of the text entries (UTF-8, at most 600 KB each); the first bytes of the others.
      $textExt = @(".md", ".json", ".html", ".htm", ".csv", ".excalidraw", ".txt")
      $entries = @(); $texts = [ordered]@{}; $heads = [ordered]@{}
      try {
        $zip = [System.IO.Compression.ZipFile]::OpenRead($p.zip)
        try {
          foreach ($e in $zip.Entries) {
            $entries += [pscustomobject]@{ name = $e.FullName; length = $e.Length; compressed = $e.CompressedLength }
            if ($e.FullName.EndsWith("/")) { continue }
            $ext = [System.IO.Path]::GetExtension($e.FullName).ToLowerInvariant()
            $stream = $e.Open()
            try {
              if (($textExt -contains $ext) -and $e.Length -le 600000) {
                $reader = New-Object System.IO.StreamReader($stream, (New-Object System.Text.UTF8Encoding($false)), $true)
                $texts[$e.FullName] = $reader.ReadToEnd()
              } else {
                $buf = New-Object byte[] 8
                $n = $stream.Read($buf, 0, 8)
                $heads[$e.FullName] = (($buf[0..([Math]::Max(0, $n - 1))] | ForEach-Object { $_.ToString("x2") }) -join "")
                $sink = New-Object byte[] 65536
                while ($stream.Read($sink, 0, $sink.Length) -gt 0) { }
              }
            } finally { $stream.Dispose() }
          }
        } finally { $zip.Dispose() }
        Write-Result @{ ok = $true; entries = $entries; texts = $texts; heads = $heads }
        exit 0
      } catch {
        Write-Result @{ ok = $false; error = "$($_.Exception.Message)" }
        exit 1
      }
    }
    "windows" {
      # Every top-level window of the app's process tree, and the children of the main window that are windows of their own.
      $app = Get-AppProcess
      if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $top = @(foreach ($h in [InteropWin]::TopLevel($pids)) { Describe-Window $h })
      $kids = @(foreach ($h in [InteropWin]::Children($app.MainWindowHandle)) { $d = Describe-Window $h; if ($d.visible -and $d.class -ne "") { $d } })
      Write-Result @{ ok = $true; main = [int64]$app.MainWindowHandle; processes = $tree.names; top = $top; children = $kids }
      exit 0
    }
    "uia" {
      # The UI Automation tree under a window ($p.hwnd): control type, name, class and id of every element with a name.
      $el = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr][int64]$p.hwnd)
      $found = @()
      $walker = $el.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
      $limit = if ($p.limit) { [int]$p.limit } else { 400 }
      foreach ($e in $walker) {
        $c = $e.Current
        if ($c.Name -ne "" -or $c.ControlType.ProgrammaticName -match "Button|Edit|ComboBox|Window|Dialog") {
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
    "postkey" {
      # $p.hwnd, $p.key (escape, enter, p) and $p.ctrl: WM_KEYDOWN and WM_KEYUP posted to the window.
      $vk = switch ($p.key) { "escape" { 0x1B } "enter" { 0x0D } "p" { 0x50 } default { 0x1B } }
      $h = [IntPtr][int64]$p.hwnd
      if ($p.ctrl) { [InteropWin]::PostMessage($h, 0x0100, [IntPtr]0x11, [IntPtr]0x001D0001) | Out-Null }
      [InteropWin]::PostMessage($h, 0x0100, [IntPtr]$vk, [IntPtr]0x00010001) | Out-Null
      [InteropWin]::PostMessage($h, 0x0101, [IntPtr]$vk, [IntPtr]([int]0xC0010001)) | Out-Null
      if ($p.ctrl) { [InteropWin]::PostMessage($h, 0x0101, [IntPtr]0x11, [IntPtr]([int]0xC01D0001)) | Out-Null }
      Write-Result @{ ok = $true }
      exit 0
    }
  }
  exit 0
}

$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null

function Launch-Built([switch]$Keep) {
  . (Join-Path $repo "scripts\dev-env.ps1")
  # Only this binary is stopped (never node: a Vite dev server may be running for another session).
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $data = Join-Path $out "interop-data"
  if ((Test-Path $data) -and -not $Keep) { Remove-Item -Recurse -Force $data }
  # A fresh pass: the reports, captures, exports, fixtures and backups of an earlier one go (the logs stay: this run writes them).
  if (-not $Keep) {
    Get-ChildItem $out -Filter "interop-*" -ErrorAction SilentlyContinue | Where-Object { $_.Extension -ne ".log" -and $_.Name -ne "interop-data" } | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
  }
  New-Item -ItemType Directory -Force $data | Out-Null
  $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
  if (-not (Test-Path $exe)) { "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop"; exit 1 }
  $env:KB_DATA_DIR = $data
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=info"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
  $proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") `
    -RedirectStandardError "$out\interop.err.log" -RedirectStandardOutput "$out\interop.out.log"
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
if ($interopStep -eq "verify") {
  $env:INTEROP_OUT_DIR = $out
  & node "$PSScriptRoot\drive-interop.mjs" "verify" "$out\interop-report.json" 2>&1 | ForEach-Object { "$_" }
  exit $LASTEXITCODE
}
if ($interopStep -eq "launch" -or $interopStep -eq "all") { Launch-Built }

# Dot-sourcing the helpers runs their own `param`, which overwrites $Step: ours is in $interopStep.
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$env:CDP_PORT = "$Port"
# The .mjs captures the window and drives the dialogs itself, at the moments it needs (it runs this script's helpers).
$env:INTEROP_CAPTURE_PS1 = Join-Path $PSScriptRoot "capture.ps1"
$env:INTEROP_PS1 = $PSCommandPath
$env:INTEROP_OUT_DIR = $out
$kitFile = Join-Path $out "interop-kit.txt"

function Run-Phase([string]$phase) {
  $report = "$out\interop-$phase.json"
  $lines = & node "$PSScriptRoot\drive-interop.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $phase
    if (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue) { Capture-Window $hwnd "$out\interop-$phase-failed.png" }
    "PHASE FAILED: $phase (see $report)"
  }
}

# The recovery kit (scheduled backups need one), the way drive-m4.ps1 -Step kit makes it; the words go to out\interop-kit.txt.
function Create-Kit() {
  $explain = Find-El "Button" "Create my recovery kit" 20
  if (-not $explain) { "no 'Create my recovery kit' button"; return $false }
  Invoke-El $explain
  Start-Sleep -Seconds 4
  $words = Read-KitWords
  "kit words read: $($words.Count)"
  if ($words.Count -ne 24) { "expected 24 words, got $($words.Count)"; return $false }
  Set-Content -Path $kitFile -Value ($words -join " ") -Encoding ascii
  Capture-Window $hwnd "$out\interop-kit.png"
  Invoke-El (Find-El "Button" "I saved my kit" 10)
  Start-Sleep -Seconds 1
  foreach ($e in (All-El (Web))) {
    $c = $e.Current
    if ($c.ControlType.ProgrammaticName -eq "ControlType.Edit" -and $c.Name -match '^Word (\d+)$') { Set-El $e $words[[int]$Matches[1] - 1] }
  }
  Start-Sleep -Milliseconds 500
  Invoke-El (Find-El "Button" "Check" 10)
  Start-Sleep -Seconds 3
  $done = Find-El "Button" "Done" 20
  if (-not $done) { "the word check did not pass"; return $false }
  Invoke-El $done
  Start-Sleep -Seconds 2
  return $true
}

if ($interopStep -in "launch", "all") {
  # Onboarding: name the workspace, create it, then the recovery kit.
  $name = Find-El "Edit" "Workspace name" 20
  if ($name) {
    Set-El $name "Interop workspace"
    Start-Sleep -Milliseconds 300
    Invoke-El (Find-El "Button" "Create workspace" 5)
    Start-Sleep -Seconds 3
  }
  if (-not (Create-Kit)) {
    $failedPhases += "launch"
    "FAILED: the recovery kit"
    Capture-Window $hwnd "$out\interop-launch-failed.png"
  }
  Run-Phase "launch"
  Capture-Window $hwnd "$out\interop-launched.png"
}

foreach ($phase in "seed", "fixtures", "import-zip", "import-folder", "import-files", "export-zip", "reimport", "export-html", "export-space", "csv", "print", "backup", "guard", "cancel", "import-hostile") {
  if ($interopStep -in $phase, "all") { Run-Phase $phase }
}

if ($interopStep -in "verify", "all") {
  & node "$PSScriptRoot\drive-interop.mjs" "verify" "$out\interop-report.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "verify" }
}

if ($interopStep -eq "all") {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  "stopped the app (node and Vite were left alone)"
}
if ($failedPhases.Count -gt 0) { "FAILED: $($failedPhases -join ', ')"; exit 1 }
"interop pass: all phases ok"
