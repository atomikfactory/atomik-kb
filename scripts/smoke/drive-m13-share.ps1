# Shared spaces pass (Milestone 13, slice 1) on the BUILT desktop binary: a local kb-server (protocol v2), three people (A the
# owner, B an editor, C a link invitee) and, later, A's second device and a kit recovery, each a separate instance on its own data
# directory with its own WebView2 profile and DevTools port. The real core, shell, sync driver and WebView2 under the production CSP.
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   cargo build -p kb-server                                      (once)
#   .\drive-m13-share.ps1 -Step launch    server.ps1 -Action reset (the server starts fresh), instances A (port 9333), B (9334), C (9335) on fresh
#                                         data (out\m13-share-<name>-data), each onboarded by UI Automation (workspace + recovery kit)
#   .\drive-m13-share.ps1 -Step share     A connects (http://127.0.0.1:8787, token dev), seeds a space (pages, a nested page, an image, a
#                                         trashed page), "Share this space...": the history warning is read, A sets a profile name, Share
#   .\drive-m13-share.ps1 -Step code      "Invite with a code": B joins with NO account on the server (the share signup makes it), the six
#                                         digits are compared in both windows, B confirms first, then A; B's tree equals A's, the image
#                                         reads; B edits and A sees it, A edits and B sees it; both show "Verified"
#   .\drive-m13-share.ps1 -Step roles     A sets B to Viewer (the editor, title and properties turn read-only, a forced write is refused),
#                                         then back to Editor
#   .\drive-m13-share.ps1 -Step link      "Invite with a link": C is started with the link as its argument (KB_SINGLE_INSTANCE=1), the
#                                         preview (inviter, space, role, whole history, not verified), accept, C sees the space
#   .\drive-m13-share.ps1 -Step remove    A removes B: the archive banner, a page A writes afterwards never reaches B, C still syncs
#   .\drive-m13-share.ps1 -Step leave     the AI switch is off by default on B and C; C leaves and A's member list updates
#   .\drive-m13-share.ps1 -Step reinvite  A invites the removed B again by code: B joins again and reads what was written while it was out
#   .\drive-m13-share.ps1 -Step devices   A's second device D (pairing on /v2/pair), a kit recovery on a fresh install E (the shared space
#                                         reads after it), attachment sync with deletion on the shared space
#   .\drive-m13-share.ps1 -Step restart   every instance is started again on its data: the state survives
#   .\drive-m13-share.ps1 -Step close     A closes the space: every member gets the read-only copy, the server refuses the write of a device
#                                         that never heard of it (the server stopped, that device frozen, then woken), B removes its copy
#   .\drive-m13-share.ps1 -Step restart-closed   the same restart, after the close
#   .\drive-m13-share.ps1 -Step logs      the server's and every instance's logs: no panic, no ERROR, no page text, name, code, link,
#                                         secret, kit word or full id
#   .\drive-m13-share.ps1 -Step verify    the summary of every phase (out\m13-share-report.json)
#   .\drive-m13-share.ps1 -Step stop      stops the instances and the server this pass started (never node)
#   .\drive-m13-share.ps1 -Step all       every step above in order, then stop
#
# Like drive-spaces.ps1 each instance is started with --remote-debugging-port, and drive-m13-share.mjs sends trusted mouse and key
# events to the pages over the DevTools ports, reads the DOM and the core through the pages' own IPC, and records every request,
# console message and content-security-policy violation per instance: a phase fails on a request to another host (the pages never
# talk to the server: the shell does), a violation, a failed request, a console error or a replaced document. UI Automation drives the
# onboarding; the native Open dialog and the captures are Win32 helper steps of this script that the .mjs calls (never SendKeys).
# Each instance has WEBVIEW2_USER_DATA_FOLDER of its own (the instances share nothing but the machine). Only the processes this pass
# started are stopped (by process id, never by name: another session may run the app). The link of the link invite lives in the
# memory of the node process of its phase and in the command line of C; the code, the link's secret and the kit words are written to
# out\m13-share-state.json only so that the log scan can look for them (never to a report or a log). Output lands in out\ (git-ignored):
# m13-share-*.png, m13-share-*.json, m13-share-<name>[-rN].err.log. A failing phase does not stop the others; the exit code is 1 when
# any failed. If the Windows session is locked, what needs real keys is listed as NOT VERIFIABLE, never as passed.
param([string]$Step = "all", [int]$BasePort = 9333, [string]$Result = "", [string]$Arg = "")
$ErrorActionPreference = "Continue"
$m13Step = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$failedPhases = @()

function Write-Result($obj) {
  $json = ConvertTo-Json -InputObject $obj -Depth 10 -Compress
  [System.IO.File]::WriteAllText($Result, $json, (New-Object System.Text.UTF8Encoding($false)))
}

