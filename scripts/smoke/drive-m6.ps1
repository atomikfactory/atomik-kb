# Milestone 6 steps on running desktop instances (launch.ps1 for A, launch-b.ps1 for B)
# against the local kb-server of server.ps1, through UI Automation and posted keystrokes
# (the helpers of drive-m4.ps1; see README.md). `-AppPid` picks the instance.
#
#   .\drive-m6.ps1 -Step connect [-AppPid N]        Settings -> Sync: server address, Connect; the invite
#                                                   token step when the server does not know the account
#   .\drive-m6.ps1 -Step token [-AppPid N]          enter the invite token (after a server data loss)
#   .\drive-m6.ps1 -Step status [-AppPid N]         print the status line and the sidebar indicator
#   .\drive-m6.ps1 -Step type -Text "..." [-AppPid N]   type a line into "Smoke page"
#   .\drive-m6.ps1 -Step check -Text "..." [-AppPid N]  wait until "Smoke page" shows that line (synced in)
#   .\drive-m6.ps1 -Step disconnect [-AppPid N]     Disconnect... and confirm
#
# The restore of B uses `drive-m4.ps1 -Step restore` with the backup of A (export step) and
# the kit words of out\kit.txt; those steps take `-AppPid` too when dot-sourced from here.
param(
  [string]$Step = "connect",
  [int]$AppPid = 0,
  [string]$Text = "",
  [string]$Url = "http://127.0.0.1:8787",
  [string]$Token = "dev",
  [int]$WaitSeconds = 60
)
$ErrorActionPreference = "Continue"
# Dot-sourcing the helper script runs its own `param`, which overwrites $Step: keep ours.
$m6Step = $Step
$m6Pid = $AppPid
. "$PSScriptRoot\drive-m4.ps1" -Step "none" 2>$null | Out-Null
if ($m6Pid -ne 0) {
  $app = Get-Process -Id $m6Pid -ErrorAction SilentlyContinue
  if (-not $app -or $app.MainWindowHandle -eq 0) { "no app window for pid $m6Pid"; exit 1 }
  $hwnd = $app.MainWindowHandle
  $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
  foreach ($name in @("b", "c", "d")) {
    $candidate = "$out\$name.err.log"
    if ((Test-Path $candidate) -and (Get-Content $candidate -Raw -ErrorAction SilentlyContinue) -match "data directory taken from KB_DATA_DIR") { $errLog = $candidate }
  }
}
$tag = if ($m6Pid -ne 0) { "pid$m6Pid" } else { "a" }

function Find-Like([string]$pattern, [int]$retries = 20, [string]$type = "") {
  for ($i = 0; $i -lt $retries; $i++) {
    $web = Web
    if ($web) {
      foreach ($e in (All-El $web)) {
        $c = $e.Current
        if (($type -eq "" -or $c.ControlType.ProgrammaticName -eq "ControlType.$type") -and $c.Name -like $pattern) { return $e }
      }
    }
    Start-Sleep -Milliseconds 500
  }
  return $null
}
function SyncLog() { Get-Content $errLog -ErrorAction SilentlyContinue | Select-String 'sync|kb_net|driver|cycle|panicked|ERROR|WARN' | Select-Object -Last 25 }
function Indicator() { $b = Find-Like "Sync: *" 2 "Button"; if ($b) { return $b.Current.Name } else { return "(no indicator)" } }
function Status-Line() {
  foreach ($p in @("Connected*", "Uploading*", "Downloading*", "Offline*", "Error*", "Paused*", "Off*")) {
    $t = Find-Like $p 1 "Text"
    if ($t) { return $t.Current.Name }
  }
  return "(no status line)"
}
function Wait-Connected([int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    if (Find-Like "Connected*" 1 "Text") { return $true }
    if (Find-El "Edit" "Invite token" 1) { return "token" }
    Start-Sleep -Seconds 1
  }
  return $false
}
# Brings the page editor of "Smoke page" to the front: selecting an already selected tree
# item is a no-op (Settings would stay up), so Home is invoked first.
function Show-Page() {
  if (-not (Find-El "Edit" "Page content" 1)) {
    # ($home is PowerShell's profile path: another name.)
    $homeButton = Find-El "Button" "Home" 2
    if ($homeButton) { Invoke-El $homeButton; Start-Sleep -Milliseconds 800 }
  }
  $item = Find-El "TreeItem" $pageTitle 5
  if (-not $item) { Fail "no '$pageTitle' in the tree" }
  if (-not (Find-El "Edit" "Page content" 1)) {
    try { Invoke-El $item } catch { Select-El $item }
    Start-Sleep -Seconds 2
  }
}
# Fills the invite token when the step is (still) up; answers $false when the step went
# away meanwhile (the driver's cycle connected after all).
function Enter-Token() {
  $field = Find-El "Edit" "Invite token" 10
  if (-not $field) { "the token step is gone"; return $false }
  Type-Into $field $Token
  Invoke-El (Find-El "Button" "Connect" 10)
  "token entered"
  return $true
}

