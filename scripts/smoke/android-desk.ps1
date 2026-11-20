# Host-side stand-in for the desktop in the Android sync smoke (crates/kb-mobile/examples/desk.rs):
# a KbEngine on out\desk-data that takes commands from out\desk-cmd. Needs kb-server running (server.ps1).
#
#   .\android-desk.ps1 -Step start             build + start the desk process (background, hidden)
#   .\android-desk.ps1 -Step cmd -Line '<verb args>'   send one command, print the answer
#   .\android-desk.ps1 -Step stop              quit it
#
# Verbs: raw <Request json>, state, events, append <pageId> <text>, quit.
param([string]$Step = "start", [string]$Line = "")
$ErrorActionPreference = "Continue"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
$cmds = Join-Path $out "desk-cmd"
$data = Join-Path $out "desk-data"
$exe = Join-Path $repo "target\debug\examples\desk.exe"

switch ($Step) {
  "start" {
    if (-not (Test-Path $exe)) { "no $exe (cargo build -p kb-mobile --example desk)"; exit 1 }
    New-Item -ItemType Directory -Force $cmds | Out-Null
    $proc = Start-Process -FilePath $exe -ArgumentList "`"$data`"", "`"$cmds`"" -PassThru -WindowStyle Hidden -RedirectStandardOutput "$out\desk.out.log" -RedirectStandardError "$out\desk.err.log"
    "desk pid $($proc.Id)"
  }
  "cmd" {
    $n = (Get-Date).ToString("HHmmssfff")
    $file = Join-Path $cmds "$n.cmd"
    [System.IO.File]::WriteAllText("$file.tmp", $Line, (New-Object System.Text.UTF8Encoding($false)))
    Move-Item "$file.tmp" $file
    $answer = Join-Path $cmds "$n.out"
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and -not (Test-Path $answer)) { Start-Sleep -Milliseconds 300 }
    if (Test-Path $answer) { Start-Sleep -Milliseconds 100; Get-Content $answer -Raw } else { "no answer" }
  }
  "stop" {
    & $PSCommandPath -Step cmd -Line "quit" | Out-Null
    Get-Process -Name "desk" -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq $exe } | Stop-Process -Force
    "stopped"
  }
}
