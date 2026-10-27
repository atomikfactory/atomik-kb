# Milestone 7 steps on running desktop instances (launch.ps1 for A, launch-b.ps1 for the
# others) against the local kb-server of server.ps1, through UI Automation and posted
# keystrokes (the helpers of drive-m4.ps1; `-AppPid` picks the instance). See README.md.
#
#   .\drive-m7.ps1 -Step join-start -AppPid J        onboarding: "Add this device to my account", server address,
#                                                    Connect; prints the pairing code (also to out\pairing-code.txt)
#   .\drive-m7.ps1 -Step sponsor-start -AppPid S     Settings -> Security -> Devices -> "Add a device...", types the
#                                                    code from out\pairing-code.txt, Continue; prints the digits
#   .\drive-m7.ps1 -Step join-confirm -AppPid J      compares the digits with out\pairing-sas.txt and confirms
#   .\drive-m7.ps1 -Step sponsor-confirm -AppPid S   waits for "You are adding", confirms, waits for Done
#   .\drive-m7.ps1 -Step join-wait -AppPid J         waits for the joined workspace (the tree)
#   .\drive-m7.ps1 -Step devices -AppPid N           prints the device list of Settings -> Security
#   .\drive-m7.ps1 -Step rename -Name "<name>" -AppPid N   renames this device (Settings -> This device)
#   .\drive-m7.ps1 -Step revoke -Name "<device>" -AppPid S   Remove... the named device, confirm, save the new kit
#                                                    words to out\kit.txt, prints the first words
#   .\drive-m7.ps1 -Step removed -AppPid J           waits for the "removed from the account" screen
#   .\drive-m7.ps1 -Step recover -AppPid N           onboarding: "Recover with my kit", server address, kit from
#                                                    out\kit.txt, Recover; waits for the workspace
param(
  [string]$Step = "join-start",
  [int]$AppPid = 0,
  [string]$Name = "",
  [string]$Url = "http://127.0.0.1:8787",
  [int]$WaitSeconds = 90
)
$ErrorActionPreference = "Continue"
$m7Step = $Step
$m7Pid = $AppPid
$m7Name = $Name
. "$PSScriptRoot\drive-m6.ps1" -Step "none" -AppPid $m7Pid 2>$null | Out-Null
$codeFile = "$out\pairing-code.txt"
$sasFile = "$out\pairing-sas.txt"
$tag = if ($m7Pid -ne 0) { "pid$m7Pid" } else { "a" }

function Digits() {
  # The six digits are two Text nodes of three inside the "Six digits to compare" group.
  $group = Find-El "Group" "Six digits to compare" 1
  if (-not $group) { return $null }
  $parts = @()
  foreach ($e in (All-El $group)) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Text" -and $c.Name -match '^\d{3}$') { $parts += $c.Name } }
  if ($parts.Count -eq 2) { return ($parts -join "") }
  return $null
}
function Wait-Digits([int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) { $d = Digits; if ($d) { return $d }; Start-Sleep -Seconds 1 }
  return $null
}
function Wait-Text([string]$pattern, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) { if (Find-Like $pattern 1) { return $true }; Start-Sleep -Seconds 1 }
  return $false
}

