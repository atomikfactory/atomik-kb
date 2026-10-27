# Builds and stages the dictation sidecar for the Windows installer (ADR-0053,
# docs/architecture/dictation.md section 7). Called by windows.ps1 after the MCP bridge; can be run by hand to
# look at what the installer will get:
#
#   .\scripts\package\stage-speech.ps1
#
# What it does, in <root>\apps\desktop\src-tauri\binaries (git-ignored):
#   - cargo build -p kb-speech-native --release (needs SHERPA_ONNX_LIB_DIR, set by scripts/dev-env.ps1 once
#     scripts/fetch-speech-deps.ps1 has run), copied to kb-speech-<target triple>.exe: the sidecar of
#     bundle.externalBin in tauri.bridge.conf.json, which NSIS installs as <install dir>\kb-speech.exe;
#   - the three DLLs the sidecar loads (sherpa-onnx-c-api.dll, onnxruntime.dll, onnxruntime_providers_shared.dll;
#     sherpa-onnx-cxx-api.dll is not needed, nothing links it) and silero_vad.onnx (checked against its pinned SHA-256),
#     which bundle.resources of the same file installs next to the sidecar: a resource goes to <install dir>\<target>,
#     Windows looks for a DLL in the folder of the executable first, and SpeechShell::vad looks in the resource
#     directory (the folder of the executable on Windows) first;
#   - a check that the sidecar and the three DLLs, alone in a folder of their own, load (kb-speech.exe --version prints
#     the same line as the build's own copy).
# THIRD-PARTY-NOTICES.txt (tracked, in src-tauri) is a resource too; this script only checks that it is there.
#
# -Root   tree to stage into (default: this repo). windows.ps1 passes its own.
# -Triple the target triple (default: the host's, from rustc -vV).

[CmdletBinding()]
param(
  [string]$Root,
  [string]$Triple
)

$ErrorActionPreference = "Continue"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $Root) { $Root = $repo }
$Root = (Resolve-Path $Root).Path
Set-Location $Root
. (Join-Path $repo "scripts\dev-env.ps1")
if (-not $env:SHERPA_ONNX_LIB_DIR -or -not $env:KB_SPEECH_VAD) {
  Write-Error "SHERPA_ONNX_LIB_DIR and KB_SPEECH_VAD are not set: run scripts\fetch-speech-deps.ps1"
  exit 1
}
if (-not $Triple) {
  $Triple = ((rustc -vV | Select-String -Pattern '^host:').Line -replace '^host:\s*', '').Trim()
  if (-not $Triple) { Write-Error "cannot read the host target triple from rustc -vV"; exit 1 }
}
$targetDir = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
$binaries = Join-Path $Root "apps\desktop\src-tauri\binaries"
New-Item -ItemType Directory -Force $binaries | Out-Null

Write-Host "Building kb-speech ($Triple)"
cargo build -p kb-speech-native --release
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build of kb-speech failed (exit $LASTEXITCODE)"; exit 1 }
$speech = Join-Path $targetDir "release\kb-speech.exe"
if (-not (Test-Path $speech)) { Write-Error "missing $speech after the build"; exit 1 }
Copy-Item $speech (Join-Path $binaries "kb-speech-$Triple.exe") -Force

$libs = "sherpa-onnx-c-api.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll"
foreach ($name in $libs) {
  $lib = Join-Path $env:SHERPA_ONNX_LIB_DIR $name
  if (-not (Test-Path $lib)) { Write-Error "missing $lib (run scripts\fetch-speech-deps.ps1 -Force)"; exit 1 }
  Copy-Item $lib (Join-Path $binaries $name) -Force
}

$vadHash = (Get-FileHash $env:KB_SPEECH_VAD -Algorithm SHA256).Hash.ToLower()
if ($vadHash -ne "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6") {
  Write-Error "$($env:KB_SPEECH_VAD) is not the pinned silero_vad.onnx (SHA-256 $vadHash)"
  exit 1
}
Copy-Item $env:KB_SPEECH_VAD (Join-Path $binaries "silero_vad.onnx") -Force

if (-not (Test-Path (Join-Path $Root "apps\desktop\src-tauri\THIRD-PARTY-NOTICES.txt"))) {
  Write-Error "missing apps\desktop\src-tauri\THIRD-PARTY-NOTICES.txt (is it committed?)"
  exit 1
}

# The staged sidecar and DLLs, alone in a folder of their own as they will be in the install folder, must load the
# libraries: the version line is the one the build's own copy prints.
$probe = Join-Path ([IO.Path]::GetTempPath()) ("kb-speech-probe-{0}" -f $PID)
New-Item -ItemType Directory -Force $probe | Out-Null
try {
  Copy-Item (Join-Path $binaries "kb-speech-$Triple.exe") (Join-Path $probe "kb-speech.exe")
  foreach ($name in $libs) { Copy-Item (Join-Path $binaries $name) $probe }
  $probed = (& (Join-Path $probe "kb-speech.exe") --version | Out-String).Trim()
  $probedCode = $LASTEXITCODE
  $built = (& $speech --version | Out-String).Trim()
  if ($probedCode -ne 0 -or -not $probed -or $probed -ne $built) {
    Write-Error "the staged kb-speech.exe does not load its DLLs from its own folder (got '$probed', expected '$built')"
    exit 1
  }
  Write-Host "Staged dictation sidecar: $probed"
} finally {
  Remove-Item $probe -Recurse -Force -ErrorAction SilentlyContinue
}
exit 0
