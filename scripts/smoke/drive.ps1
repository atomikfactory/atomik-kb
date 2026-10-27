# Drives the running desktop app through UI Automation (the WebView2 exposes the DOM as UIA
# elements) and SendKeys, capturing after each step into out/. Steps are examples from the
# Milestone 3 pass; adapt them. Usage: .\drive.ps1 -Step onboard ; -Step page ; -Step type ; -Step search
param([string]$Step = "page")
$ErrorActionPreference = "Continue"
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
. "$PSScriptRoot\capture.ps1"
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null

$app = Get-Process -Name "atomik-kb-desktop" | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $app) { "app is not running (launch.ps1 first)"; exit 1 }
$hwnd = $app.MainWindowHandle
$root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
$errLog = "$out\app.err.log"

function Web() {
  $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, "atomik-kb - Web content")
  return $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}
function Find-El([string]$type, [string]$name, [int]$retries = 20) {
  for ($i = 0; $i -lt $retries; $i++) {
    $all = (Web).FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($e in $all) {
      $c = $e.Current
      if (($type -eq "" -or $c.ControlType.ProgrammaticName -eq "ControlType.$type") -and $c.Name -eq $name) { return $e }
    }
    Start-Sleep -Milliseconds 400
  }
  return $null
}
function Invoke-El($el) { $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() }
function Select-El($el) { $el.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select() }
function Set-El($el, [string]$text) { $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($text) }
# Keystrokes go through WScript.Shell: the .NET SendKeys journal hook is refused from a
# tool shell ("Access is denied").
$wsh = New-Object -ComObject WScript.Shell
function Keys([string]$keys) {
  [Win]::ShowWindow($hwnd, 9) | Out-Null
  [Win]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 300
  $wsh.SendKeys($keys)
}
function Dump([string]$filter = "") {
  $all = (Web).FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  foreach ($e in $all) { $c = $e.Current; if ($c.Name -and ($filter -eq "" -or $c.Name -like "*$filter*" -or $c.ControlType.ProgrammaticName -like "*$filter*")) { "  {0,-22} '{1}' id='{2}'" -f ($c.ControlType.ProgrammaticName -replace 'ControlType.',''), $c.Name, $c.AutomationId } }
}
function DocLog() { Get-Content $errLog | Select-String '"doc"|doc_|dispatch kind=|panicked|ERROR|WARN' | Select-Object -Last 25 }

switch ($Step) {
  "onboard" {
    $edit = Find-El "Edit" "Workspace name"
    if (-not $edit) { "no workspace name field"; Dump; exit 1 }
    Set-El $edit "Smoke workspace"
    Start-Sleep -Milliseconds 300
    Invoke-El (Find-El "Button" "Create workspace")
    Start-Sleep -Seconds 3
    Capture-Window $hwnd "$out\onboard.png"
    Dump "Button"
  }
  "page" {
    $newPage = Find-El "Button" "New page"
    if (-not $newPage) { "no New page button"; Dump; exit 1 }
    Invoke-El $newPage
    Start-Sleep -Seconds 2
    Keys "Smoke page{ENTER}"
    Start-Sleep -Seconds 2
    Capture-Window $hwnd "$out\page.png"
  }
  "type" {
    $editor = Find-El "" "Page content" 10
    if ($editor) { $editor.SetFocus() } else { "editor not found"; Dump; exit 1 }
    Start-Sleep -Milliseconds 500
    Keys "Hello from the smoke test.{ENTER}/head"
    Start-Sleep -Seconds 1
    Capture-Window $hwnd "$out\slash-menu.png"
    Keys "{ENTER}Section title{ENTER}"
    Start-Sleep -Seconds 2
    Capture-Window $hwnd "$out\typed.png"
    DocLog
  }
  "search" {
    Keys "^k"
    Start-Sleep -Milliseconds 800
    Keys "smoke"
    Start-Sleep -Seconds 1
    Capture-Window $hwnd "$out\search.png"
    Keys "{ESC}"
    DocLog
  }
  "open" {
    # Reopens the smoke page after a relaunch (persistence check).
    $item = Find-El "TreeItem" "Smoke page" 10
    if (-not $item) { "no Smoke page in the tree"; Dump; exit 1 }
    Select-El $item
    Start-Sleep -Seconds 3
    Capture-Window $hwnd "$out\reopened.png"
    DocLog
  }
  default { "unknown step $Step" }
}
