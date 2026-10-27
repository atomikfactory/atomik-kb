# Installs or updates kb-server (the atomik-kb sync server) natively on this Windows machine,
# for a LAN / Tailscale-only setup: no Docker, no admin rights needed (except the optional
# firewall rule).
#
#   .\scripts\deploy\server-windows.ps1                       # build, install, start, check
#   .\scripts\deploy\server-windows.ps1 -Bind 100.64.0.5      # listen on the Tailscale IP only
#   .\scripts\deploy\server-windows.ps1 -SkipBuild            # reinstall the binary already built
#   .\scripts\deploy\server-windows.ps1 -Status               # task state, health, log tail
#   .\scripts\deploy\server-windows.ps1 -Uninstall            # remove the task and binary, keep the data
#
# Layout (per user, default %LOCALAPPDATA%\atomik-kb-server):
#   bin\kb-server.exe   the release binary (cargo build --release -p kb-server)
#   config.json         listen address, port, invite token (created once, never overwritten
#                       except for -Bind/-Port given on the command line)
#   run.ps1             sets the environment from config.json and runs the server
#   data\               server.db and its -wal/-shm files
#   backups\            offline copies of data\ taken before each update (last 10 kept)
#   logs\server.log     the server's log (no bodies, no tokens)
#
# The server runs as a scheduled task "atomik-kb-server" started at logon of this user and
# restarted on failure. It runs while this user is logged on; for a machine nobody logs into,
# use a service wrapper instead of the scheduled task.
# -NoTask starts the process directly instead (until the next logoff), for a quick test.

[CmdletBinding()]
param(
  [string]$Bind = "",          # listen address; "" = keep the configured one (default 0.0.0.0)
  [int]$Port = 0,              # listen port; 0 = keep the configured one (default 8787)
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "atomik-kb-server"),
  [string]$TaskName = "atomik-kb-server",
  [switch]$SkipBuild,
  [switch]$NoTask,
  [switch]$Status,
  [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if ($Bind -and $Bind -notmatch '^[0-9a-fA-F:.]+$') { throw "Bad -Bind '$Bind' (expected an IP address)." }
if ($Port -lt 0 -or $Port -gt 65535) { throw "Bad -Port $Port." }

$bin = Join-Path $InstallDir "bin\kb-server.exe"
$configPath = Join-Path $InstallDir "config.json"
$runPath = Join-Path $InstallDir "run.ps1"
$dataDir = Join-Path $InstallDir "data"
$backupDir = Join-Path $InstallDir "backups"
$logDir = Join-Path $InstallDir "logs"
$logPath = Join-Path $logDir "server.log"

function Get-ServerProcess {
  Get-Process -Name "kb-server" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ($_.Path -ieq $bin) }
}

function Stop-Server {
  if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  }
  # The run.ps1 wrapper restarts the server when it exits: stop the wrapper first.
  Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($runPath) } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  $procs = @(Get-ServerProcess)
  foreach ($p in $procs) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
  foreach ($p in $procs) { $p.WaitForExit(15000) | Out-Null }
}

function Get-CheckUrl($cfg) {
  $h = $cfg.bind
  if ($h -eq "0.0.0.0" -or $h -eq "::") { $h = "127.0.0.1" }
  if ($h -match ':') { $h = "[$h]" }
  return "http://${h}:$($cfg.port)/v2/server"
}

function Test-Health($cfg, [int]$Seconds = 20) {
  $url = Get-CheckUrl $cfg
  $deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $deadline) {
    try { return Invoke-RestMethod -Uri $url -TimeoutSec 3 } catch { Start-Sleep -Milliseconds 500 }
  }
  return $null
}

function Show-LogTail([int]$Lines = 30) {
  if (Test-Path $logPath) { Get-Content $logPath -Tail $Lines | ForEach-Object { Write-Host "  $_" } }
}

