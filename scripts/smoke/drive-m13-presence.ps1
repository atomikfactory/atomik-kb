# Realtime and presence (Milestone 13, slice 3) on the BUILT desktop binary: a local kb-server (protocol v2, the WebSocket v2 relay) and
# three app instances, each on its own data directory with its own WebView2 profile and DevTools port: A the Owner, B an Editor and C a
# Viewer (both invited by a code), later D, a second device of B's account (paired, then revoked). The real core, shell, sync driver,
# realtime socket and WebView2 under the production CSP.
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   cargo build -p kb-server -j 4                                 (once)
#   .\drive-m13-presence.ps1 -Step setup      a fresh server (own port, own data), A, B and C on fresh data (onboarded by UI Automation), A shares
#                                             a space and invites B as Editor and C as Viewer by code
#   .\drive-m13-presence.ps1 -Step realtime   A and B open one page; a token typed in A is in B in under a second, and the other way round, 20 times
#                                             each (median and 95th percentile, measured over the DevTools ports with the pages' own clocks); no pull
#                                             of the receiving device happens in between (the server's request log and the device's own log)
#   .\drive-m13-presence.ps1 -Step presence   A, B and C (a Viewer too) on one page: avatars (header and space switcher), carets and selections with the
#                                             name and the colour of join order, a moved selection is followed, A leaves the page and goes
#   .\drive-m13-presence.ps1 -Step removal    A removes C: its presence goes from A and B at once and it receives no more entries; a second device of B's
#                                             account is paired, shown on the page, and revoked: its socket is closed, it cannot come back, its presence goes
#   .\drive-m13-presence.ps1 -Step reconnect  the server is stopped and started while A and B have the page open: both reconnect and resubscribe, the
#                                             edits made meanwhile arrive once, realtime and presence are back
#   .\drive-m13-presence.ps1 -Step restart    A is killed (its presence times out), started again (B accepts its frames), then every instance is started
#                                             again: presence comes back and nothing stale remains
#   .\drive-m13-presence.ps1 -Step logs       the logs of the server and of every instance: no panic, no ERROR, no presence content, page text, name or
#                                             secret, and no id longer than 8 hex digits
#   .\drive-m13-presence.ps1 -Step verify     the summary of every phase (out\m13-presence-report.json)
#   .\drive-m13-presence.ps1 -Step stop       stops the instances and the server this pass started (never node)
#   .\drive-m13-presence.ps1 -Step all        every step above in order, then stop
#
# Like drive-m13-comments.ps1 each instance is started with --remote-debugging-port, and drive-m13-presence.mjs sends trusted mouse and
# key events to the pages over the DevTools ports, reads the DOM and the core through the pages' own IPC, and records every request,
# console message and content-security-policy violation per instance: a phase fails on a request to another host, a violation, a failed
# request, a console error or a replaced document. UI Automation drives the onboarding; the native dialogs, the captures, launching and
# killing are the helper steps of drive-m13-share.ps1. Only the processes this pass started are stopped, by process id (another worktree
# or session may run the app or a kb-server: server.ps1 stops only the binary of its own checkout).
#
# The server listens on its own port (-ServerPort, 8807) and the DevTools ports start at -BasePort (9533): another pass of the repository
# may use 8787, 8797 and 9333 or 9433. Output lands in out\ (git-ignored): m13-presence-*.png, m13-presence-*.json,
# m13-presence-<name>[-rN].err.log, m13-presence-server*.log. A failing phase does not stop the others; the exit code is 1 when any
# failed. If the Windows session is locked, what needs real keys is listed as NOT VERIFIABLE, never as passed.
param([string]$Step = "all", [int]$BasePort = 9533, [int]$ServerPort = 8807)
$ErrorActionPreference = "Continue"
$m13Step = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$failedPhases = @()

# ---- the pass --------------------------------------------------------------------------------------------------------------
$env:M13P_OUT_DIR = $out
$env:M13P_SHARE_PS1 = Join-Path $PSScriptRoot "drive-m13-share.ps1"
$env:M13P_BASE_PORT = "$BasePort"
$env:M13P_SERVER_URL = "http://127.0.0.1:$ServerPort"
$env:M13P_REPO = $repo