switch ($m7Step) {
  "join-start" {
    # A session already on screen (an earlier run) is read as it is.
    if (-not (Find-El "Group" "Pairing code" 2)) {
      # A join screen already up (an earlier attempt that stopped) is retried from Connect.
      $field = Find-El "Edit" "Sync server address" 2
      if (-not $field) {
        $button = Find-El "Button" "Add this device to my account" 20
        if (-not $button) { Fail "no 'Add this device to my account' button (not on onboarding)" }
        Invoke-El $button
        Start-Sleep -Seconds 1
        $field = Find-El "Edit" "Sync server address" 10
        if (-not $field) { Fail "no server address field" }
      }
      Type-Into $field $Url
      Invoke-El (Find-El "Button" "Connect" 10)
    }
    # The code is three Text nodes of four characters inside the "Pairing code" group.
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $code = $null
    while ((Get-Date) -lt $deadline -and -not $code) {
      $group = Find-El "Group" "Pairing code" 1
      if ($group) {
        $parts = @()
        foreach ($e in (All-El $group)) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Text" -and $c.Name -match '^[0-9A-Z]{4}$') { $parts += $c.Name } }
        if ($parts.Count -eq 3) { $code = $parts -join " " }
      }
      if (-not $code) { Start-Sleep -Seconds 1 }
    }
    if (-not $code) { Capture-Window $hwnd "$out\m7-$tag-nocode.png"; Fail "no pairing code within $WaitSeconds s" }
    Set-Content -Path $codeFile -Value $code -Encoding ascii
    "pairing code: $code"
    Capture-Window $hwnd "$out\m7-$tag-code.png"
  }
  "sponsor-start" {
    if (-not (Test-Path $codeFile)) { Fail "no $codeFile (join-start first)" }
    $code = (Get-Content $codeFile -Raw).Trim()
    # A dialog already at the digits (an earlier run) is read as it is.
    if (-not (Find-El "Group" "Six digits to compare" 2)) {
      Go-Settings
      $add = Find-El "Button" "Add a device..." 20
      if (-not $add) { Fail "no 'Add a device...' button" }
      Invoke-El $add
      Start-Sleep -Seconds 1
      $field = Find-El "Edit" "Code shown on the new device" 10
      if (-not $field) { Fail "no code field" }
      Type-Into $field $code
      Invoke-El (Find-El "Button" "Continue" 10)
    }
    $digits = Wait-Digits $WaitSeconds
    if (-not $digits) { Capture-Window $hwnd "$out\m7-$tag-nodigits.png"; Fail "no digits on the sponsor within $WaitSeconds s" }
    Set-Content -Path $sasFile -Value $digits -Encoding ascii
    "sponsor digits: $digits"
    Capture-Window $hwnd "$out\m7-$tag-digits.png"
  }
  "join-confirm" {
    $digits = Wait-Digits $WaitSeconds
    if (-not $digits) { Capture-Window $hwnd "$out\m7-$tag-nodigits.png"; Fail "no digits on the joiner" }
    $expected = if (Test-Path $sasFile) { (Get-Content $sasFile -Raw).Trim() } else { "" }
    "joiner digits: $digits (sponsor: $expected)"
    if ($expected -and $digits -ne $expected) { Fail "the digits differ: $digits vs $expected" }
    Capture-Window $hwnd "$out\m7-$tag-digits.png"
    Invoke-El (Find-El "Button" "Confirm" 10)
    Start-Sleep -Seconds 2
    "joiner confirmed"
  }
  "sponsor-confirm" {
    if (-not (Wait-Text "You are adding*" $WaitSeconds)) { Fail "the sponsor never saw the new device's name" }
    $adding = (Find-Like "You are adding*" 1).Current.Name
    "$adding"
    Capture-Window $hwnd "$out\m7-$tag-adding.png"
    Invoke-El (Find-El "Button" "Confirm" 10)
    if (-not (Wait-Text "* was added to your account*" $WaitSeconds)) { Fail "the sponsor did not finish (status: $(Dump 'Text'))" }
    "sponsor done"
    Capture-Window $hwnd "$out\m7-$tag-done.png"
    $close = Find-El "Button" "Close" 5
    if ($close) { Invoke-El $close }
  }
  "join-wait" {
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $ok = $false
    while ((Get-Date) -lt $deadline) {
      if (Find-El "TreeItem" $pageTitle 1) { $ok = $true; break }
      if (Find-El "Button" "New page" 1) { $ok = $true; break }
      Start-Sleep -Seconds 2
    }
    if (-not $ok) { Capture-Window $hwnd "$out\m7-$tag-notjoined.png"; Fail "the joined workspace did not appear" }
    "joined: workspace shown"
    Capture-Window $hwnd "$out\m7-$tag-joined.png"
  }
  "devices" {
    Go-Settings
    Start-Sleep -Seconds 1
    foreach ($e in (All-El (Web))) {
      $c = $e.Current
      if ($c.Name -like "Added *" -or $c.Name -like "Remove *" -or $c.Name -eq "This device" -or $c.Name -like "*via kit*") { "  {0,-10} '{1}'" -f ($c.ControlType.ProgrammaticName -replace 'ControlType.',''), $c.Name }
    }
    Capture-Window $hwnd "$out\m7-$tag-devices.png"
  }
  "revoke" {
    if ($m7Name -eq "") { Fail "-Name is required" }
    Go-Settings
    $remove = Find-El "Button" "Remove $m7Name" 20
    if (-not $remove) { Fail "no 'Remove $m7Name' button" }
    Invoke-El $remove
    Start-Sleep -Seconds 1
    $pass = Find-El "Edit" "App lock passphrase" 3
    if ($pass) { Type-Into $pass $passphrase }
    Invoke-El (Find-El "Button" "Remove device" 10)
    if (-not (Find-El "List" "Recovery kit words" 60)) { Fail "no new kit after the removal" }
    $words = Read-KitWords
    if ($words.Count -ne 24) { Fail "expected 24 words, got $($words.Count)" }
    Set-Content -Path $kitFile -Value ($words -join " ") -Encoding ascii
    "device removed; new kit saved ($($words[0]) $($words[1]) ...)"
    Capture-Window $hwnd "$out\m7-$tag-newkit.png"
    $saved = Find-El "Button" "I saved my kit" 5
    if ($saved) { Invoke-El $saved; Start-Sleep -Seconds 1 }
    $done = Find-El "Button" "Done" 3
    if ($done) { Invoke-El $done }
  }
  "removed" {
    if (-not (Wait-Text "This device was removed from the account*" $WaitSeconds)) { Fail "the removed screen did not appear (status: $(Status-Line))" }
    "removed screen shown"
    Capture-Window $hwnd "$out\m7-$tag-removed.png"
  }
  "recover" {
    if (-not (Test-Path $kitFile)) { Fail "no $kitFile" }
    $button = Find-El "Button" "Recover with my kit" 20
    if (-not $button) { Fail "no 'Recover with my kit' button (not on onboarding)" }
    Invoke-El $button
    Start-Sleep -Seconds 1
    $field = Find-El "Edit" "Sync server address" 10
    if (-not $field) { Fail "no server address field" }
    Type-Into $field $Url
    $kitField = Find-El "Edit" "Recovery kit" 10
    if (-not $kitField) { $kitField = Find-El "Document" "Recovery kit" 5 }
    if (-not $kitField) { Fail "no kit field" }
    Set-El $kitField (Get-Content $kitFile -Raw).Trim()
    Invoke-El (Find-El "Button" "Recover" 10)
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $ok = $false
    while ((Get-Date) -lt $deadline) {
      if (Find-El "TreeItem" $pageTitle 1) { $ok = $true; break }
      if (Find-Like "Recovery stopped*" 1) { break }
      Start-Sleep -Seconds 2
    }
    if (-not $ok) { Capture-Window $hwnd "$out\m7-$tag-notrecovered.png"; Fail "the recovered workspace did not appear ($(Dump 'Text'))" }
    "recovered: workspace shown"
    Capture-Window $hwnd "$out\m7-$tag-recovered.png"
  }
  "rename" {
    # Settings -> This device: a new device name, so the other devices can tell it apart.
    if ($m7Name -eq "") { Fail "-Name is required" }
    Go-Settings
    $field = Find-El "Edit" "Device name" 20
    if (-not $field) { Fail "no 'Device name' field" }
    Type-Into $field $m7Name
    Invoke-El (Find-El "Button" "Save name" 10)
    if (-not (Has-Text "Device name saved." 20)) { Fail "the name was not saved" }
    "renamed to $m7Name"
  }
  "none" { }
  default { "unknown step $m7Step" }
}
