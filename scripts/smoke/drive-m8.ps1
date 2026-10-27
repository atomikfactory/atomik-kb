# Milestone 8 steps (attachment sync) on running desktop instances (launch.ps1 for A,
# launch-b.ps1 for B) against the local kb-server of server.ps1, through UI Automation,
# posted keystrokes and Win32 for the native dialogs (the helpers of drive-m4.ps1 through
# drive-m6.ps1; `-AppPid` picks the instance). See README.md.
#
#   .\drive-m8.ps1 -Step sample [-MB N]              writes out\sample.png (64x64) and, with -MB, out\big.bin of N MiB
#   .\drive-m8.ps1 -Step insert -Kind image|file [-Path P] -AppPid A
#                                                    inserts an image or file block in "Smoke page" (slash menu, Open dialog)
#   .\drive-m8.ps1 -Step check -Kind image|file -AppPid B [-WaitSeconds S] [-MinBlocks N]
#                                                    waits until the block shows complete (no transfer card), prints the states seen
#   .\drive-m8.ps1 -Step state -AppPid N             prints the attachment card texts (Downloading, Waiting, Not downloaded...)
#   .\drive-m8.ps1 -Step transfers -AppPid N         Settings -> Sync: the transfer line, if any
#   .\drive-m8.ps1 -Step policy -Value "Automatically"|"When opened" -AppPid N
#   .\drive-m8.ps1 -Step download -AppPid B          presses the card's Download button
#   .\drive-m8.ps1 -Step saveas -Path P -AppPid B [-Block N]   Save as... of the newest (or N-th) attachment block to P, prints the size
#   .\drive-m8.ps1 -Step remove -AppPid A [-Block N]            removes the newest (or N-th) attachment block from the page
#   .\drive-m8.ps1 -Step clean -AppPid A             Settings -> Attachments: retention shortest, "Clean up now"
#   .\drive-m8.ps1 -Step blobs -Data <dir>           counts the .chunk files under <dir>\blobs
#   .\drive-m8.ps1 -Step deletions                   counts DELETE /v2/feeds/.../chunks requests in out\server.log
param(
  [string]$Step = "sample",
  [int]$AppPid = 0,
  [string]$Kind = "image",
  [string]$Path = "",
  [string]$Value = "",
  [string]$Data = "",
  [int]$MB = 0,
  [int]$MinBlocks = 1,
  [int]$Block = 0,
  [int]$WaitSeconds = 600
)
$ErrorActionPreference = "Continue"
$m8Step = $Step
$m8Pid = $AppPid
$m8Kind = $Kind
$m8Path = $Path
$m8Value = $Value
# Dot-sourcing drive-m6 runs its own `param`, which resets $WaitSeconds: keep ours.
$m8Wait = $WaitSeconds
$m8MB = $MB
. "$PSScriptRoot\drive-m6.ps1" -Step "none" -AppPid $m8Pid 2>$null | Out-Null
$sample = "$out\sample.png"
$big = "$out\big.bin"
$tag = if ($m8Pid -ne 0) { "pid$m8Pid" } else { "a" }

function Card-Texts() {
  $texts = @()
  foreach ($e in (All-El (Web))) {
    $c = $e.Current
    if ($c.ControlType.ProgrammaticName -eq "ControlType.Text" -and ($c.Name -like "Downloading*" -or $c.Name -like "Waiting for*" -or $c.Name -like "*not downloaded*" -or $c.Name -like "Deleted *" -or $c.Name -like "*could not be read*")) { $texts += $c.Name }
  }
  return $texts
}
# The "File actions" button of an attachment block: the N-th on the page (1-based), the
# newest (last) when N is 0.
function FileActions-At([int]$n) {
  $buttons = @()
  foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Button" -and $c.Name -eq "File actions") { $buttons += $e } }
  if ($buttons.Count -eq 0) { return $null }
  if ($n -le 0 -or $n -gt $buttons.Count) { return $buttons[-1] }
  return $buttons[$n - 1]
}
# Presses a button whatever pattern WebView2 gave it: Invoke, the accessible default
# action, or a real click at its clickable point.
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Mouse {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  public static void Click(int x, int y) { SetCursorPos(x, y); System.Threading.Thread.Sleep(80); mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero); }
}
"@
function Press-Button($el) {
  try { Invoke-El $el; return "invoke" } catch { }
  try { $el.GetCurrentPattern([System.Windows.Automation.LegacyIAccessiblePattern]::Pattern).DoDefaultAction(); return "default action" } catch { }
  $pt = $el.GetClickablePoint()
  [Win]::ShowWindow($hwnd, 9) | Out-Null
  [Win]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 300
  [Mouse]::Click([int]$pt.X, [int]$pt.Y)
  return "click at $([int]$pt.X),$([int]$pt.Y)"
}
# A menu button (aria-haspopup) exposes ExpandCollapse, not Invoke, through WebView2.
function Open-Menu($el) {
  try { $el.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand(); return } catch { }
  try { Invoke-El $el; return } catch { }
  $el.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle()
}
function Attachment-Block() {
  foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Image" -or ($c.ControlType.ProgrammaticName -eq "ControlType.Button" -and $c.Name -eq "File actions")) { return $e } }
  return $null
}

