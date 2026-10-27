# Online SQLite backup of kb-server's data on the server, optionally downloaded to
# dist\backups\ (git-ignored).
#
#   .\scripts\deploy\server-backup.ps1 -Target user@your-server [-Download] [-RemoteDir ~/atomik-kb-server]

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$Target,
  [string]$RemoteDir = "~/atomik-kb-server",
  [switch]$Download,
  [switch]$DryRun
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if ($Target -notmatch '^[A-Za-z0-9_.@-]+$') { throw "Bad -Target '$Target'." }
if ($RemoteDir -notmatch '^[~A-Za-z0-9_./-]+$') { throw "Bad -RemoteDir '$RemoteDir'." }
$dir = $RemoteDir -replace '^~/', ''
if ($dir -eq '~') { $dir = '.' }
$sshOpts = @("-o", "ConnectTimeout=10")
$remoteSh = "$dir/kb-server-remote.sh"

function Run([string]$exe, [string[]]$arguments) {
  Write-Host ("> " + $exe + " " + ($arguments -join " ")) -ForegroundColor DarkGray
  if ($DryRun) { return @() }
  $out = & $exe @arguments
  if ($LASTEXITCODE -ne 0) { throw "$exe failed (exit $LASTEXITCODE)" }
  return $out
}

Run "ssh" ($sshOpts + @($Target, "mkdir -p '$dir'")) | Out-Null
Run "scp" ($sshOpts + @((Join-Path $PSScriptRoot "kb-server-remote.sh"), "${Target}:$remoteSh")) | Out-Null
$out = Run "ssh" ($sshOpts + @($Target, "sed -i 's/\r$//' '$remoteSh' && bash '$remoteSh' backup '$dir'"))
if ($DryRun) { exit 0 }
$line = $out | Where-Object { $_ -like "BACKUP=*" } | Select-Object -First 1
if (-not $line) { throw "No backup line returned." }
$path = $line -replace '^BACKUP=', ''
Write-Host "Backup on the server: $path" -ForegroundColor Green
if ($Download -and $path -like "*.db") {
  $dest = Join-Path $root "dist\backups"
  New-Item -ItemType Directory -Force $dest | Out-Null
  Run "scp" ($sshOpts + @("${Target}:$path", $dest)) | Out-Null
  Write-Host "Downloaded to $dest" -ForegroundColor Green
}
