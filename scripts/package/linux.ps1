# Builds the Linux .deb in a reproducible Ubuntu 22.04 Docker image.
#
#   .\scripts\package\linux.ps1                 # build image (cached), build the .deb
#   .\scripts\package\linux.ps1 -NoImageBuild   # reuse the existing image
#   .\scripts\package\linux.ps1 -Verify         # build, then install it in clean ubuntu 22.04 / 24.04 / debian 12
#   .\scripts\package\linux.ps1 -VerifyOnly     # only verify the .deb already in dist/installers
#
# Output: dist/installers/atomik-kb_<version>_amd64.deb (git-ignored), with its SHA-256.
#
# How the source gets in: the host repo is NOT bind-mounted. node_modules/ and target/
# on the host hold Windows binaries, and sharing target/ would clobber a concurrent host
# build. Instead a tarball of the WORKING TREE (tracked + untracked-not-ignored files, so
# uncommitted work is included, deleted files are skipped) is copied into a throwaway
# container. Named volumes keep the cargo registry, the pnpm store and the container's
# target dir between runs, so rebuilds are incremental.

[CmdletBinding()]
param(
  [switch]$NoImageBuild,
  [switch]$Verify,          # after the build, install the .deb in clean ubuntu:22.04, ubuntu:24.04 and debian:12 containers
  [switch]$VerifyOnly,      # skip the build; verify the .deb already in dist/installers
  [switch]$ResetCache
)

$ErrorActionPreference = "Continue"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $root

$docker = (Get-Command docker -ErrorAction SilentlyContinue).Source
if (-not $docker) { $docker = "C:\Program Files\Docker\Docker\resources\bin\docker.exe" }
if (-not (Test-Path $docker)) { throw "docker not found; install Docker Desktop and start it." }
& $docker info *> $null
if ($LASTEXITCODE -ne 0) { throw "The Docker daemon is not running; start Docker Desktop." }

# Versions come from the repo, so the image follows the pins.
$rustVersion = (Select-String -Path "rust-toolchain.toml" -Pattern '^\s*channel\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$pnpmVersion = (Select-String -Path "package.json" -Pattern '"packageManager"\s*:\s*"pnpm@([^"]+)"').Matches[0].Groups[1].Value
$nodeVersion = "25.5.0"   # the version the repo is developed with (node -v on the dev machine)
$image = "atomik-kb-linux-build:ubuntu22.04"
$volumes = @("atomik-kb-linux-cargo", "atomik-kb-linux-target", "atomik-kb-linux-pnpm")

$ok = $false
$outDir = Join-Path $root "dist\installers"
New-Item -ItemType Directory -Force $outDir | Out-Null
if (-not $VerifyOnly) {
if ($ResetCache) {
  foreach ($v in $volumes) { & $docker volume rm $v *> $null }
}

if (-not $NoImageBuild) {
  # Pull the base image through the daemon first: with the broken IPv6 stack on the dev
  # machine, buildx's own client-side metadata lookup (auth.docker.io) fails with
  # "connectex ... forbidden by its access permissions", while a daemon pull works; once the
  # image is local the build needs no lookup.
  & $docker pull ubuntu:22.04 *> $null
  Write-Host "== Building image $image (rust $rustVersion, node $nodeVersion, pnpm $pnpmVersion)" -ForegroundColor Cyan
  & $docker build -t $image `
    --build-arg "RUST_VERSION=$rustVersion" --build-arg "NODE_VERSION=$nodeVersion" --build-arg "PNPM_VERSION=$pnpmVersion" `
    (Join-Path $PSScriptRoot "linux")
  if ($LASTEXITCODE -ne 0) { throw "docker build failed" }
}

# Working-tree source tarball.
$scratch = Join-Path ([System.IO.Path]::GetTempPath()) "atomik-kb-linux-pkg"
New-Item -ItemType Directory -Force $scratch | Out-Null
$list = Join-Path $scratch "files.txt"
$tar = Join-Path $scratch "source.tar"
$files = & git -c core.quotepath=false ls-files --cached --others --exclude-standard |
  Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } | Sort-Object -Unique
[System.IO.File]::WriteAllLines($list, [string[]]$files, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "== Packing $($files.Count) working-tree files" -ForegroundColor Cyan
& tar -cf $tar -T $list
if ($LASTEXITCODE -ne 0) { throw "tar failed" }

$name = "atomik-kb-linux-build-$([guid]::NewGuid().ToString('N').Substring(0,8))"
try {
  & $docker create --name $name `
    -v atomik-kb-linux-cargo:/opt/cargo/registry `
    -v atomik-kb-linux-target:/cache/target `
    -v atomik-kb-linux-pnpm:/cache/pnpm-store `
    $image bash /build/build-in-container.sh | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "docker create failed" }
  & $docker cp $tar "${name}:/build/source.tar"
  & $docker cp (Join-Path $PSScriptRoot "linux\build-in-container.sh") "${name}:/build/build-in-container.sh"
  Write-Host "== Building in container $name (the first build compiles SQLCipher/OpenSSL; be patient)" -ForegroundColor Cyan
  & $docker start -a $name
  if ($LASTEXITCODE -ne 0) { throw "the build in the container failed" }
  Get-ChildItem $outDir -Filter "atomik-kb_*.deb" -ErrorAction SilentlyContinue | Remove-Item -Force
  & $docker cp "${name}:/build/out/." $outDir
  if ($LASTEXITCODE -ne 0) { throw "docker cp of the .deb failed" }
  $ok = $true
} finally {
  & $docker rm -f $name *> $null
  Remove-Item -Force $tar, $list -ErrorAction SilentlyContinue
}
}  # -not $VerifyOnly

if ($ok) {
  Get-ChildItem $outDir -Filter "atomik-kb_*.deb" | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    Write-Host ""
    Write-Host "deb:     $($_.FullName)"
    Write-Host "size:    $($_.Length) bytes"
    Write-Host "sha256:  $hash"
  }
}

if ($VerifyOnly) { $ok = $true }
if ($ok -and ($Verify -or $VerifyOnly)) {
  $failed = @()
  foreach ($img in @("ubuntu:22.04", "ubuntu:24.04", "debian:12")) {
    Write-Host ""
    Write-Host "== Verifying the .deb in a clean $img container" -ForegroundColor Cyan
    & $docker pull $img *> $null
    & $docker run --rm -v "${outDir}:/pkg:ro" -v "$(Join-Path $PSScriptRoot 'linux'):/v:ro" $img bash /v/verify-in-container.sh
    if ($LASTEXITCODE -ne 0) { $failed += $img }
  }
  if ($failed.Count -gt 0) { throw "verification failed on: $($failed -join ', ')" }
  Write-Host "Verification passed on ubuntu:22.04, ubuntu:24.04 and debian:12." -ForegroundColor Green
}
