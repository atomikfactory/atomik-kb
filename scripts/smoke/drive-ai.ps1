# Local AI and MCP pass (ADR-0037) on the BUILT desktop app, the real WebView2 under the production CSP, against the real
# Ollama on 127.0.0.1:11434 (qwen3:8b, nomic-embed-text, r2t2) and the real kb-mcp-bridge:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   cargo build -p kb-mcp --bin kb-mcp-bridge                    (once; the app looks for it next to its executable)
#   .\drive-ai.ps1 -Step launch      fresh data directory out\ai-data, onboarding by UI Automation, the marker of the document is set
#   .\drive-ai.ps1 -Step seed        two pages through the UI: "Budget review" and "Garden"
#   .\drive-ai.ps1 -Step settings    Settings -> AI: on, Connected + version, default models (qwen3:8b, nomic-embed-text), transcription r2t2
#   .\drive-ai.ps1 -Step consent     a remote endpoint -> the NATIVE "Send text to another computer?" dialog -> Cancel: nothing changes
#   .\drive-ai.ps1 -Step guard       through the page's own IPC: SettingsSet app.ai.endpoint, AiIndexNext, McpCall, ... are refused
#   .\drive-ai.ps1 -Step index       waits until Settings shows every page indexed (the semantic index, nomic-embed-text)
#   .\drive-ai.ps1 -Step writer      select a paragraph -> AI -> Improve -> Replace; Ctrl+Z restores it in ONE step; /ai opens the panel
#   .\drive-ai.ps1 -Step chat        Ctrl+J, a question about the pages: an answer with a citation chip that opens the page
#   .\drive-ai.ps1 -Step related     Ctrl+K "watering vegetables": a Related group lists "Garden" (by meaning)
#   .\drive-ai.ps1 -Step transcribe  /audio -> test.wav -> "Transcribe" in the block menu -> a Transcript toggle and a summary
#   .\drive-ai.ps1 -Step record      microphone: the app is started again with a fake device (and auto-accepted permission): /record, Stop;
#                                    then with the fake device only: does WebView2 prompt, allow or deny the microphone?
#   .\drive-ai.ps1 -Step record-prompt | record-fake   the two halves of record, alone (each starts the binary again)
#   .\drive-ai.ps1 -Step mcp         Settings -> MCP: on, a client, its token, grants (native dialog), the bridge over stdio, raw HTTP checks
#   .\drive-ai.ps1 -Step logs        the log of the shell and the core: no panic, no ERROR, no token, page text or host
#   .\drive-ai.ps1 -Step verify      the summary of every phase (out\ai-report.json)
#   .\drive-ai.ps1 -Step all         every phase above, then stops the binary
#
# Like drive-interop.ps1 the app is started with --remote-debugging-port, and drive-ai.mjs sends trusted mouse and key events
# to the page over the WebView2 DevTools port (cdp.mjs' calls), reads the DOM and the core through the page's own IPC, and
# records every request, console message and content-security-policy violation: a phase fails on a request to another host
# (the page must never talk to Ollama: the shell does the HTTP; 127.0.0.1:11434 is "another host" for the page), a violation,
# a failed request, a console error, or when the document of the webview was replaced. The native dialogs (file dialogs and the
# two confirmations of the shell) are driven with Win32 by this script's helper steps, which the .mjs calls (never SendKeys).
# Output lands in out\ (git-ignored): ai-*.png, ai-*.json, ai.err.log (and ai-r1..3.err.log of the restarts); the data directory
# is out\ai-data (never the person's own: KB_DATA_DIR). A failing phase does not stop the others; the exit code is 1 when any
# failed. Only this binary is stopped (never node: a Vite dev server may be running for another session).
param([string]$Step = "all", [int]$Port = 9333, [string]$Result = "", [string]$Arg = "", [string]$Wav = "", [string]$Answers = "none,block,allow")
$ErrorActionPreference = "Continue"
$aiStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$failedPhases = @()

function Write-Result($obj) {
  $json = ConvertTo-Json -InputObject $obj -Depth 10 -Compress
  [System.IO.File]::WriteAllText($Result, $json, (New-Object System.Text.UTF8Encoding($false)))
}

