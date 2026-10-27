# Dictation (ADR-0053) and sorting smoke on the dev app (`pnpm tauri dev`), over the WebView2 DevTools port.
#
#   . .\scripts\dev-env.ps1; .\scripts\smoke\drive-dictation.ps1 -Wav <16 kHz mono wav> [-Step all|launch|run|stop]
#
# What it proves (drive-dictation.mjs, trusted input over CDP; unit tests cover the logic):
#   A. /microphone with the REAL kb-speech.exe sidecar and the Light engine (Moonshine): the first-use dialog, the real download
#      through the loopback proxy (the native consent dialog is accepted with UI Automation / BM_CLICK, never SendKeys), phrases
#      landing in the page from a fake microphone fed by a WAV, Esc stops, nothing more is inserted, Ctrl+Z removes only the
#      last phrase, kb-speech.exe exits on its own after the stop (the idle exit).
#   B. Sorting: "Sort subpages A-Z" / "by date created" in the tree context menu, structure undo (Ctrl+Z outside a text
#      field), and `/sort list` on a bulleted list with a nested item.
#
# Needs, once:  cargo build -p kb-speech-native   (target\debug\kb-speech.exe and the DLLs; dev-env sets KB_SPEECH_VAD)
#               node scripts\dev-ipv4-proxy.mjs 3128   (the Light model is downloaded through it: Rust sockets get 10013 here)
# The microphone is Chromium's fake device (WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS) reading -Wav in a loop. A fresh data
# directory out\dict-data (KB_DATA_DIR) is used; -Keep reuses it (the model stays installed). Output: out\dict-*.png/json/log.
# -Preseed copies a folder of Moonshine files (-ModelDir) into the data directory's speech store instead of downloading
# (the fallback when the native consent dialog cannot be driven); the real download is the default.
param([string]$Step = "all", [string]$Wav = "", [int]$Port = 9333, [string]$Result = "", [string]$Arg = "", [switch]$Keep)
$ErrorActionPreference = "Continue"
$dictStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
$dataDir = Join-Path $out "dict-data"
New-Item -ItemType Directory -Force $out | Out-Null

function Write-Result($obj) {
  $json = ConvertTo-Json -InputObject $obj -Depth 10 -Compress
  [System.IO.File]::WriteAllText($Result, $json, (New-Object System.Text.UTF8Encoding($false)))
}

