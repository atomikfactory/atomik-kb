# Fetches what the dictation sidecar needs to build and run on this machine (ADR-0053,
# docs/architecture/dictation.md section 6): the sherpa-onnx native libraries (the DLLs and their import
# libraries) and the Silero voice activity model. Both land in .build/speech/ (ignored by git), checked
# against the SHA-256 of the pinned release asset. Idempotent: what is already there and verified is kept.
#
#   . .\scripts\dev-env.ps1; .\scripts\fetch-speech-deps.ps1
#   .\scripts\fetch-speech-deps.ps1 -Force      # download and unpack again
#
# Afterwards `scripts/dev-env.ps1` sets SHERPA_ONNX_LIB_DIR (so `cargo build` does not download the libraries
# itself) and KB_SPEECH_VAD (where a debug build of the shell finds the voice activity model). The speech
# *models* (Parakeet, Moonshine) are not fetched here: the app downloads them on first use.
#
# curl.exe is used for the downloads: on this machine Rust-built executables can fail to connect (os error
# 10013) while curl.exe does not. Windows x64 only.

[CmdletBinding()]
param(
  [switch]$Force
)

$ErrorActionPreference = "Stop"

# Pinned: sherpa-onnx 1.13.8, the "shared" (DLL) build with a static CRT (MT), Windows x64.
$ArchiveName = "sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib.tar.bz2"
$ArchiveUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/$ArchiveName"
$ArchiveBytes = 8032957
$ArchiveSha256 = "b8eedf41bd6d3779218887b48367bb7a3ece5aaa7667f01f69ee823a12b0a9e7"
$ExtractedDir = "sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib"

# Silero VAD (MIT). The release tag is mutable, so the file is pinned by its hash only.
$VadName = "silero_vad.onnx"
$VadUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
$VadBytes = 643854
$VadSha256 = "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6"

# What the extracted lib folder must hold (sizes of the pinned build).
$ExpectedLibs = @{
  "sherpa-onnx-c-api.dll" = 4605952
  "onnxruntime.dll"       = 17799168
  "sherpa-onnx-c-api.lib" = $null
  "onnxruntime.lib"       = $null
}

$repo = Split-Path -Parent $PSScriptRoot
$root = Join-Path $repo ".build\speech"
$archivePath = Join-Path $root $ArchiveName
$libDir = Join-Path $root "$ExtractedDir\lib"
$vadPath = Join-Path $root $VadName

function Get-Sha256([string]$Path) {
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Test-File([string]$Path, [long]$Bytes, [string]$Sha256) {
  if (-not (Test-Path -LiteralPath $Path)) { return $false }
  if ((Get-Item -LiteralPath $Path).Length -ne $Bytes) { return $false }
  return (Get-Sha256 $Path) -eq $Sha256
}

function Test-Libs() {
  foreach ($name in $ExpectedLibs.Keys) {
    $path = Join-Path $libDir $name
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    $size = $ExpectedLibs[$name]
    if ($null -ne $size -and (Get-Item -LiteralPath $path).Length -ne $size) { return $false }
  }
  return $true
}

function Get-Download([string]$Url, [string]$Path, [long]$Bytes, [string]$Sha256) {
  Write-Host "Downloading $Url"
  $part = "$Path.part"
  if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
  & curl.exe -L --fail --silent --show-error --retry 3 -o $part $Url
  if ($LASTEXITCODE -ne 0) { throw "curl.exe failed ($LASTEXITCODE) downloading $Url" }
  if (-not (Test-File $part $Bytes $Sha256)) {
    Remove-Item -LiteralPath $part -Force
    throw "$Url is not the pinned file (size or SHA-256 differs); nothing was kept"
  }
  Move-Item -LiteralPath $part -Destination $Path -Force
}

New-Item -ItemType Directory -Force -Path $root | Out-Null

# --- the native libraries ---
if ($Force -or -not (Test-Libs)) {
  if ($Force -or -not (Test-File $archivePath $ArchiveBytes $ArchiveSha256)) {
    Get-Download $ArchiveUrl $archivePath $ArchiveBytes $ArchiveSha256
  } else {
    Write-Host "Archive already there and verified: $ArchiveName"
  }
  # Unpack into a fresh folder next to the final one, then swap, so a half-unpacked tree is never used.
  $staging = Join-Path $root "unpack.tmp"
  if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
  New-Item -ItemType Directory -Force -Path $staging | Out-Null
  & tar.exe -xjf $archivePath -C $staging
  if ($LASTEXITCODE -ne 0) { throw "tar.exe could not unpack $ArchiveName ($LASTEXITCODE)" }
  $final = Join-Path $root $ExtractedDir
  if (Test-Path -LiteralPath $final) { Remove-Item -LiteralPath $final -Recurse -Force }
  Move-Item -LiteralPath (Join-Path $staging $ExtractedDir) -Destination $final
  Remove-Item -LiteralPath $staging -Recurse -Force
  if (-not (Test-Libs)) { throw "the unpacked archive does not hold the expected libraries in $libDir" }
  Write-Host "Native libraries ready: $libDir"
} else {
  Write-Host "Native libraries already there: $libDir"
}

# --- the voice activity model ---
if ($Force -or -not (Test-File $vadPath $VadBytes $VadSha256)) {
  Get-Download $VadUrl $vadPath $VadBytes $VadSha256
  Write-Host "Voice activity model ready: $vadPath"
} else {
  Write-Host "Voice activity model already there and verified: $vadPath"
}

Write-Host ""
Write-Host "SHERPA_ONNX_LIB_DIR = $libDir"
Write-Host "KB_SPEECH_VAD       = $vadPath"
Write-Host "(scripts/dev-env.ps1 sets both when these exist)"