switch ($m8Step) {
  "sample" {
    Add-Type -AssemblyName System.Drawing
    $bmp = New-Object System.Drawing.Bitmap 64, 64
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::FromArgb(255, 60, 110, 220))
    $g.FillEllipse([System.Drawing.Brushes]::Gold, 12, 12, 40, 40)
    $g.Dispose()
    $bmp.Save($sample, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    "sample written: $sample ($((Get-Item $sample).Length) bytes)"
    if ($m8MB -gt 0) {
      $rng = New-Object System.Random 42
      $block = New-Object byte[] (1MB)
      $stream = [IO.File]::Create($big)
      for ($i = 0; $i -lt $m8MB; $i++) { $rng.NextBytes($block); $stream.Write($block, 0, $block.Length) }
      $stream.Close()
      "big file written: $big ($((Get-Item $big).Length) bytes)"
    }
  }
  "insert" {
    $file = if ($m8Path -ne "") { $m8Path } elseif ($m8Kind -eq "image") { $sample } else { $big }
    if (-not (Test-Path $file)) { Fail "no $file (sample step first)" }
    Show-Page
    $editor = Find-El "Edit" "Page content" 10
    if (-not $editor) { Fail "no page content editor" }
    $editor.SetFocus()
    Start-Sleep -Milliseconds 500
    Post-Enter
    Post-Text "/$m8Kind"
    Start-Sleep -Seconds 1
    $label = if ($m8Kind -eq "image") { "Image *" } else { "File *" }
    $option = $null
    for ($i = 0; $i -lt 10 -and -not $option; $i++) {
      foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.ListItem" -and $c.Name -like $label) { $option = $e; break } }
      if (-not $option) { Start-Sleep -Milliseconds 400 }
    }
    if (-not $option) { Fail "no '$m8Kind' slash command" }
    try { Invoke-El $option } catch {
      Start-Sleep -Seconds 1
      if ((Find-Dialog "Open" 1) -eq [IntPtr]::Zero) { try { Select-El $option } catch { }; Post-Enter }
    }
    Drive-FileDialog "Open" $file
    # A large file takes a while to seal: wait for the upload text to go away.
    $deadline = (Get-Date).AddSeconds($m8Wait)
    while ((Get-Date) -lt $deadline -and (Find-Like "Uploading*" 1)) { Start-Sleep -Seconds 2 }
    Start-Sleep -Seconds 2
    "$m8Kind inserted from $file"
    Capture-Window $hwnd "$out\m8-$tag-inserted-$m8Kind.png"
  }
  "state" {
    Show-Page
    Start-Sleep -Seconds 1
    $texts = Card-Texts
    if ($texts.Count -eq 0) { "no transfer card (complete, or no block)" } else { $texts | ForEach-Object { "  '$_'" } }
    Capture-Window $hwnd "$out\m8-$tag-state.png"
  }
  "check" {
    $deadline = (Get-Date).AddSeconds($m8Wait)
    $seen = @{}
    $done = $false
    while ((Get-Date) -lt $deadline) {
      Show-Page
      $block = Attachment-Block
      $texts = Card-Texts
      foreach ($t in $texts) { $seen[($t -replace '\d+', 'n')] = $true }
      $blocks = 0; foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Button" -and $c.Name -eq "File actions") { $blocks++ } }
      if ($block -and $texts.Count -eq 0 -and $blocks -ge $MinBlocks) { $done = $true; break }
      Start-Sleep -Seconds 5
    }
    if (-not $done) { Capture-Window $hwnd "$out\m8-$tag-notcomplete-$m8Kind.png"; Fail "the $m8Kind block did not complete within $m8Wait s (seen: $($seen.Keys -join ' | '))" }
    "$m8Kind complete (states seen: $($seen.Keys -join ' | '))"
    Capture-Window $hwnd "$out\m8-$tag-complete-$m8Kind.png"
  }
  "transfers" {
    Go-Settings
    Start-Sleep -Seconds 1
    $line = $null
    foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Text" -and ($c.Name -like "*to upload*" -or $c.Name -like "*to download*" -or $c.Name -like "*waiting for another*")) { $line = $c.Name } }
    if ($line) { "transfers: $line" } else { "transfers: none shown" }
  }
  "policy" {
    if ($m8Value -eq "") { Fail "-Value is required" }
    Go-Settings
    Start-Sleep -Seconds 1
    $combo = Find-El "ComboBox" "Download attachments" 10
    if ($combo) {
      # ValuePattern.SetValue on a <select> changes the value without a change event
      # (React never sees it): expand the list and select the option instead.
      try { $combo.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand() } catch { }
      Start-Sleep -Milliseconds 500
      $item = $null
      foreach ($e in (All-El $combo)) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.ListItem" -and $c.Name -eq $m8Value) { $item = $e } }
      if (-not $item) { $item = Find-El "ListItem" $m8Value 5 }
      if (-not $item) { Fail "no '$m8Value' option" }
      Select-El $item
      try { $combo.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Collapse() } catch { }
    } else {
      $radio = Find-El "RadioButton" $m8Value 5
      if (-not $radio) { Fail "no 'Download attachments' control (combo or radio '$m8Value')" }
      Select-El $radio
    }
    Start-Sleep -Seconds 1
    "policy set: $m8Value"
    Capture-Window $hwnd "$out\m8-$tag-policy.png"
  }
  "download" {
    Show-Page
    $button = Find-El "Button" "Download" 10
    if (-not $button) { Fail "no Download button on the card" }
    $how = Press-Button $button
    Start-Sleep -Seconds 2
    "download requested ($how)"
  }
  "saveas" {
    if ($m8Path -eq "") { Fail "-Path is required" }
    Show-Page
    $actions = FileActions-At $Block
    if (-not $actions) { Fail "no 'File actions' menu" }
    Open-Menu $actions
    Start-Sleep -Milliseconds 800
    $save = Find-El "" "Save as..." 5
    if (-not $save) { Fail "no 'Save as...' action" }
    Invoke-El $save
    Drive-FileDialog "Save As" $m8Path
    # The shell decrypts the whole file before the final rename: wait for it, then for
    # its size to settle.
    $deadline = (Get-Date).AddSeconds($m8Wait)
    while ((Get-Date) -lt $deadline -and -not (Test-Path $m8Path)) { Start-Sleep -Seconds 2 }
    if (-not (Test-Path $m8Path)) { Fail "nothing saved at $m8Path within $m8Wait s" }
    $size = -1
    while ((Get-Date) -lt $deadline) { $now = (Get-Item $m8Path).Length; if ($now -eq $size) { break }; $size = $now; Start-Sleep -Seconds 2 }
    "saved: $m8Path ($size bytes)"
  }
  "remove" {
    Show-Page
    $actions = FileActions-At $Block
    if (-not $actions) { Fail "no 'File actions' menu" }
    Open-Menu $actions
    Start-Sleep -Milliseconds 800
    $remove = Find-El "" "Remove" 5
    if (-not $remove) { Fail "no 'Remove' action" }
    Invoke-El $remove
    Start-Sleep -Seconds 2
    "block removed"
    Capture-Window $hwnd "$out\m8-$tag-removed.png"
  }
  "clean" {
    Go-Settings
    Start-Sleep -Seconds 1
    $combo = Find-El "ComboBox" "Keep files no page uses for" 10
    if ($combo) {
      # The shortest period offered.
      try { $combo.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand() } catch { }
      Start-Sleep -Milliseconds 500
      $options = @(); foreach ($e in (All-El $combo)) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.ListItem") { $options += $e } }
      "retention options: $(($options | ForEach-Object { $_.Current.Name }) -join ', ')"
      $shortest = $options | Where-Object { $_.Current.Name -match '^0 ' } | Select-Object -First 1
      if (-not $shortest) { $shortest = $options | Where-Object { $_.Current.Name -match '^\d' } | Sort-Object { [int]($_.Current.Name -replace '\D', '') } | Select-Object -First 1 }
      if ($shortest) { Select-El $shortest; "retention set: $($shortest.Current.Name)" }
      Start-Sleep -Seconds 1
    }
    $clean = Find-El "Button" "Clean up now" 10
    if (-not $clean) { Fail "no 'Clean up now' button" }
    Invoke-El $clean
    Start-Sleep -Seconds 3
    Dump "ttachment"
    Capture-Window $hwnd "$out\m8-$tag-clean.png"
  }
  "blobs" {
    $dir = if ($Data -ne "") { Join-Path $Data "blobs" } else { Join-Path $env:LOCALAPPDATA "com.atomik-kb.desktop\blobs" }
    if (-not (Test-Path $dir)) { "blobs: 0 (no $dir)" } else { "blobs: $((Get-ChildItem $dir -Recurse -File -Filter '*.chunk' | Measure-Object).Count) under $dir" }
  }
  "deletions" {
    $log = "$out\server.log"
    if (-not (Test-Path $log)) { Fail "no $log" }
    "chunk deletions received by the server: $((Select-String -Path $log -Pattern 'method=DELETE path=/v2/feeds/[0-9a-f.]+/chunks/' | Measure-Object).Count)"
  }
  "none" { }
  default { "unknown step $m8Step" }
}
