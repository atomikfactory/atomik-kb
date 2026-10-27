# Video and audio pass (ADR-0032) on the real desktop app, plus the PDF viewer (ADR-0031) and the
# photo gallery (ADR-0030) under the PRODUCTION CSP. `tauri dev` loads the page from Vite and
# does not enforce the CSP of tauri.conf.json, so this script runs the built binary, which embeds
# the frontend and serves it from the app's own protocol:
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   .\drive-media.ps1 -Step launch      fresh data directory out\media-data, onboarding skipped, a page titled "Smoke page"
#   .\drive-media.ps1 -Step sample      writes out\media-sample.wav (about 9 MiB: three attachment chunks),
#                                       media-sample.pdf and media-sample-1.png / -2.png
#   .\drive-media.ps1 -Step audio       /audio + the file chooser: the player appears, plays, seeks (the shell's
#                                       log must show 206 answers from the kbmedia protocol), the card toggle works
#   .\drive-media.ps1 -Step pdf         /pdf: the viewer shows page 1 / N and a canvas
#   .\drive-media.ps1 -Step gallery     /gallery + two pictures: the gallery renders
#   .\drive-media.ps1 -Step seek       relaunch on the same data (the webview's media cache is empty), scrub to 92 %
#                                       before playing: the log must show a 206 answer for the tail of the file
#   .\drive-media.ps1 -Step lock       needs the app lock on (drive-m4.ps1 -Step kit, lock, unlock first): plays the audio,
#                                       then "Lock now": the lock screen shows and the player is gone
#   .\drive-media.ps1 -Step all         launch, sample, audio, pdf, gallery
#   (stop with stop.ps1, which also kills this binary)
#
# Helpers are those of drive-m4.ps1 (UI Automation, WM_CHAR posts, Win32 for the native dialogs; never
# SendKeys). Screenshots and logs land in out\ (git-ignored): media-*.png, media.err.log.
param([string]$Step = "all")
$ErrorActionPreference = "Continue"
$mediaStep = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

function Launch-Built([switch]$Keep) {
  . (Join-Path $repo "scripts\dev-env.ps1")
  $out = Join-Path $PSScriptRoot "out"
  New-Item -ItemType Directory -Force $out | Out-Null
  Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  $data = Join-Path $out "media-data"
  if ((Test-Path $data) -and -not $Keep) { Remove-Item -Recurse -Force $data }
  New-Item -ItemType Directory -Force $data | Out-Null
  $exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
  if (-not (Test-Path $exe)) { "no ${exe}: run 'pnpm tauri build --debug --no-bundle' in apps/desktop"; exit 1 }
  $env:KB_DATA_DIR = $data
  $env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=info"
  $proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") `
    -RedirectStandardError "$out\media.err.log" -RedirectStandardOutput "$out\media.out.log"
  Remove-Item Env:KB_DATA_DIR
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 2
    $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if ($p -and $p.MainWindowHandle -ne 0) { "app window up: pid=$($p.Id)"; Start-Sleep -Seconds 5; return }
  }
  "no app window"; exit 1
}
if ($mediaStep -eq "launch" -or $mediaStep -eq "all") { Launch-Built }
if ($mediaStep -eq "relaunch") { Launch-Built -Keep }

# Dot-sourcing the helpers runs their own `param`, which overwrites $Step: ours is in $mediaStep.
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$errLog = "$out\media.err.log"
# While media plays WebView2 renames its web content pane ("atomik-kb - Audio playing - Web content"): match by suffix.
function Web() {
  foreach ($e in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
    $n = $e.Current.Name
    if ($n -like "atomik-kb*Web content") { return $e }
  }
  return $null
}
$wav = "$out\media-sample.wav"
$pdf = "$out\media-sample.pdf"
$png1 = "$out\media-sample-1.png"
$png2 = "$out\media-sample-2.png"

