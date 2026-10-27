# Deploys kb-server (the atomik-kb sync server) to an Ubuntu host running Docker, over
# SSH, for a LAN / Tailscale-only setup (no Caddy, no public domain).
#
#   .\scripts\deploy\server.ps1 -Target user@your-server
#   .\scripts\deploy\server.ps1 -Target user@your-server -Bind 100.64.0.5 -Port 8787
#   .\scripts\deploy\server.ps1 -Target user@your-server -SkipBuild     # re-ship the image already built
#   .\scripts\deploy\server.ps1 -Target user@your-server -DryRun        # print the commands only
#
# Steps: build the image locally -> docker save to dist\deploy\*.tar -> scp -> docker load
# on the server -> copy docker-compose.lan.yml (as docker-compose.yml) and, on the first
# deploy only, create .env with a random invite token -> online backup of the existing
# data -> docker compose up -> GET /v2/server from the server.

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$Target,   # user@host (SSH)
  [int]$Port = 0,                                  # published port; 0 = keep the server's setting (default 8787)
  [string]$Bind = "",                              # address to publish on, e.g. the server's Tailscale IP; "" = keep (default 0.0.0.0)
  [string]$RemoteDir = "~/atomik-kb-server",
  [switch]$SkipBuild,
  [switch]$AllowDirty,                             # do not ask when the working tree is dirty (the tag gets -dirty either way)
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $root

# ---- validate (values end up in remote shell command lines) ----
if ($Target -notmatch '^[A-Za-z0-9_.@-]+$') { throw "Bad -Target '$Target' (expected user@host)." }
if ($Bind -and $Bind -notmatch '^[0-9a-fA-F:.]+$') { throw "Bad -Bind '$Bind' (expected an IP address)." }
if ($Port -lt 0 -or $Port -gt 65535) { throw "Bad -Port $Port." }
if ($RemoteDir -notmatch '^[~A-Za-z0-9_./-]+$') { throw "Bad -RemoteDir '$RemoteDir'." }
$dir = $RemoteDir -replace '^~/', ''      # relative to the remote home for both ssh and scp
if ($dir -eq '~') { $dir = '.' }
$hostPart = ($Target -split '@')[-1]

$image = "atomik-kb-server"
$sshOpts = @("-o", "ConnectTimeout=10")

function Run([string]$exe, [string[]]$arguments) {
  Write-Host ("> " + $exe + " " + ($arguments -join " ")) -ForegroundColor DarkGray
  if ($DryRun) { return @() }
  $out = & $exe @arguments
  if ($LASTEXITCODE -ne 0) { throw "$exe failed (exit $LASTEXITCODE)" }
  return $out
}

# ---- tag from git ----
$sha = (& git rev-parse --short HEAD).Trim()
$dirty = [bool](& git status --porcelain)
$tag = $sha
if ($dirty) {
  $tag = "$sha-dirty"
  Write-Warning "The working tree has uncommitted changes: the image is built from them and tagged $tag."
  if (-not $AllowDirty -and -not $DryRun) {
    $ans = Read-Host "Deploy a dirty build anyway? [y/N]"
    if ($ans -notmatch '^[yY]') { throw "Aborted (commit first, or pass -AllowDirty)." }
  }
}
Write-Host "== kb-server $tag -> ${Target}:$RemoteDir" -ForegroundColor Cyan

$docker = (Get-Command docker -ErrorAction SilentlyContinue).Source
if (-not $docker) { $docker = "C:\Program Files\Docker\Docker\resources\bin\docker.exe" }
if (-not $DryRun) {
  if (-not (Test-Path $docker)) { throw "docker not found; install Docker Desktop." }
  & $docker info *> $null
  if ($LASTEXITCODE -ne 0) { throw "The Docker daemon is not running; start Docker Desktop." }
  foreach ($t in "ssh", "scp") { if (-not (Get-Command $t -ErrorAction SilentlyContinue)) { throw "$t not found (enable the Windows OpenSSH client)." } }
}

# ---- 1. build ----
if (-not $SkipBuild) {
  Write-Host "== Building the image (the first build compiles SQLite/OpenSSL; be patient)" -ForegroundColor Cyan
  # Pull the base images through the daemon first: with the broken IPv6 stack on the dev
  # machine buildx's own metadata lookup fails (os error 10013), a daemon pull works.
  foreach ($base in "rust:1-bookworm", "gcr.io/distroless/cc-debian12:nonroot", "docker/dockerfile:1") {
    Run $docker @("pull", $base) | Out-Null
  }
  Run $docker @("build", "-f", "deploy/kb-server/Dockerfile", "-t", "${image}:$tag", "-t", "${image}:latest", ".") | Out-Null
} elseif (-not $DryRun) {
  & $docker image inspect "${image}:latest" *> $null
  if ($LASTEXITCODE -ne 0) { throw "-SkipBuild, but ${image}:latest does not exist locally." }
  # Retag so the server runs a tag that names what was shipped.
  & $docker tag "${image}:latest" "${image}:$tag"
}

