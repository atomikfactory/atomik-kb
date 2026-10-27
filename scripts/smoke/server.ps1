# Runs a local kb-server for the Milestone 6 real-app pass (see README.md).
#
#   .\server.ps1 -Action start    start target\debug\kb-server.exe on 127.0.0.1:8787 with the
#                                 data directory out\server and the invite token "dev"
#   .\server.ps1 -Action stop     stop it
#   .\server.ps1 -Action reset    stop, delete out\server (the server loses its data: new epoch), start
#   .\server.ps1 -Action status   GET /v2/server
#
# A data directory written by a protocol v1 server (before M13) is refused at startup
# (ADR-0045: v2 starts fresh): run -Action reset once.
#
# Build it first with `cargo build -p kb-server`. Logs go to out\server.log (requests only,
# never bodies or tokens).
#
# The web viewer of view-only links (M13 slice 4) is served at /view when apps\viewer\dist exists
# (`pnpm --filter @atomik-kb/viewer build`, see apps\viewer\README.md): this script points
# KB_SERVER_VIEWER_DIR at it. Without the build, /view answers 404 and nothing else changes.
param([string]$Action = "start", [string]$Listen = "127.0.0.1:8787", [string]$Token = "dev")
$ErrorActionPreference = "Continue"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$data = Join-Path $out "server"
$exe = Join-Path $repo "target\debug\kb-server.exe"
$log = Join-Path $out "server.log"
$errLog = Join-Path $out "server.err.log"

function Stop-Server() {
  # Only the server built by this checkout: another worktree (or another session) may run its own kb-server.
  $procs = Get-Process -Name "kb-server" -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq $exe }
  foreach ($p in $procs) { "stopping kb-server $($p.Id)"; Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Seconds 1
}
function Start-Server() {
  if (-not (Test-Path $exe)) { "no $exe (cargo build -p kb-server first)"; exit 1 }
  New-Item -ItemType Directory -Force $data | Out-Null
  $env:KB_SERVER_LISTEN = $Listen
  $env:KB_SERVER_DATA = $data
  $env:KB_SERVER_INVITE_TOKENS = $Token
  $env:RUST_LOG = "info"
  $viewer = Join-Path $repo "apps\viewer\dist"
  if (Test-Path (Join-Path $viewer "index.html")) { $env:KB_SERVER_VIEWER_DIR = $viewer; "viewer: $viewer" } else { Remove-Item Env:KB_SERVER_VIEWER_DIR -ErrorAction SilentlyContinue }
  $proc = Start-Process -FilePath $exe -PassThru -WindowStyle Hidden -RedirectStandardOutput $log -RedirectStandardError $errLog
  "kb-server pid $($proc.Id) on $Listen (data: $data)"
  $deadline = (Get-Date).AddSeconds(20)
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
    try { $info = Invoke-RestMethod -Uri "http://$Listen/v2/server" -TimeoutSec 2; "server up: epoch $($info.serverEpoch) protocol $($info.protocolVersion)"; return } catch {}
  }
  "the server did not answer within 20 s"; Get-Content $errLog -Tail 20; exit 1
}

switch ($Action) {
  "start" { Stop-Server; Start-Server }
  "stop" { Stop-Server; "stopped" }
  "reset" {
    Stop-Server
    if (Test-Path $data) { Remove-Item -Recurse -Force $data; "deleted $data" }
    Start-Server
  }
  "status" {
    try { Invoke-RestMethod -Uri "http://$Listen/v2/server" -TimeoutSec 3 | ConvertTo-Json -Compress } catch { "not reachable: $($_.Exception.Message)" }
  }
  default { "unknown action $Action" }
}