switch ($m6Step) {
  "connect" {
    Go-Settings
    $field = Find-El "Edit" "Server address" 10
    if (-not $field) {
      if (Find-Like "Connected*" 2 "Text") { "already connected: $(Status-Line)"; break }
      Fail "no 'Server address' field"
    }
    Type-Into $field $Url
    Invoke-El (Find-El "Button" "Connect" 10)
    Start-Sleep -Seconds 2
    Capture-Window $hwnd "$out\m6-$tag-connecting.png"
    $state = Wait-Connected $WaitSeconds
    if ($state -eq "token" -and (Find-Like "Connected*" 2 "Text")) { $state = $true }
    if ($state -eq "token") {
      "the server does not know the account: token step"
      Capture-Window $hwnd "$out\m6-$tag-token.png"
      Enter-Token | Out-Null
      $state = Wait-Connected $WaitSeconds
      if ($state -eq "token") { $state = Wait-Connected $WaitSeconds }
    }
    if ($state -ne $true) { Fail "not connected within $WaitSeconds s (status: $(Status-Line))" }
    "connected: $(Status-Line) | indicator: $(Indicator)"
    Capture-Window $hwnd "$out\m6-$tag-connected.png"
    SyncLog
  }
  "token" {
    # After a server data loss the driver notices on its next cycle (periodic pull every
    # 60 s): wait up to -WaitSeconds for the token step to appear.
    Go-Settings
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $field = $null
    while ((Get-Date) -lt $deadline -and -not $field) { $field = Find-El "Edit" "Invite token" 2 }
    if (-not $field) { Fail "no invite token step within $WaitSeconds s (status: $(Status-Line))" }
    Capture-Window $hwnd "$out\m6-$tag-token-again.png"
    Enter-Token | Out-Null
    if ((Wait-Connected $WaitSeconds) -ne $true) { Fail "not connected after the token (status: $(Status-Line))" }
    "connected: $(Status-Line)"
    SyncLog
  }
  "status" {
    Go-Settings
    "status: $(Status-Line) | indicator: $(Indicator)"
    $err = Find-Like "Last error:*" 1 "Text"
    if ($err) { $err.Current.Name }
    Capture-Window $hwnd "$out\m6-$tag-status.png"
    SyncLog
  }
  "type" {
    if ($Text -eq "") { Fail "-Text is required" }
    Show-Page
    $editor = Find-El "Edit" "Page content" 10
    if (-not $editor) { Fail "no page content editor" }
    $editor.SetFocus()
    Start-Sleep -Milliseconds 500
    # A fresh paragraph at the end of the document, then the line.
    Post-Enter
    Post-Text $Text
    Start-Sleep -Seconds 2
    if (-not (Has-Text $Text 10)) { Fail "the typed text did not appear" }
    "typed: $Text"
    Capture-Window $hwnd "$out\m6-$tag-typed.png"
  }
  "check" {
    if ($Text -eq "") { Fail "-Text is required" }
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $seen = $false
    while ((Get-Date) -lt $deadline) {
      Show-Page
      if (Has-Text $Text 1) { $seen = $true; break }
    }
    if (-not $seen) { Capture-Window $hwnd "$out\m6-$tag-missing.png"; Fail "'$Text' did not arrive within $WaitSeconds s" }
    "arrived: $Text"
    Capture-Window $hwnd "$out\m6-$tag-arrived.png"
    SyncLog
  }
  "disconnect" {
    Go-Settings
    $button = Find-El "Button" "Disconnect..." 10
    if (-not $button) { Fail "no 'Disconnect...' button" }
    Invoke-El $button
    Start-Sleep -Seconds 1
    $confirm = Find-El "Button" "Disconnect" 10
    if (-not $confirm) { Fail "no confirmation button" }
    Invoke-El $confirm
    Start-Sleep -Seconds 2
    if (-not (Find-El "Edit" "Server address" 10)) { Fail "the address field did not come back" }
    "disconnected"
    SyncLog
  }
  "none" { }
  default { "unknown step $m6Step" }
}
