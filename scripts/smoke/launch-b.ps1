# Starts a second desktop instance on its own data directory (debug builds honour
# KB_DATA_DIR) for the Milestone 6 two-device pass. launch.ps1 must be running first: the
# dev binary loads the UI from the Vite dev server on port 1420. Prints the new window's
# process id for `drive-m6.ps1 -AppPid`. See README.md.
#
#   .\launch-b.ps1            start (or reuse) the instance on out\b-data
#   .\launch-b.ps1 -Fresh     wipe out\b-data first (a wiped installation: onboarding)
#   .\launch-b.ps1 -Name c    another instance on out\c-data (logs out\c.err.log)
param([switch]$Fresh, [string]$Name = "b", [int]$WaitSeconds = 120)
$ErrorActionPreference = "Continue"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$data = Join-Path $out "$Name-data"
$exe = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
if (-not (Test-Path $exe)) { "no $exe (launch.ps1 builds it)"; exit 1 }
if (-not (Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue)) { "the Vite dev server is not listening on 1420 (launch.ps1 first)"; exit 1 }
if ($Fresh -and (Test-Path $data)) { Remove-Item -Recurse -Force $data; "wiped $data" }
New-Item -ItemType Directory -Force $data | Out-Null
$before = @(Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$env:KB_DATA_DIR = $data
$env:RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug,kb_net=debug,kb_storage=info"
$err = Join-Path $out "$Name.err.log"; $stdout = Join-Path $out "$Name.out.log"
if (Test-Path $err) { Clear-Content $err }
$proc = Start-Process -FilePath $exe -PassThru -WorkingDirectory (Join-Path $repo "apps\desktop\src-tauri") -RedirectStandardError $err -RedirectStandardOutput $stdout
Remove-Item Env:KB_DATA_DIR
"instance '$Name' pid $($proc.Id) (data: $data)"
$deadline = (Get-Date).AddSeconds($WaitSeconds)
$app = $null
while ((Get-Date) -lt $deadline) {
  Start-Sleep -Seconds 2
  $app = Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 -and $before -notcontains $_.Id } | Select-Object -First 1
  if ($app) { break }
}
if (-not $app) { "no new app window within $WaitSeconds s"; Get-Content $err -Tail 30; exit 1 }
"window up: pid=$($app.Id) title='$($app.MainWindowTitle)'"
Start-Sleep -Seconds 4
. "$PSScriptRoot\capture.ps1"
Capture-Window $app.MainWindowHandle "$out\m6-$Name-launched.png"
"APP_PID=$($app.Id)"