# ---- 2. save the image ----
$outDir = Join-Path $root "dist\deploy"
New-Item -ItemType Directory -Force $outDir | Out-Null
$tar = Join-Path $outDir "$image-$tag.tar"
Write-Host "== Saving the image" -ForegroundColor Cyan
# docker save -o writes the file itself: no PowerShell pipe touches the binary data.
Run $docker @("save", "-o", $tar, "${image}:$tag") | Out-Null

# ---- 3. remote directory and helper script ----
$remoteSh = "$dir/kb-server-remote.sh"
Write-Host "== Preparing $Target" -ForegroundColor Cyan
Run "ssh" ($sshOpts + @($Target, "mkdir -p '$dir/backups'")) | Out-Null
Run "scp" ($sshOpts + @((Join-Path $PSScriptRoot "kb-server-remote.sh"), "${Target}:$remoteSh")) | Out-Null
Run "scp" ($sshOpts + @((Join-Path $root "deploy\kb-server\docker-compose.lan.yml"), "${Target}:$dir/docker-compose.yml")) | Out-Null
# The helper may have CRLF line endings after a git checkout on Windows.
$remote = "sed -i 's/\r$//' '$remoteSh' && bash '$remoteSh'"

$prep = Run "ssh" ($sshOpts + @($Target, "$remote prepare '$dir'"))
$previous = $false
if ($prep -contains "PREVIOUS=yes") { $previous = $true }
$firstToken = ($prep | Where-Object { $_ -like "KB_FIRST_TOKEN=*" } | Select-Object -First 1)

# ---- 4. ship and load the image ----
Write-Host "== Copying the image" -ForegroundColor Cyan
$remoteTar = "$dir/$image-$tag.tar"
Run "scp" ($sshOpts + @($tar, "${Target}:$remoteTar")) | Out-Null
Run "ssh" ($sshOpts + @($Target, "docker load -i '$remoteTar' && rm -f '$remoteTar'")) | Out-Null

# ---- 5. backup the existing data ----
if ($previous -or $DryRun) {
  Write-Host "== Backing up the existing data" -ForegroundColor Cyan
  $bk = Run "ssh" ($sshOpts + @($Target, "$remote backup '$dir'"))
  $bk | Where-Object { $_ -like "BACKUP=*" } | ForEach-Object { Write-Host $_ }
}

# ---- 6. up and health check ----
Write-Host "== Starting" -ForegroundColor Cyan
$portArg = ""
if ($Port -gt 0) { $portArg = "$Port" }
$upOut = $null
try {
  $upOut = Run "ssh" ($sshOpts + @($Target, "$remote up '$dir' '$tag' '$Bind' '$portArg'"))
} catch {
  Write-Host "The new container did not become healthy (log lines above). Data backups are in $RemoteDir/backups." -ForegroundColor Red
  exit 1
}

if ($DryRun) { Write-Host "`n(dry run: nothing was executed)" -ForegroundColor Yellow; exit 0 }

$json = ($upOut | Where-Object { $_ -like "HEALTH_JSON=*" } | Select-Object -First 1) -replace '^HEALTH_JSON=', ''
$bindOut = (($upOut | Where-Object { $_ -like "BIND=*" } | Select-Object -First 1) -replace '^BIND=', '')
$portOut = (($upOut | Where-Object { $_ -like "PORT=*" } | Select-Object -First 1) -replace '^PORT=', '')
try {
  $info = $json | ConvertFrom-Json
  Write-Host ("Health check OK: protocolVersion {0}, serverEpoch {1}" -f $info.protocolVersion, $info.serverEpoch) -ForegroundColor Green
} catch {
  Write-Host "Server answered, but the reply was not understood: $json" -ForegroundColor Yellow
}

$urlHost = $hostPart
if ($bindOut -and $bindOut -ne "0.0.0.0") { $urlHost = $bindOut }
Write-Host ""
Write-Host "Server address for the app (Settings -> Sync): http://${urlHost}:${portOut}" -ForegroundColor Green
if ($urlHost -ne $hostPart) { Write-Host "(or http://${hostPart}:${portOut} if that name resolves to the bound address)" }
if ($firstToken) {
  Write-Host ""
  Write-Host "First deployment. Invite token (shown only now; also in $RemoteDir/.env on the server):" -ForegroundColor Yellow
  Write-Host ($firstToken -replace '^KB_FIRST_TOKEN=', '') -ForegroundColor Yellow
}