# ---- status ----
if ($Status) {
  if (-not (Test-Path $configPath)) { Write-Host "Not installed in $InstallDir."; exit 1 }
  $cfg = Get-Content $configPath -Raw | ConvertFrom-Json
  $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Write-Host ("Task:    " + $(if ($task) { $task.State } else { "not registered" }))
  Write-Host ("Process: " + $(if (Get-ServerProcess) { "running" } else { "not running" }))
  $health = Test-Health $cfg 3
  if ($health) { Write-Host "Health:  OK (protocolVersion $($health.protocolVersion), serverEpoch $($health.serverEpoch))" }
  else { Write-Host "Health:  no answer from $(Get-CheckUrl $cfg)" }
  Write-Host "Log tail ($logPath):"
  Show-LogTail 15
  exit 0
}

# ---- uninstall ----
if ($Uninstall) {
  Stop-Server
  if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Host "Scheduled task $TaskName removed."
  }
  if (Test-Path $bin) { Remove-Item $bin -Force }
  Write-Host "Binary removed. Data, backups, config (invite token) and logs are kept in $InstallDir;"
  Write-Host "delete that folder yourself to remove them (devices re-upload to a new server)."
  exit 0
}

# ---- build ----
$built = Join-Path $root "target\release\kb-server.exe"
if (-not $SkipBuild) {
  Push-Location $root
  try {
    . (Join-Path $root "scripts\dev-env.ps1")
    Write-Host "Building kb-server (release; the first build compiles SQLite and takes several minutes)..."
    cargo build --release --locked -p kb-server
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
  } finally { Pop-Location }
}
if (-not (Test-Path $built)) { throw "No ${built}: run without -SkipBuild." }