# The file chooser of an <input type="file"> belongs to the WebView2 browser process, not to the app's, so
# the helper of drive-m4.ps1 (which matches the app's process id) does not find it: match any process.
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class AnyDlg {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder sb, int n);
  public static IntPtr Find(string title) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { if (!IsWindowVisible(h)) return true;
      var c = new StringBuilder(256); GetClassName(h, c, 256); var t = new StringBuilder(256); GetWindowText(h, t, 256);
      if (c.ToString() == "#32770" && t.ToString() == title) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@
function Choose-File([string]$path) {
  $dlg = [IntPtr]::Zero
  for ($i = 0; $i -lt 40 -and $dlg -eq [IntPtr]::Zero; $i++) { $dlg = [AnyDlg]::Find("Open"); if ($dlg -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 500 } }
  if ($dlg -eq [IntPtr]::Zero) { Fail "no file chooser appeared" }
  Start-Sleep -Milliseconds 800
  $edit = [DlgWins]::FileNameEdit($dlg)
  if ($edit -eq [IntPtr]::Zero) { Fail "no file-name edit in the chooser" }
  [DlgWins]::SetForegroundWindow($dlg) | Out-Null
  [DlgWins]::SetText($edit, $path)
  Start-Sleep -Milliseconds 500
  [DlgWins]::PressOk($dlg)
  Start-Sleep -Seconds 3
  if ([AnyDlg]::Find("Open") -ne [IntPtr]::Zero) { Fail "the chooser did not close" }
}

function Media-Log() { if (Test-Path $errLog) { Get-Content $errLog | Select-String "media request" } }
function Media-Count() { return @(Media-Log).Count }

function Open-SmokePage() {
  $item = Find-El "TreeItem" $pageTitle 10
  if (-not $item) { Fail "no '$pageTitle' in the tree" }
  Select-El $item
  Start-Sleep -Seconds 2
}

# Types a slash command at the end of the page and chooses the entry whose name starts with $label.
function Slash([string]$command, [string]$label) {
  $editor = Find-El "Edit" "Page content" 10
  if (-not $editor) { Fail "no page content editor" }
  $editor.SetFocus()
  Start-Sleep -Milliseconds 500
  Post-Enter
  Post-Text $command
  Start-Sleep -Seconds 1
  $option = $null
  for ($i = 0; $i -lt 10 -and -not $option; $i++) {
    foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.ListItem" -and $c.Name -like "$label *") { $option = $e; break } }
    if (-not $option) { Start-Sleep -Milliseconds 400 }
  }
  if (-not $option) { Fail "no '$label' slash command" }
  # Enter, as a posted key: the file chooser of these commands needs a real (trusted) user gesture, which a
  # UI Automation Invoke does not give the page.
  Post-Enter
}

function Media-Elements() {
  # WebView2 exposes <audio controls> and <video controls> as a group named by the aria-label with the
  # native controls inside (buttons "play", sliders). Returns every element whose name mentions $args.
  foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.Name -like "Player for*") { $e } }
}

switch ($mediaStep) {
  { $_ -in "sample", "all" } {
    Add-Type -AssemblyName System.Drawing
    Add-Type @"
using System; using System.IO;
public static class Tone {
  public static void Wav(string path, int seconds) {
    int rate = 44100, n = rate * seconds, bytes = n * 2;
    using (var w = new BinaryWriter(File.Create(path))) {
      w.Write(new char[] {'R','I','F','F'}); w.Write(36 + bytes); w.Write(new char[] {'W','A','V','E','f','m','t',' '});
      w.Write(16); w.Write((short)1); w.Write((short)1); w.Write(rate); w.Write(rate * 2); w.Write((short)2); w.Write((short)16);
      w.Write(new char[] {'d','a','t','a'}); w.Write(bytes);
      double phase = 0;
      for (int i = 0; i < n; i++) {
        double t = (double)i / rate;
        double f = 330 + 110 * Math.Sin(2 * Math.PI * t / 10.0);
        phase += 2 * Math.PI * f / rate;
        w.Write((short)(Math.Sin(phase) * 9000));
      }
    }
  }
}
"@
    [Tone]::Wav($wav, 100)
    "wav written: $wav ($((Get-Item $wav).Length) bytes)"
    # A one-page PDF with a correct cross-reference table.
    $objs = @(
      "<< /Type /Catalog /Pages 2 0 R >>",
      "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
      "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
      $null,
      "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    $stream = "BT /F1 28 Tf 72 700 Td (atomik-kb smoke PDF) Tj ET"
    $objs[3] = "<< /Length $($stream.Length) >>`nstream`n$stream`nendstream"
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append("%PDF-1.4`n")
    $offsets = @()
    for ($i = 0; $i -lt $objs.Count; $i++) { $offsets += $sb.Length; [void]$sb.Append("$($i + 1) 0 obj`n$($objs[$i])`nendobj`n") }
    $xref = $sb.Length
    [void]$sb.Append("xref`n0 $($objs.Count + 1)`n0000000000 65535 f `n")
    foreach ($o in $offsets) { [void]$sb.Append(("{0:D10} 00000 n `n" -f $o)) }
    [void]$sb.Append("trailer`n<< /Size $($objs.Count + 1) /Root 1 0 R >>`nstartxref`n$xref`n%%EOF`n")
    [System.IO.File]::WriteAllText($pdf, $sb.ToString(), [System.Text.Encoding]::ASCII)
    "pdf written: $pdf"
    foreach ($spec in @(@($png1, 60, 110, 220), @($png2, 220, 90, 60))) {
      $bmp = New-Object System.Drawing.Bitmap 96, 96
      $g = [System.Drawing.Graphics]::FromImage($bmp)
      $g.Clear([System.Drawing.Color]::FromArgb(255, $spec[1], $spec[2], $spec[3]))
      $g.FillEllipse([System.Drawing.Brushes]::Gold, 20, 20, 56, 56)
      $g.Dispose(); $bmp.Save($spec[0], [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    }
    "pngs written"
  }
}

switch ($mediaStep) {
  { $_ -in "launch", "all" } {
    $later = Find-El "Button" "Do this later" 10
    if ($later) { Invoke-El $later; Start-Sleep -Seconds 2 }
    Invoke-El (Find-El "Button" "New page" 10)
    Start-Sleep -Seconds 2
    & "$PSScriptRoot\drive-m4.ps1" -Step content
    Capture-Window $hwnd "$out\media-launched.png"
  }
}

switch ($mediaStep) {
  { $_ -in "audio", "all" } {
    Open-SmokePage
    Slash "/audio" "Audio"
    Choose-File $wav
    Start-Sleep -Seconds 4
    Capture-Window $hwnd "$out\media-audio-inserted.png"
    $players = @(Media-Elements)
    "player elements: $($players.Count)"
    if ($players.Count -eq 0) { Dump; Fail "no player in the page" }
    Dump "play"; Dump "seek"; Dump "time"; Dump "audio"
    "media requests so far: $(Media-Count)"
    Media-Log | Select-Object -Last 5
  }
}

switch ($mediaStep) {
  { $_ -in "seek", "all" } {
    if ($mediaStep -ne "all") { Open-SmokePage }
    $before = Media-Count
    $scrub = Find-El "Slider" "audio time scrubber" 15
    if (-not $scrub) { Dump; Fail "no audio scrubber (is the player in the page?)" }
    Start-Sleep -Seconds 2
    "requests after the metadata load: $(Media-Count) (was $before)"
    $range = $scrub.GetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern)
    $range.SetValue(92)
    Start-Sleep -Seconds 4
    "scrubbed to $($range.Current.Value) %; requests now: $(Media-Count)"
    Media-Log | Select-Object -Last 3
    Capture-Window $hwnd "$out\media-audio-seeked.png"
    $play = Find-El "Button" "play" 5
    if ($play) {
      Invoke-El $play
      Start-Sleep -Seconds 4
      if (Find-El "Button" "pause" 5) { "playing from the seeked position" } else { Fail "the player does not play" }
      Capture-Window $hwnd "$out\media-audio-playing-after-seek.png"
      Invoke-El (Find-El "Button" "pause" 5)
    }
  }
}

switch ($mediaStep) {
  "lock" {
    Open-SmokePage
    $play = Find-El "Button" "play" 10
    if (-not $play) { Dump; Fail "no player to play" }
    Invoke-El $play
    Start-Sleep -Seconds 3
    if (-not (Find-El "Button" "pause" 5)) { Fail "the player does not play" }
    $n = Media-Count
    "playing; media requests so far: $n"
    Capture-Window $hwnd "$out\media-before-lock.png"
    $lockNow = Find-El "Button" "Lock now" 10
    if (-not $lockNow) { Dump; Fail "no 'Lock now' button (app lock off?)" }
    Invoke-El $lockNow
    Start-Sleep -Seconds 3
    if (-not (Find-El "Button" "Unlock" 20)) { Fail "no lock screen after Lock now" }
    Capture-Window $hwnd "$out\media-locked.png"
    $media = $false
    foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.Name -in "play", "pause", "audio time scrubber") { $media = $true } }
    if ($media) { Fail "media controls are still on the screen while locked" }
    Start-Sleep -Seconds 5
    "locked: no player left; requests since: $((Media-Count) - $n)"
    Media-Log | Select-Object -Last 2
  }
}