# ---- helper steps, called by drive-dictation.mjs ------------------------------------------------------------------------
if ($dictStep -in "message-dialog", "capture-main") {
  $p = Get-Content -Raw -Encoding UTF8 -Path $Arg | ConvertFrom-Json
  Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
  if (-not ("DictWin" -as [type])) {
    Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class DictWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public static string Cls(IntPtr h) { var c = new StringBuilder(256); GetClassName(h, c, 256); return c.ToString(); }
  public static string Txt(IntPtr h) { var t = new StringBuilder(512); GetWindowText(h, t, 512); return t.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static List<IntPtr> TopLevel(uint pid) {
    var found = new List<IntPtr>();
    EnumWindows((h, l) => { if (Pid(h) == pid) { found.Add(h); } return true; }, IntPtr.Zero);
    return found;
  }
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
  $app = Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if (-not $app) { Write-Result @{ ok = $false; error = "no app window" }; exit 1 }
  if ($dictStep -eq "capture-main") {
    . "$PSScriptRoot\capture.ps1"
    $msg = Capture-Window $app.MainWindowHandle $p.path
    Write-Result @{ ok = (Test-Path $p.path); path = $p.path; message = "$msg" }
    exit 0
  }
  # message-dialog: the shell's native confirmation ("Download a speech model?"): $p.title, $p.press, $p.capture, $p.timeoutMs.
  $find = {
    foreach ($h in [DictWin]::TopLevel([uint32]$app.Id)) {
      if ([DictWin]::IsWindowVisible($h) -and [DictWin]::Txt($h) -eq $p.title) { return $h }
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
  $res = [ordered]@{ ok = $false; title = $p.title; waitedMs = [int]((Get-Date) - $started).TotalMilliseconds; texts = @(); buttons = @(); pressed = $false; method = $null; closed = $false }
  if ($dlg -eq [IntPtr]::Zero) {
    $res.error = "no '$($p.title)' dialog appeared"
    $res.seen = @(foreach ($h in [DictWin]::TopLevel([uint32]$app.Id)) { if ([DictWin]::IsWindowVisible($h)) { "$([DictWin]::Cls($h)) '$([DictWin]::Txt($h))'" } })
    Write-Result $res; exit 1
  }
  Start-Sleep -Milliseconds 700
  $el = [System.Windows.Automation.AutomationElement]::FromHandle($dlg)
  foreach ($e in $el.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
    $c = $e.Current
    $type = $c.ControlType.ProgrammaticName -replace "ControlType.", ""
    if ($type -eq "Button" -and $c.Name -ne "") { $res.buttons += $c.Name }
    elseif ($type -eq "Text" -and $c.Name -ne "") { $res.texts += $c.Name }
  }
  if ($p.capture) { . "$PSScriptRoot\capture.ps1"; Capture-Window $dlg $p.capture | Out-Null }
  [DictWin]::SetForegroundWindow($dlg) | Out-Null
  try {
    $cond = New-Object System.Windows.Automation.AndCondition(
      (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)),
      (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, [string]$p.press)))
    $button = $el.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($button) { $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); $res.pressed = $true; $res.method = "uia-invoke" }
  } catch { $res.uiaError = "$($_.Exception.Message)" }
  if (-not $res.pressed) { $res.pressed = [DictWin]::PressText($dlg, [string]$p.press); if ($res.pressed) { $res.method = "bm-click" } }
  for ($i = 0; $i -lt 40; $i++) { if ((& $find) -eq [IntPtr]::Zero) { $res.closed = $true; break }; Start-Sleep -Milliseconds 250 }
  $res.ok = ($res.pressed -and $res.closed)
  Write-Result $res
  exit $(if ($res.ok) { 0 } else { 1 })
}

# ---- launch / run / stop ------------------------------------------------------------------------------------------------
. (Join-Path $repo "scripts\dev-env.ps1")

function Launch-App {
  if (-not $Wav -or -not (Test-Path $Wav)) { "need -Wav <16 kHz mono wav>"; exit 2 }
  if (-not (Test-Path (Join-Path $repo "target\debug\kb-speech.exe"))) { "no target\debug\kb-speech.exe: run 'cargo build -p kb-speech-native' first"; exit 2 }
  if (-not (Get-NetTCPConnection -LocalPort 3128 -State Listen -ErrorAction SilentlyContinue)) {
    "the loopback proxy is not listening on 3128: starting scripts\dev-ipv4-proxy.mjs"
    Start-Process node -ArgumentList (Join-Path $repo "scripts\dev-ipv4-proxy.mjs"), "3128" -WindowStyle Hidden
    Start-Sleep -Seconds 2
  }
  & "$PSScriptRoot\stop.ps1" | Out-Null
  if ((Test-Path $dataDir) -and -not $Keep) { Remove-Item -Recurse -Force $dataDir }
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  $err = "$out\dict.err.log"; $stdout = "$out\dict.out.log"
  $env:KB_DATA_DIR = $dataDir
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug"
  # The fake microphone (Chromium flags merged with the DevTools port) and the proxy for the model download.
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port --use-fake-ui-for-media-stream --use-fake-device-for-media-stream --use-file-for-fake-audio-capture=`"$Wav`""
  $env:HTTPS_PROXY = "http://127.0.0.1:3128"
  $env:NO_PROXY = "127.0.0.1,localhost"
  $proc = Start-Process -FilePath "cmd.exe" -ArgumentList "/c", "pnpm", "tauri", "dev" -WorkingDirectory (Join-Path $repo "apps\desktop") `
    -PassThru -WindowStyle Hidden -RedirectStandardError $err -RedirectStandardOutput $stdout
  Remove-Item Env:KB_DATA_DIR, Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS, Env:HTTPS_PROXY, Env:NO_PROXY -ErrorAction SilentlyContinue
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

if ($dictStep -in "launch", "all") { Launch-App }
if ($dictStep -in "run", "all") {
  & node "$PSScriptRoot\drive-dictation.mjs" "$out\dict-report.json" $Wav 2>&1 | ForEach-Object { "$_" }
  $code = $LASTEXITCODE
  Get-Process -Name "kb-speech" -ErrorAction SilentlyContinue | ForEach-Object { "kb-speech.exe still running after the pass (pid $($_.Id))" }
}
if ($dictStep -in "stop", "all") {
  & "$PSScriptRoot\stop.ps1"
  Start-Sleep -Seconds 1
  $left = @(Get-Process -Name "kb-speech" -ErrorAction SilentlyContinue)
  if ($left.Count -gt 0) { "killing a left-over kb-speech.exe"; $left | Stop-Process -Force }
}
if ($code) { exit $code }