# ---- config (created once) ----
New-Item -ItemType Directory -Force (Split-Path $bin), $dataDir, $backupDir, $logDir | Out-Null
$firstInstall = -not (Test-Path $configPath)
if ($firstInstall) {
  $bytes = New-Object byte[] 24
  [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
  $cfg = [pscustomobject]@{
    bind = "0.0.0.0"; port = 8787
    inviteTokens = (($bytes | ForEach-Object { $_.ToString("x2") }) -join "")
    accountQuotaBytes = 10737418240; pruneGraceDays = 7; idleDeviceDays = ""; rustLog = "info"
  }
} else {
  $cfg = Get-Content $configPath -Raw | ConvertFrom-Json
}
if ($Bind) { $cfg.bind = $Bind }
if ($Port) { $cfg.port = $Port }
[IO.File]::WriteAllText($configPath, ($cfg | ConvertTo-Json), (New-Object Text.UTF8Encoding($false)))

# run.ps1: rewritten on every install so fixes reach existing installs.
$run = @'
# Started by the scheduled task (scripts/deploy/server-windows.ps1). Reads config.json, sets the
# kb-server environment and runs it, appending its output to logs\server.log, and starts it
# again 5 s after it exits (a crash, or a bad value in config.json, which the log names).
$ErrorActionPreference = "Stop"
$dir = $PSScriptRoot
$cfg = Get-Content (Join-Path $dir "config.json") -Raw | ConvertFrom-Json
$h = [string]$cfg.bind
if ($h -match ':') { $h = "[$h]" }
$env:KB_SERVER_LISTEN = "${h}:$($cfg.port)"
$env:KB_SERVER_DATA = Join-Path $dir "data"
$env:KB_SERVER_INVITE_TOKENS = [string]$cfg.inviteTokens
$env:KB_SERVER_ACCOUNT_QUOTA_BYTES = [string]$cfg.accountQuotaBytes
$env:KB_SERVER_PRUNE_GRACE_DAYS = [string]$cfg.pruneGraceDays
if ([string]$cfg.idleDeviceDays) { $env:KB_SERVER_IDLE_DEVICE_DAYS = [string]$cfg.idleDeviceDays }
$env:RUST_LOG = [string]$cfg.rustLog
$env:NO_COLOR = "1"
$exe = Join-Path $dir "bin\kb-server.exe"
$log = Join-Path $dir "logs\server.log"
while ($true) {
  & cmd.exe /c "`"$exe`" >> `"$log`" 2>&1"
  Add-Content -Path $log -Value ("{0} run.ps1: kb-server exited with code {1}; restarting in 5 s" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $LASTEXITCODE)
  Start-Sleep -Seconds 5
}
'@
[IO.File]::WriteAllText($runPath, $run, (New-Object Text.UTF8Encoding($true)))

# ---- stop, back up, replace ----
Stop-Server
if (Test-Path (Join-Path $dataDir "server.db")) {
  $dest = Join-Path $backupDir ("server-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
  New-Item -ItemType Directory -Force $dest | Out-Null
  Get-ChildItem $dataDir -Filter "server.db*" | Copy-Item -Destination $dest
  Write-Host "Backup (server stopped, consistent): $dest"
  Get-ChildItem $backupDir -Directory | Sort-Object Name -Descending | Select-Object -Skip 10 |
    Remove-Item -Recurse -Force
}
Copy-Item $built $bin -Force

# ---- start ----
$psArgs = "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$runPath`""
if ($NoTask) {
  Start-Process -FilePath "powershell.exe" -ArgumentList $psArgs -WindowStyle Hidden | Out-Null
  Write-Host "Started without a scheduled task (stops at logoff; -Uninstall or Stop-Process kb-server stops it)."
} else {
  $action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument $psArgs -WorkingDirectory $InstallDir
  $trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
  $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
  $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -MultipleInstances IgnoreNew -StartWhenAvailable
  Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal `
    -Settings $settings -Description "atomik-kb sync server (scripts/deploy/server-windows.ps1)" -Force | Out-Null
  Start-ScheduledTask -TaskName $TaskName
  Write-Host "Scheduled task $TaskName registered (starts at logon, restarts on failure) and started."
}

$health = Test-Health $cfg 30
if (-not $health) {
  Write-Host "Health check failed: no answer from $(Get-CheckUrl $cfg). Last log lines:" -ForegroundColor Red
  Show-LogTail 30
  exit 1
}
Write-Host "Health check OK: protocolVersion $($health.protocolVersion), serverEpoch $($health.serverEpoch)" -ForegroundColor Green

# ---- firewall (inbound from other devices) ----
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
  [Security.Principal.WindowsBuiltInRole]::Administrator)
$ruleCmd = "New-NetFirewallRule -DisplayName 'atomik-kb-server' -Direction Inbound -Action Allow -Protocol TCP " +
  "-LocalPort $($cfg.port) -Program '$bin' -RemoteAddress 100.64.0.0/10,LocalSubnet -Profile Any"
if (-not (Get-NetFirewallRule -DisplayName "atomik-kb-server" -ErrorAction SilentlyContinue)) {
  if ($isAdmin) {
    Invoke-Expression $ruleCmd | Out-Null
    Write-Host "Firewall rule 'atomik-kb-server' added (Tailscale range and local subnet only)."
  } else {
    Write-Host ""
    Write-Host "Other devices need an inbound firewall rule. In an elevated PowerShell, run once:" -ForegroundColor Yellow
    Write-Host "  $ruleCmd"
  }
}

# ---- summary ----
$appHost = $cfg.bind
if ($appHost -eq "0.0.0.0" -or $appHost -eq "::") {
  $ts = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object { $_.IPAddress -like "100.*" -and $_.InterfaceAlias -like "*Tailscale*" } | Select-Object -First 1
  $appHost = if ($ts) { $ts.IPAddress } else { $env:COMPUTERNAME }
}
Write-Host ""
Write-Host "Address for the app (Settings -> Sync): http://${appHost}:$($cfg.port)"
if ($firstInstall) {
  Write-Host "Invite token (needed once, to create your account): $($cfg.inviteTokens)" -ForegroundColor Yellow
  Write-Host "It is also in $configPath."
}
