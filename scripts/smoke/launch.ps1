# Starts the desktop app in dev mode, waits for its window, dumps the UI Automation tree,
# captures the window and LEAVES THE APP RUNNING for drive.ps1. See README.md.
param([int]$WaitSeconds = 420)
$ErrorActionPreference = "Continue"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
. (Join-Path $repo "scripts\dev-env.ps1")
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug,kb_storage=debug"
$err = "$out\app.err.log"; $stdout = "$out\app.out.log"
if (Test-Path $err) { Clear-Content $err }; if (Test-Path $stdout) { Clear-Content $stdout }

$proc = Start-Process -FilePath "cmd.exe" -ArgumentList "/c", "pnpm", "tauri", "dev" `
  -WorkingDirectory (Join-Path $repo "apps\desktop") -PassThru -WindowStyle Hidden `
  -RedirectStandardError $err -RedirectStandardOutput $stdout
"launcher pid $($proc.Id)"

$app = $null
$deadline = (Get-Date).AddSeconds($WaitSeconds)
while ((Get-Date) -lt $deadline) {
  Start-Sleep -Seconds 3
  $app = Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if ($app) { break }
}
if (-not $app) { "no app window within $WaitSeconds s"; Get-Content $err -Tail 30; exit 1 }
"app window up: title='$($app.MainWindowTitle)' pid=$($app.Id)"
Start-Sleep -Seconds 6

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
"uia elements: $($all.Count)"
$i = 0
foreach ($e in $all) {
  $c = $e.Current
  if ($c.Name -or $c.AutomationId) { "{0,-22} name='{1}' id='{2}'" -f ($c.ControlType.ProgrammaticName -replace 'ControlType.', ''), $c.Name, $c.AutomationId }
  $i++; if ($i -gt 400) { "..."; break }
}

. "$PSScriptRoot\capture.ps1"
Capture-Window $app.MainWindowHandle "$out\launch.png"
"--- core log so far ---"
Get-Content $err | Select-String "atomik_kb_desktop_lib|kb_core|kb_storage|rusqlite_migration|panicked|ERROR" | Select-Object -Last 30
