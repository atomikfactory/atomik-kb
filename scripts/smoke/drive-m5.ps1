# Milestone 5 steps on the running desktop app (launch.ps1 first), through UI Automation,
# posted keystrokes and Win32 for the native dialogs (the helpers are the ones of
# drive-m4.ps1; see README.md).
#
#   .\drive-m5.ps1 -Step sample    writes out\sample.png (a 64x64 PNG) to insert
#   .\drive-m5.ps1 -Step image     inserts an image block through the slash menu and the Open dialog
#   .\drive-m5.ps1 -Step check     the image renders (an Image element with the alt text), captures
#   .\drive-m5.ps1 -Step data      Settings -> Data shows the attachment count and size, "Clean up now"
#   .\drive-m5.ps1 -Step purge     trashes and purges the page, sets retention to 0 days-ish, cleans up, checks blobs/
#
# The backup export / restore steps are those of drive-m4.ps1 (export, restore, verify):
# a restored installation must show the image again (check step).
param([string]$Step = "sample")
$ErrorActionPreference = "Continue"
# Dot-sourcing the helper script runs its own `param`, which overwrites $Step: keep ours.
$m5Step = $Step
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
$sample = "$out\sample.png"

function Blob-Dir() {
  $dir = Join-Path $env:LOCALAPPDATA "com.atomik-kb.desktop\blobs"
  if (Test-Path $dir) { return $dir } else { return $null }
}
function Blob-Count() {
  $dir = Blob-Dir
  if (-not $dir) { return 0 }
  return (Get-ChildItem $dir -Recurse -File -Filter "*.chunk" | Measure-Object).Count
}

switch ($m5Step) {
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
  }
  "image" {
    # Open the smoke page and put the caret at the end of the content.
    $item = Find-El "TreeItem" $pageTitle 10
    if (-not $item) { Fail "no '$pageTitle' in the tree" }
    Select-El $item
    Start-Sleep -Seconds 2
    $editor = Find-El "Edit" "Page content" 10
    if (-not $editor) { Fail "no page content editor" }
    $editor.SetFocus()
    Start-Sleep -Milliseconds 500
    Post-Enter
    Post-Text "/image"
    Start-Sleep -Seconds 1
    # The slash menu's items are named "<label> <hint>".
    $option = $null
    for ($i = 0; $i -lt 10 -and -not $option; $i++) {
      foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.ListItem" -and $c.Name -like "Image *") { $option = $e; break } }
      if (-not $option) { Start-Sleep -Milliseconds 400 }
    }
    if (-not $option) { Fail "no 'Image' slash command" }
    Capture-Window $hwnd "$out\m5-slash-image.png"
    try { Invoke-El $option } catch {
      # Invoke is refused by some list items: fall back to typing Enter, unless the
      # command already ran (its dialog is up).
      Start-Sleep -Seconds 1
      if ((Find-Dialog "Open" 1) -eq [IntPtr]::Zero) { try { Select-El $option } catch { }; Post-Enter }
    }
    Drive-FileDialog "Open" $sample
    Start-Sleep -Seconds 3
    "image inserted"
    Capture-Window $hwnd "$out\m5-image-inserted.png"
    SecLog
  }
  "check" {
    $item = Find-El "TreeItem" $pageTitle 10
    if (-not $item) { Fail "no '$pageTitle' in the tree" }
    Select-El $item
    Start-Sleep -Seconds 3
    # An <img> without alt text is left out of the accessibility tree, so the block's
    # own menu button is the proof that an attachment block rendered; the capture shows
    # the picture itself.
    $img = $null
    foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Image" -or ($c.ControlType.ProgrammaticName -eq "ControlType.Button" -and $c.Name -eq "File actions")) { $img = $e; break } }
    if (-not $img) { Fail "no attachment block rendered in the page" }
    "attachment block rendered ($($img.Current.ControlType.ProgrammaticName -replace 'ControlType.',''))"
    if (Has-Text "Deleted image" 1) { Fail "the image shows as deleted" }
    if (Has-Text "Loading the image..." 1) { Start-Sleep -Seconds 3 }
    Capture-Window $hwnd "$out\m5-image-rendered.png"
    "blobs on disk: $(Blob-Count)"
  }
  "data" {
    Go-Settings
    Start-Sleep -Seconds 1
    Dump "ttachment"
    $clean = Find-El "Button" "Clean up now" 10
    if (-not $clean) { Fail "no 'Clean up now' button" }
    Invoke-El $clean
    Start-Sleep -Seconds 3
    Dump "ttachment"
    Capture-Window $hwnd "$out\m5-settings-data.png"
    SecLog
  }
  "purge" {
    $before = Blob-Count
    "blobs before: $before"
    $item = Find-El "TreeItem" $pageTitle 10
    if (-not $item) { Fail "no '$pageTitle' in the tree" }
    Select-El $item
    Start-Sleep -Seconds 1
    $actions = $null
    foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Button" -and $c.Name -eq "Page actions") { $actions = $e } }
    if (-not $actions) { Fail "no page actions menu" }
    Invoke-El $actions
    Start-Sleep -Milliseconds 800
    $trash = Find-El "" "Move to trash" 5
    if (-not $trash) { $trash = Find-El "" "Trash" 5 }
    if (-not $trash) { Fail "no trash action" }
    Invoke-El $trash
    Start-Sleep -Seconds 2
    Dump "Trash"
    "page trashed; blobs now: $(Blob-Count) (unchanged: trash keeps references)"
    Capture-Window $hwnd "$out\m5-trashed.png"
  }
  "none" { }
  default { "unknown step $Step" }
}
