# Comments, mentions, the Person property and notifications (Milestone 13, slice 2) on the BUILT desktop binary: a local kb-server
# (protocol v2) and four app instances, each on its own data directory with its own WebView2 profile and DevTools port: A the Owner,
# B an Editor, C a Commenter (all three invited by a code) and E, a second device of A's account made by a kit recovery (the first read
# of a space must raise no notification there). The real core, shell, sync driver and WebView2 under the production CSP.
#
#   pnpm --dir apps/desktop tauri build --debug --no-bundle      (once; a debug build keeps KB_DATA_DIR)
#   cargo build -p kb-server -j 4                                 (once)
#   .\drive-m13-comments.ps1 -Step setup       a fresh server (own port, own data), A, B and C on fresh data (onboarded by UI Automation), A
#                                              connects and shares a space, invites B as Editor and C as Commenter by code
#   .\drive-m13-comments.ps1 -Step comments    A selects text and comments (the highlight), B replies (A: inbox count, toast, OS notification),
#                                              C comments but cannot edit the page, B cannot edit or delete A's comment (the UI hides the
#                                              actions, forced requests answer Forbidden), A deletes B's reply, B resolves, Show resolved,
#                                              the anchors survive an edit of the text around them
#   .\drive-m13-comments.ps1 -Step mentions    A types @ and picks B (B: a notification that opens the page at the block), @ in a comment
#                                              notifies C, nothing arrives for one's own changes, E (A's second device, a kit recovery) reads
#                                              the whole history without a single notification and then gets a new one
#   .\drive-m13-comments.ps1 -Step person      A adds a Person property and assigns B (B: a notification), the "me" filter of a table view on B
#   .\drive-m13-comments.ps1 -Step reminders   A sets a date with "Remind: B": only B's devices fire it
#   .\drive-m13-comments.ps1 -Step search      a word of a comment is found "in comment"; the page's export has @label and no comment
#   .\drive-m13-comments.ps1 -Step languages   B in French: the notification texts and the inbox
#   .\drive-m13-comments.ps1 -Step restart     every instance started again: comments, read state, Person property and mentions survive
#   .\drive-m13-comments.ps1 -Step former      A removes B: B's comments, mentions and Person value read "Former member (name)" on A and C
#   .\drive-m13-comments.ps1 -Step logs        the logs of the server and of every instance: no panic, no ERROR, no comment text, name, page
#                                              text, link or secret, and no id longer than 8 hex digits
#   .\drive-m13-comments.ps1 -Step verify      the summary of every phase (out\m13-comments-report.json)
#   .\drive-m13-comments.ps1 -Step stop        stops the instances and the server this pass started (never node)
#   .\drive-m13-comments.ps1 -Step all         every step above in order, then stop
#
# Like drive-m13-share.ps1 each instance is started with --remote-debugging-port, and drive-m13-comments.mjs sends trusted mouse and
# key events to the pages over the DevTools ports, reads the DOM and the core through the pages' own IPC, and records every request,
# console message and content-security-policy violation per instance: a phase fails on a request to another host, a violation, a
# failed request, a console error or a replaced document. UI Automation drives the onboarding; the native Open dialog, the captures,
# launching and killing are the helper steps of drive-m13-share.ps1. The OS notifications the shell shows are read from the Windows
# notification database (wpndatabase.db) by a watcher process that `setup` starts (node, `drive-m13-comments.mjs os-watch`, polling every
# 250 ms: the system removes some toasts again within seconds) and `stop` ends. Only the processes this pass started are stopped, by
# process id (another worktree or session may run the app or a kb-server: server.ps1 stops only the binary of its own checkout).
#
# The server listens on its own port (-ServerPort, 8797) and the DevTools ports start at -BasePort (9433): another pass of the repository
# may use 8787 and 9333. Output lands in out\ (git-ignored): m13-comments-*.png, m13-comments-*.json, m13-comments-<name>[-rN].err.log,
# m13-comments-server*.log. A failing phase does not stop the others; the exit code is 1 when any failed. If the Windows session is locked,
# what needs real keys is listed as NOT VERIFIABLE, never as passed.
param([string]$Step = "all", [int]$BasePort = 9433, [int]$ServerPort = 8797)
$ErrorActionPreference = "Continue"
$m13Step = $Step
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$failedPhases = @()

# ---- the pass --------------------------------------------------------------------------------------------------------------
$env:M13C_OUT_DIR = $out
$env:M13C_SHARE_PS1 = Join-Path $PSScriptRoot "drive-m13-share.ps1"
$env:M13C_BASE_PORT = "$BasePort"
$env:M13C_SERVER_URL = "http://127.0.0.1:$ServerPort"
$env:M13C_REPO = $repo

function Run-Mjs([string]$phase) {
  $report = if ($phase -eq "verify") { "$out\m13-comments-report.json" } else { "$out\m13-comments-$phase.json" }
  $lines = & node "$PSScriptRoot\drive-m13-comments.mjs" $phase $report 2>&1
  $lines | ForEach-Object { "$_" }
  if ($LASTEXITCODE -ne 0) {
    $script:failedPhases += $phase
    "PHASE FAILED: $phase (see $report)"
  }
}

function Stop-Pass() {
  # The instances this pass started (their ids are in the state file) and the server of this checkout.
  & node "$PSScriptRoot\drive-m13-comments.mjs" "stop" "$out\m13-comments-stop.json" 2>&1 | ForEach-Object { "$_" }
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
    & node "$PSScriptRoot\drive-m13-comments.mjs" "stop" "$out\m13-comments-stop.json" 2>&1 | ForEach-Object { "$_" }
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
  & "$PSScriptRoot\server.ps1" -Action reset -Listen "127.0.0.1:$ServerPort" | ForEach-Object { "$_" }
  # A fresh pass: the reports, captures, exports and logs of an earlier one go (the server's log is out\server.log, which server.ps1 starts anew).
  Get-ChildItem $out -Filter "m13-comments*" -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

$phases = @("setup", "comments", "mentions", "person", "reminders", "search", "languages", "restart", "former")
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
"m13 comments pass: all phases ok"