# ---- helper steps, called by drive-ai.mjs ---------------------------------------------------------------------------------
# Every one reads its parameters from the JSON file named by -Arg and writes one JSON document to -Result.
$helpers = @("dialog", "message-dialog", "windows", "uia", "capture-hwnd", "invoke")
if ($helpers -contains $aiStep) {
  $p = Get-Content -Raw -Encoding UTF8 -Path $Arg | ConvertFrom-Json
  Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
  if (-not ("AiWin" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class AiWin {
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
  [StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
  [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO info);
  [DllImport("kernel32.dll")] public static extern uint GetTickCount();
  // Milliseconds since the last keyboard or mouse input of the real user (to tell a person's click from the pass's own work).
  public static long IdleMs() { var i = new LASTINPUTINFO(); i.cbSize = (uint)Marshal.SizeOf(i); GetLastInputInfo(ref i); return (long)(GetTickCount() - i.dwTime); }
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
  // A button of a message box or task dialog by its text ("Allow", "Cancel"): BM_CLICK.
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
  # The app's process and every process below it (the WebView2 browser process owns the permission prompt).
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
    $r = New-Object AiWin+RECT
    [AiWin]::GetWindowRect($h, [ref]$r) | Out-Null
    return [pscustomobject]@{
      hwnd = [int64]$h; pid = [int][AiWin]::Pid($h); class = [AiWin]::Cls($h); title = [AiWin]::Txt($h)
      visible = [AiWin]::IsWindowVisible($h); owner = [int64][AiWin]::GetWindow($h, 4)
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

  switch ($aiStep) {
    "dialog" {
      # $p.title: the dialog's title ("Save As", "Open" or "Select Folder"); $p.steps: the texts to put in the file name box,
      # each followed by IDOK (the last one closes the dialog); $p.cancel: IDCANCEL instead.
      $app = Get-AppProcess
      if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $find = {
        foreach ($h in [AiWin]::TopLevel($pids)) {
          if ([AiWin]::IsWindowVisible($h) -and [AiWin]::Cls($h) -eq "#32770" -and [AiWin]::Txt($h) -eq $p.title) { return $h }
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
        $res.seen = @(foreach ($h in [AiWin]::TopLevel($pids)) { if ([AiWin]::IsWindowVisible($h)) { "$([AiWin]::Cls($h)) '$([AiWin]::Txt($h))'" } })
        Write-Result $res; exit 1
      }
      Start-Sleep -Milliseconds 800
      if ($p.cancel) {
        [AiWin]::SetForegroundWindow($dlg) | Out-Null
        $res.cancelled = [AiWin]::PressId($dlg, 2)
        for ($i = 0; $i -lt 40; $i++) {
          if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }
          Start-Sleep -Milliseconds 250
        }
        $res.ok = $res.closed
        if (-not $res.closed) { $res.error = "the '$($p.title)' dialog did not close when cancelled" }
        Write-Result $res
        exit $(if ($res.ok) { 0 } else { 1 })
      }
      $edit = [AiWin]::FileNameEdit($dlg)
      $res.controls = @(foreach ($c in [AiWin]::Children($dlg)) { if ([AiWin]::IsWindowVisible($c)) { "$([AiWin]::Cls($c)) id=$([AiWin]::GetDlgCtrlID($c))" } })
      if ($edit -eq [IntPtr]::Zero) { $res.error = "no file-name edit in the '$($p.title)' dialog"; Write-Result $res; exit 1 }
      $res.edit = [AiWin]::GetDlgCtrlID($edit)
      $res.initial = [AiWin]::GetText($edit)
      [AiWin]::SetForegroundWindow($dlg) | Out-Null
      $steps = @($p.steps)
      for ($s = 0; $s -lt $steps.Count; $s++) {
        [AiWin]::SetText($edit, [string]$steps[$s])
        Start-Sleep -Milliseconds 500
        [AiWin]::PressOk($dlg)
        Start-Sleep -Milliseconds 900
        foreach ($h in [AiWin]::TopLevel($pids)) {
          if ([AiWin]::IsWindowVisible($h) -and [AiWin]::Cls($h) -eq "#32770" -and [AiWin]::Txt($h) -like "Confirm*") {
            $res.confirmed = [AiWin]::PressId($h, 6)
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
      # button's text: "Allow" or "Cancel"), $p.capture (a PNG path, or empty), $p.timeoutMs. The texts and button names that UI
      # Automation and Win32 show are returned, so the page's caller can check the question that was asked.
      $app = Get-AppProcess
      if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
      $tree = Get-Tree $app.Id
      $pids = [uint32[]]@($tree.ids | ForEach-Object { [uint32]$_ })
      $find = {
        foreach ($h in [AiWin]::TopLevel($pids)) {
          if ([AiWin]::IsWindowVisible($h) -and [AiWin]::Txt($h) -eq $p.title) { return $h }
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
        $res.seen = @(foreach ($h in [AiWin]::TopLevel($pids)) { if ([AiWin]::IsWindowVisible($h)) { "$([AiWin]::Cls($h)) '$([AiWin]::Txt($h))'" } })
        Write-Result $res; exit 1
      }
      Start-Sleep -Milliseconds 700
      $res.class = [AiWin]::Cls($dlg)
      $res.children = @(foreach ($c in [AiWin]::Children($dlg)) { if ([AiWin]::IsWindowVisible($c)) { "$([AiWin]::Cls($c)) '$([AiWin]::Txt($c))'" } })
      $uia = Read-Uia $dlg
      $res.texts = @($uia.texts)
      $res.buttons = @($uia.buttons)
      if ($p.capture) {
        . "$PSScriptRoot\capture.ps1"
        Capture-Window $dlg $p.capture | Out-Null
        $res.captured = if (Test-Path $p.capture) { $p.capture } else { $null }
      }
      [AiWin]::SetForegroundWindow($dlg) | Out-Null
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
        $pressed = [AiWin]::PressText($dlg, [string]$p.press)
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
      $top = @(foreach ($h in [AiWin]::TopLevel($pids)) { Describe-Window $h })
      $kids = @(foreach ($h in [AiWin]::Children($app.MainWindowHandle)) { $d = Describe-Window $h; if ($d.visible -and $d.class -ne "") { $d } })
      Write-Result @{ ok = $true; main = [int64]$app.MainWindowHandle; processes = $tree.names; top = $top; children = $kids; idleMs = [AiWin]::IdleMs() }
      exit 0
    }
    "uia" {
      # The UI Automation tree under a window ($p.hwnd, or the app's main window): type, name, class and id of every named element.
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
    "invoke" {
      # Presses a button of a window by its name with UI Automation ($p.hwnd, $p.name): the permission bubble of the webview is
      # a window of its own, with the buttons Close, Block and Allow.
      $el = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr][int64]$p.hwnd)
      $cond = New-Object System.Windows.Automation.AndCondition(
        (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)),
        (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, [string]$p.name)))
      $button = $el.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
      if (-not $button) { Write-Result @{ ok = $false; error = "no button '$($p.name)'" }; exit 1 }
      $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
      Write-Result @{ ok = $true }
      exit 0
    }
    "capture-hwnd" {
      . "$PSScriptRoot\capture.ps1"
      $line = Capture-Window ([IntPtr][int64]$p.hwnd) $p.path
      Write-Result @{ ok = (Test-Path $p.path); detail = "$line" }
      exit 0
    }
  }
  exit 0
}

$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$script:launches = 0

# Starts the built app on out\ai-data with the DevTools port. -Keep keeps the data (a restart); -Browser adds WebView2 arguments
# (a fake microphone); the log of a restart goes to ai-rN.err.log so that the first one is kept for the scan.
function Launch-Built([switch]$Keep, [string]$Browser = "") {
  . (Join-Path $repo "scripts\dev-env.ps1")
  # Only this binary is stopped (never node: a Vite dev server may be running for another session).
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $data = Join-Path $out "ai-data"
  if ((Test-Path $data) -and -not $Keep) { Remove-Item -Recurse -Force $data }
  if (-not $Keep) {
    # A fresh pass: the reports, captures and state of an earlier one go (the logs are written again by this run).
    Get-ChildItem $out -Filter "ai-*" -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne "ai-data" -and $_.Name -notlike "ai-test.wav" } | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    Get-ChildItem $out -Filter "ai.*" -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    $script:launches = 0
  }
  New-Item -ItemType Directory -Force $data | Out-Null
  $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
  if (-not (Test-Path $exe)) { "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop"; exit 1 }
  $log = if ($script:launches -eq 0) { "ai" } else { "ai-r$($script:launches)" }
  $script:launches += 1
  $env:KB_DATA_DIR = $data
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug,kb_ai=debug,kb_mcp=debug"
  $browserArgs = "--remote-debugging-port=$Port"
  if ($Browser -ne "") { $browserArgs = "$browserArgs $Browser" }
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $browserArgs
  $proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") `
    -RedirectStandardError "$out\$log.err.log" -RedirectStandardOutput "$out\$log.out.log"
  Remove-Item Env:KB_DATA_DIR
  Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 2
    $q = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if ($q -and $q.MainWindowHandle -ne 0) {
      "app window up: pid=$($q.Id) log=$log.err.log browser args='$browserArgs'"
      $script:hwnd = $q.MainWindowHandle
      Start-Sleep -Seconds 5
      return
    }
  }
  "no app window"; exit 1
}

# A speech WAV (16 kHz mono) for the transcription: the given file, else the one of the session, else the one Windows speech makes.
function Ensure-Wav() {
  $target = Join-Path $out "ai-test.wav"
  if (Test-Path $target) { return }
  $candidates = @($Wav, $env:AI_TEST_WAV) | Where-Object { $_ -and (Test-Path $_) }
  if ($candidates.Count -gt 0) { Copy-Item -Force $candidates[0] $target; "speech WAV copied from $($candidates[0])"; return }
  try {
    Add-Type -AssemblyName System.Speech
    $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
    $fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
    $synth.SetOutputToWaveFile($target, $fmt)
    $synth.Speak("Hello Ben. The quarterly meeting moved to Thursday at three pm. Action item: Alice sends the budget draft.")
    $synth.Dispose()
    "speech WAV made with Windows speech"
  } catch { "no speech WAV (pass -Wav): $($_.Exception.Message)" }
}

# The summary and the scan of the log read what the phases left: no app needed.
if ($aiStep -in "verify", "logs") {
  $env:AI_OUT_DIR = $out
  & node "$PSScriptRoot\drive-ai.mjs" $aiStep "$out\ai-$(if ($aiStep -eq 'verify') { 'report' } else { 'logs' }).json" 2>&1 | ForEach-Object { "$_" }
  exit $LASTEXITCODE
}
if ($aiStep -eq "launch" -or $aiStep -eq "all") { Launch-Built; Ensure-Wav }

# Dot-sourcing the helpers runs their own `param`, which overwrites $Step: ours is in $aiStep.
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$env:CDP_PORT = "$Port"
# The .mjs captures the window and drives the dialogs itself, at the moments it needs (it runs this script's helpers).
$env:AI_CAPTURE_PS1 = Join-Path $PSScriptRoot "capture.ps1"
$env:AI_PS1 = $PSCommandPath
$env:AI_OUT_DIR = $out

function Stop-App() {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 3
}

# The webview keeps a microphone permission (Allow or Block) in its profile, %LOCALAPPDATA%\com.atomik-kb.desktop\EBWebView, which
# every run of the app shares, this pass and the person's own dev app alike. It also keeps an "ignore embargo": a prompt left
# unanswered a few times makes Chromium refuse the microphone for days without asking (`permission_autoblocking_data`,
# `AudioCapture.ignore_count`). Both entries are put back to nothing (the app must be stopped: the webview rewrites the file
# when it quits), before a run that needs a prompt and after one that left a choice. Nothing else in the file is touched.
function Reset-MicPermission([string]$why) {
  $prefs = Join-Path $env:LOCALAPPDATA "com.atomik-kb.desktop\EBWebView\Default\Preferences"
  if (-not (Test-Path $prefs)) { "microphone permission: no profile file ($why)"; return }
  $script = @'
const fs = require("fs");
const file = process.argv[2];
const prefs = JSON.parse(fs.readFileSync(file, "utf8"));
const exceptions = prefs?.profile?.content_settings?.exceptions;
if (!exceptions) { console.log("no content settings"); process.exit(0); }
const was = { mic: exceptions.media_stream_mic ?? null, embargo: exceptions.permission_autoblocking_data ?? null };
const empty = (v) => v === null || (typeof v === "object" && Object.keys(v).length === 0);
if (empty(was.mic) && empty(was.embargo)) { console.log("none stored"); process.exit(0); }
exceptions.media_stream_mic = {};
exceptions.permission_autoblocking_data = {};
fs.writeFileSync(file, JSON.stringify(prefs));
console.log("was " + JSON.stringify(was).replace(/"last_[a-z]+":"\d+",?/g, "") + ", reset to {}");
'@
  # A file, not `node -e`: Windows PowerShell 5.1 drops the double quotes of a native argument.
  $js = Join-Path $out "reset-mic.cjs"
  [System.IO.File]::WriteAllText($js, $script, (New-Object System.Text.UTF8Encoding($false)))
  $result = & node $js $prefs 2>&1
  "microphone permission ($why): $result"
}

function Run-Mjs([string]$phase, [string]$reportName = "") {
  if ($reportName -eq "") { $reportName = $phase }
  $report = "$out\ai-$reportName.json"
  $lines = & node "$PSScriptRoot\drive-ai.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $reportName
    if (Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue) { Capture-Window $hwnd "$out\ai-$reportName-failed.png" }
    "PHASE FAILED: $reportName (see $report)"
  }
}

function Run-Phase([string]$phase) {
  switch ($phase) {
    "record-prompt" {
      # A fake microphone and the webview's own permission handling: prompt, allow or deny? Three runs from a profile that
      # has no microphone permission (the webview keeps the person's choice in its profile, shared with the dev app):
      # none = only look at the prompt, block = the person presses Block, allow = the person presses Allow.
      foreach ($answer in $Answers.Split(",")) {
        Stop-App
        Reset-MicPermission "before the $answer run"
        $env:AI_PROMPT_ANSWER = $answer
        Launch-Built -Keep -Browser "--use-fake-device-for-media-stream"
        Run-Mjs "mark"
        Run-Mjs "record-prompt" "record-prompt-$answer"
      }
      Remove-Item Env:AI_PROMPT_ANSWER -ErrorAction SilentlyContinue
      Stop-App
      Reset-MicPermission "after the permission runs"
    }
    "record-fake" {
      # A fake microphone with its permission accepted: the recorder records, saves, transcribes.
      Stop-App
      Reset-MicPermission "before the fake-ui run"
      # The fake microphone plays the speech WAV once (Chromium's --use-file-for-fake-audio-capture, "%noloop"), so the recording
      # has words to transcribe.
      $speech = Join-Path $out "ai-test.wav"
      Launch-Built -Keep -Browser "--use-fake-device-for-media-stream --use-fake-ui-for-media-stream --use-file-for-fake-audio-capture=`"$speech%noloop`""
      Run-Mjs "mark"
      Run-Mjs "record" "record"
      Stop-App
      Reset-MicPermission "after the fake-ui run"
    }
    "record" {
      Run-Phase "record-prompt"
      Run-Phase "record-fake"
      # Back to the plain app for the phases that follow.
      Launch-Built -Keep
      Run-Mjs "mark"
    }
    default { Run-Mjs $phase }
  }
}

if ($aiStep -in "launch", "all") {
  # Onboarding: name the workspace and create it.
  $name = Find-El "Edit" "Workspace name" 20
  if ($name) {
    Set-El $name "AI workspace"
    Start-Sleep -Milliseconds 300
    Invoke-El (Find-El "Button" "Create workspace" 5)
    Start-Sleep -Seconds 3
  }
  # The recovery kit is not what this pass is about (no backups here): "Do this later".
  $later = Find-El "" "Do this later" 12
  if ($later) { Invoke-El $later; Start-Sleep -Seconds 3 } else { "no 'Do this later' on the recovery-kit screen (continuing)" }
  Run-Mjs "launch"
  Capture-Window $hwnd "$out\ai-launched.png"
}

foreach ($phase in "seed", "settings", "consent", "guard", "index", "writer", "chat", "related", "transcribe", "record", "mcp") {
  if ($aiStep -in $phase, "all") {
    # A phase run alone is on the app that is running: the marker of its document is (re)set first.
    if ($aiStep -eq $phase -and $phase -ne "record") { Run-Mjs "mark" }
    Run-Phase $phase
  }
}
# The two halves of "record", alone (each starts the binary again).
if ($aiStep -in "record-prompt", "record-fake") { Run-Phase $aiStep }

if ($aiStep -in "logs", "all") {
  & node "$PSScriptRoot\drive-ai.mjs" "logs" "$out\ai-logs.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "logs" }
}
if ($aiStep -in "verify", "all") {
  & node "$PSScriptRoot\drive-ai.mjs" "verify" "$out\ai-report.json" 2>&1 | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) { $failedPhases += "verify" }
}

if ($aiStep -eq "all") {
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  "stopped the app (node and Vite were left alone)"
}
if ($failedPhases.Count -gt 0) { "FAILED: $($failedPhases -join ', ')"; exit 1 }
"ai pass: all phases ok"