function Run-Mjs([string]$phase) {
  $report = if ($phase -eq "verify") { "$out\m13-presence-report.json" } else { "$out\m13-presence-$phase.json" }
  $lines = & node "$PSScriptRoot\drive-m13-presence.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $phase
    "PHASE FAILED: $phase (see $report)"
  }
}

function Stop-Pass() {
  # The instances this pass started (their ids are in the state file) and the server of this checkout.
  & node "$PSScriptRoot\drive-m13-presence.mjs" "stop" "$out\m13-presence-stop.json" 2>&1 | ForEach-Object { "$_" }
  & "$PSScriptRoot\server.ps1" -Action stop -Listen "127.0.0.1:$ServerPort" | ForEach-Object { "$_" }
}

if ($m13Step -eq "stop") { Stop-Pass; exit 0 }
if ($m13Step -in "verify", "logs") {
  Run-Mjs $m13Step
  if ($failedPhases.Count -gt 0) { exit 1 }
  exit 0
}

if ($m13Step -in "setup", "all") {
  # The server starts fresh (a new epoch, no accounts) on its own port; the ports of the instances must be free.
  . (Join-Path $repo "scripts\dev-env.ps1")
  $busy = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalPort -ge $BasePort -and $_.LocalPort -lt ($BasePort + 6) })
  if ($busy.Count -gt 0) {
    # Instances of an earlier pass of this script (their ids are in the state file) are stopped first.
    & node "$PSScriptRoot\drive-m13-presence.mjs" "stop" "$out\m13-presence-stop.json" 2>&1 | ForEach-Object { "$_" }
    Start-Sleep -Seconds 3
    $busy = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalPort -ge $BasePort -and $_.LocalPort -lt ($BasePort + 6) })
  }
  if ($busy.Count -gt 0) {
    # An instance of this checkout's binary that an interrupted pass left (its state file lost the id): the port's owner is the
    # webview of an atomik-kb-desktop.exe built here. Only that binary is stopped, never another checkout's or another program.
    $ours = Join-Path $repo "target\debug\atomik-kb-desktop.exe"
    foreach ($b in $busy) {
      $owner = Get-CimInstance Win32_Process -Filter "ProcessId=$($b.OwningProcess)" -ErrorAction SilentlyContinue
      $parent = if ($owner) { Get-CimInstance Win32_Process -Filter "ProcessId=$($owner.ParentProcessId)" -ErrorAction SilentlyContinue } else { $null }
      if ($parent -and $parent.ExecutablePath -ieq $ours) { "stopping a left-over instance of this checkout: $($parent.ProcessId)"; Stop-Process -Id $parent.ProcessId -Force -ErrorAction SilentlyContinue }
    }
    Start-Sleep -Seconds 3
    $busy = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalPort -ge $BasePort -and $_.LocalPort -lt ($BasePort + 6) })
  }
  if ($busy.Count -gt 0) { "FAIL: the DevTools ports $BasePort-$($BasePort + 5) are in use by process $($busy[0].OwningProcess): pass -BasePort"; exit 1 }
  $serverBusy = @(Get-NetTCPConnection -State Listen -LocalPort $ServerPort -ErrorAction SilentlyContinue)
  if ($serverBusy.Count -gt 0) {
    # Ours (the binary of this checkout) is replaced by the reset; anything else is somebody else's: another port.
    $owner = Get-Process -Id $serverBusy[0].OwningProcess -ErrorAction SilentlyContinue
    $ours = Join-Path $repo "target\debug\kb-server.exe"
    if (-not $owner -or $owner.Path -ine $ours) { "FAIL: port $ServerPort is in use by process $($serverBusy[0].OwningProcess) ($($owner.ProcessName)): pass -ServerPort"; exit 1 }
  }
  # A fresh pass: the reports, captures and logs of an earlier one go (the server's log is out\server.log, which server.ps1 starts anew).
  Get-ChildItem $out -Filter "m13-presence*" -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
  & "$PSScriptRoot\server.ps1" -Action reset -Listen "127.0.0.1:$ServerPort" | ForEach-Object { "$_" }
}

$phases = @("setup", "realtime", "presence", "removal", "reconnect", "restart")
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
"m13 presence pass: all phases ok"