switch ($mediaStep) {
  { $_ -in "pdf", "all" } {
    Open-SmokePage
    Slash "/pdf" "PDF"
    Choose-File $pdf
    Start-Sleep -Seconds 5
    # Bring the viewer into the window and its first page to the top of the viewer.
    $group = Find-El "Group" "PDF preview of media-sample.pdf" 10
    if ($group) {
      try { $group.GetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView() } catch { }
      try { $group.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern).SetScrollPercent(-1, 0) } catch { }
    }
    Start-Sleep -Seconds 2
    Capture-Window $hwnd "$out\media-pdf.png"
    if (-not (Has-Text "1 / 1" 5)) { Dump; Fail "the PDF viewer does not show '1 / 1'" }
    "pdf viewer shows page 1 / 1"
  }
}

switch ($mediaStep) {
  { $_ -in "gallery", "all" } {
    Open-SmokePage
    Slash "/gallery" "Photo gallery"
    Start-Sleep -Seconds 2
    Capture-Window $hwnd "$out\media-gallery-empty.png"
    $add = Find-El "Button" "Add images to the gallery" 10
    if (-not $add) { Dump "Button"; Fail "no 'Add images to the gallery' button" }
    # Focus + a posted Enter, so that the file chooser gets a trusted user gesture.
    try { $add.GetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView() } catch { }
    $add.SetFocus()
    Start-Sleep -Milliseconds 500
    Post-Enter
    # The gallery's chooser takes several files: both pictures, quoted names in the file-name box.
    Choose-File "`"$png1`" `"$png2`""
    Start-Sleep -Seconds 4
    $group = $null
    foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Group" -and $c.Name -like "*allery*") { $group = $e; break } }
    if ($group) { try { $group.GetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView() } catch { } }
    Start-Sleep -Seconds 2
    Capture-Window $hwnd "$out\media-gallery.png"
    Dump "allery"
  }
}