# ---- helper steps, called by drive-m13-share.mjs -----------------------------------------------------------------------------
# Every one reads its parameters from the JSON file named by -Arg and writes one JSON document to -Result.
$helpers = @("dialog", "capture", "onboard", "session", "spawn", "kill", "freeze", "thaw", "windows")
if ($helpers -contains $m13Step) {
  $p = Get-Content -Raw -Encoding UTF8 -Path $Arg | ConvertFrom-Json
  Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
  if (-not ("M13Win" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class M13Win {
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
  // The file-name box of a common dialog: the visible Edit inside a ComboBox, control id 1001 (Save) or 1148 (Open).
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
}
"@
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
    $r = New-Object M13Win+RECT
    [M13Win]::GetWindowRect($h, [ref]$r) | Out-Null
    return [pscustomobject]@{
      hwnd = [int64]$h; pid = [int][M13Win]::Pid($h); class = [M13Win]::Cls($h); title = [M13Win]::Txt($h)
      visible = [M13Win]::IsWindowVisible($h); left = $r.Left; top = $r.Top; width = $r.Right - $r.Left; height = $r.Bottom - $r.Top
    }
  }

  switch ($m13Step) {
    "session" {
      # A locked Windows session (LogonUI.exe runs) keeps the keyboard and the clipboard from other programs.
      $locked = $null -ne (Get-Process -Name "LogonUI" -ErrorAction SilentlyContinue)
      Write-Result @{ ok = $true; locked = $locked }
      exit 0
    }
    { $_ -in "freeze", "thaw" } {
      # A device that is "offline for good": its process is suspended (NtSuspendProcess) and resumed. Only an app process
      # of this pass (named by the state file) is touched.
      if (-not ("M13Nt.Nt" -as [type])) {
        Add-Type -MemberDefinition '[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h); [DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr h);' -Name "Nt" -Namespace "M13Nt" | Out-Null
      }
      $proc = Get-Process -Id ([int]$p.pid) -ErrorAction SilentlyContinue
      if (-not $proc -or $proc.ProcessName -ne "atomik-kb-desktop") { Write-Result @{ ok = $false; error = "no such app process" }; exit 1 }
      $status = if ($m13Step -eq "freeze") { [M13Nt.Nt]::NtSuspendProcess($proc.Handle) } else { [M13Nt.Nt]::NtResumeProcess($proc.Handle) }
      Write-Result @{ ok = ($status -eq 0); status = $status }
      exit $(if ($status -eq 0) { 0 } else { 1 })
    }
    "kill" {
      # Only the processes this pass started (by id), never by name.
      $stopped = @()
      foreach ($id in @($p.pids)) {
        $proc = Get-Process -Id ([int]$id) -ErrorAction SilentlyContinue
        if ($proc -and $proc.ProcessName -eq "atomik-kb-desktop") { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; $stopped += [int]$id }
      }
      Write-Result @{ ok = $true; stopped = $stopped }
      exit 0
    }
    "spawn" {
      # $p.name, $p.data, $p.webview, $p.port, $p.log (a file name stem in out\), $p.rustLog, $p.singleInstance, $p.useLink (the link is in
      # the environment variable M13_LINK of this process, never in a file).
      $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
      if (-not (Test-Path $exe)) { Write-Result @{ ok = $false; error = "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop" }; exit 1 }
      New-Item -ItemType Directory -Force $p.data | Out-Null
      New-Item -ItemType Directory -Force $p.webview | Out-Null
      $before = @(Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
      $env:KB_DATA_DIR = $p.data
      $env:WEBVIEW2_USER_DATA_FOLDER = $p.webview
      $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$($p.port)"
      $env:RUST_LOG = $p.rustLog
      if ($p.singleInstance) { $env:KB_SINGLE_INSTANCE = "1" } else { Remove-Item Env:KB_SINGLE_INSTANCE -ErrorAction SilentlyContinue }
      $start = @{
        FilePath = $exe; PassThru = $true; WorkingDirectory = (Join-Path $repo "apps\desktop\src-tauri")
        RedirectStandardError = (Join-Path $out "$($p.log).err.log"); RedirectStandardOutput = (Join-Path $out "$($p.log).out.log")
      }
      if ($p.useLink -and $env:M13_LINK) { $start.ArgumentList = @($env:M13_LINK) }
      $proc = Start-Process @start
      Remove-Item Env:M13_LINK -ErrorAction SilentlyContinue
      $res = [ordered]@{ ok = $false; pid = $proc.Id; hwnd = 0; others = $before.Count }
      for ($i = 0; $i -lt 90; $i++) {
        Start-Sleep -Seconds 2
        $q = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
        if (-not $q) { $res.error = "the process ended before its window came up"; break }
        if ($q.MainWindowHandle -ne 0) { $res.ok = $true; $res.hwnd = [int64]$q.MainWindowHandle; break }
      }
      if (-not $res.ok -and -not $res.error) { $res.error = "no app window within 180 s" }
      Start-Sleep -Seconds 3
      Write-Result $res
      exit $(if ($res.ok) { 0 } else { 1 })
    }
    "windows" {
      $app = Get-Process -Id ([int]$p.pid) -ErrorAction SilentlyContinue
      if (-not $app) { Write-Result @{ ok = $false; error = "no such process" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $top = @(foreach ($h in [M13Win]::TopLevel($pids)) { Describe-Window $h })
      Write-Result @{ ok = $true; main = [int64]$app.MainWindowHandle; processes = $tree.names; top = $top }
      exit 0
    }
    "capture" {
      . "$PSScriptRoot\capture.ps1"
      $app = Get-Process -Id ([int]$p.pid) -ErrorAction SilentlyContinue
      if (-not $app -or $app.MainWindowHandle -eq 0) { Write-Result @{ ok = $false; error = "no window" }; exit 1 }
      $line = Capture-Window $app.MainWindowHandle $p.path
      Write-Result @{ ok = (Test-Path $p.path); detail = "$line" }
      exit 0
    }
    "dialog" {
      # $p.pid: the instance; $p.title: the dialog's title ("Open"); $p.steps: the texts to put in the file name box, each followed by
      # IDOK; $p.cancel: IDCANCEL instead; $p.timeoutMs: how long to wait for it (default 30 s).
      $app = Get-Process -Id ([int]$p.pid) -ErrorAction SilentlyContinue
      if (-not $app) { Write-Result @{ ok = $false; error = "no such process" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $find = {
        foreach ($h in [M13Win]::TopLevel($pids)) {
          if ([M13Win]::IsWindowVisible($h) -and [M13Win]::Cls($h) -eq "#32770" -and [M13Win]::Txt($h) -eq $p.title) { return $h }
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
      $res = [ordered]@{ ok = $false; title = $p.title; waitedMs = [int]((Get-Date) - $started).TotalMilliseconds; edit = $null; seen = @(); closed = $false; cancelled = $false }
      if ($dlg -eq [IntPtr]::Zero) {
        $res.error = "no '$($p.title)' dialog appeared"
        $res.seen = @(foreach ($h in [M13Win]::TopLevel($pids)) { if ([M13Win]::IsWindowVisible($h)) { "$([M13Win]::Cls($h)) '$([M13Win]::Txt($h))'" } })
        Write-Result $res; exit 1
      }
      Start-Sleep -Milliseconds 800
      if ($p.cancel) {
        [M13Win]::SetForegroundWindow($dlg) | Out-Null
        $res.cancelled = [M13Win]::PressId($dlg, 2)
        for ($i = 0; $i -lt 40; $i++) { if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }; Start-Sleep -Milliseconds 250 }
        $res.ok = $res.closed
        Write-Result $res
        exit $(if ($res.ok) { 0 } else { 1 })
      }
      $edit = [M13Win]::FileNameEdit($dlg)
      if ($edit -eq [IntPtr]::Zero) { $res.error = "no file-name edit in the '$($p.title)' dialog"; Write-Result $res; exit 1 }
      $res.edit = [M13Win]::GetDlgCtrlID($edit)
      [M13Win]::SetForegroundWindow($dlg) | Out-Null
      foreach ($text in @($p.steps)) {
        [M13Win]::SetText($edit, [string]$text)
        Start-Sleep -Milliseconds 500
        [M13Win]::PressOk($dlg)
        Start-Sleep -Milliseconds 900
      }
      for ($i = 0; $i -lt 40; $i++) { if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }; Start-Sleep -Milliseconds 250 }
      $res.ok = $res.closed
      if (-not $res.closed) { $res.error = "the '$($p.title)' dialog did not close (the path may have been refused)" }
      Write-Result $res
      exit $(if ($res.ok) { 0 } else { 1 })
    }
    "onboard" {
      # The first screen of a fresh installation, by UI Automation (the helpers of drive-m4.ps1): the workspace, then the recovery
      # kit (created, its 24 words read, three of them typed back, Done). The words go to $p.kitFile (a smoke-test secret of this
      # machine, git-ignored, read by the log scan and by the kit recovery). $p.skipKit does "Do this later" instead.
      . "$PSScriptRoot\drive-m4.ps1" -Step "none" -AppPid ([int]$p.pid) 2>$null | Out-Null
      function Fail([string]$why) { Write-Result @{ ok = $false; error = $why }; exit 1 }
      $name = Find-El "Edit" "Workspace name" 40
      if (-not $name) { Fail "no 'Workspace name' field (not on onboarding)" }
      Set-El $name $p.workspace
      Start-Sleep -Milliseconds 400
      $create = Find-El "Button" "Create workspace" 5
      if (-not $create) { Fail "no 'Create workspace' button" }
      Invoke-El $create
      Start-Sleep -Seconds 3
      $words = @()
      if ($p.skipKit) {
        $later = Find-El "" "Do this later" 15
        if ($later) { Invoke-El $later; Start-Sleep -Seconds 3 }
      } else {
        $explain = Find-El "Button" "Create my recovery kit" 20
        if (-not $explain) { Fail "no 'Create my recovery kit' button after the workspace" }
        Invoke-El $explain
        Start-Sleep -Seconds 4
        $words = Read-KitWords
        if ($words.Count -ne 24) { Fail "expected 24 kit words, got $($words.Count)" }
        [System.IO.File]::WriteAllText($p.kitFile, ($words -join " "), (New-Object System.Text.UTF8Encoding($false)))
        $saved = Find-El "Button" "I saved my kit" 10
        if (-not $saved) { Fail "no 'I saved my kit' button" }
        Invoke-El $saved
        Start-Sleep -Seconds 1
        foreach ($e in (All-El (Web))) {
          $c = $e.Current
          if ($c.ControlType.ProgrammaticName -eq "ControlType.Edit" -and $c.Name -match '^Word (\d+)$') { Set-El $e $words[[int]$Matches[1] - 1] }
        }
        Start-Sleep -Milliseconds 500
        $check = Find-El "Button" "Check" 10
        if (-not $check) { Fail "no 'Check' button" }
        Invoke-El $check
        Start-Sleep -Seconds 3
        $done = Find-El "Button" "Done" 20
        if (-not $done) { Fail "the word check did not pass" }
        Invoke-El $done
        Start-Sleep -Seconds 2
      }
      Write-Result @{ ok = $true; words = $words.Count }
      exit 0
    }
  }
  exit 0
}

# ---- the pass --------------------------------------------------------------------------------------------------------------
$env:M13_OUT_DIR = $out
$env:M13_PS1 = $PSCommandPath
$env:M13_BASE_PORT = "$BasePort"
$env:M13_REPO = $repo

function Run-Mjs([string]$phase) {
  $report = if ($phase -eq "verify") { "$out\m13-share-report.json" } else { "$out\m13-share-$phase.json" }
  $lines = & node "$PSScriptRoot\drive-m13-share.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $phase
    "PHASE FAILED: $phase (see $report)"
  }
}

function Stop-Pass() {
  # The instances this pass started (their ids are in the state file) and the server it started.
  & node "$PSScriptRoot\drive-m13-share.mjs" "stop" "$out\m13-share-stop.json" 2>&1 | ForEach-Object { "$_" }
  & "$PSScriptRoot\server.ps1" -Action stop | ForEach-Object { "$_" }
}

if ($m13Step -eq "stop") { Stop-Pass; exit 0 }
if ($m13Step -in "verify", "logs") {
  Run-Mjs $m13Step
  if ($failedPhases.Count -gt 0) { exit 1 }
  exit 0
}

if ($m13Step -in "launch", "all") {
  # The server starts fresh (protocol v2 refuses a data directory of v1): a new epoch, no accounts.
  . (Join-Path $repo "scripts\dev-env.ps1")
  $busy = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalPort -ge $BasePort -and $_.LocalPort -lt ($BasePort + 6) })
  if ($busy.Count -gt 0) {
    # Instances of an earlier pass of this script (their ids are in the state file) are stopped first.
    & node "$PSScriptRoot\drive-m13-share.mjs" "stop" "$out\m13-share-stop.json" 2>&1 | ForEach-Object { "$_" }
    Start-Sleep -Seconds 3
    $busy = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalPort -ge $BasePort -and $_.LocalPort -lt ($BasePort + 6) })
  }
  if ($busy.Count -gt 0) { "FAIL: the DevTools ports $BasePort-$($BasePort + 5) are in use by process $($busy[0].OwningProcess): pass -BasePort"; exit 1 }
  & "$PSScriptRoot\server.ps1" -Action reset | ForEach-Object { "$_" }
  # A fresh pass: the reports, captures and logs of an earlier one go.
  Get-ChildItem $out -Filter "m13-share*" -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

$phases = @("launch", "share", "code", "roles", "link", "remove", "leave", "reinvite", "devices", "restart", "close", "restart-closed")
try {
  foreach ($phase in $phases) {
    if ($m13Step -in $phase, "all") { Run-Mjs $phase }
  }
  if ($m13Step -eq "all") {
    Run-Mjs "logs"
    Run-Mjs "verify"
  }
} finally {
  if ($m13Step -eq "all") { Stop-Pass }
}
if ($failedPhases.Count -gt 0) { "FAILED: $($failedPhases -join ', ')"; exit 1 }
"m13 share pass: all phases ok"
