# One-command Windows release build: an unsigned per-user NSIS installer.
#
#   .\scripts\package\windows.ps1                 # version 0.1.<commit count>
#   .\scripts\package\windows.ps1 -Version 0.2.0  # explicit version
#
# Runs `tauri build` for apps/desktop (its beforeBuildCommand builds the frontend), which
# merges apps/desktop/src-tauri/tauri.windows.conf.json over tauri.conf.json. The version
# is passed at build time through a temporary --config file, so no tracked file changes. The MCP
# stdio bridge (kb-mcp-bridge.exe) and the dictation sidecar (kb-speech.exe, ADR-0053) are built first and bundled next to
# the app as sidecars through tauri.bridge.conf.json (see below); the sherpa-onnx DLLs, the voice activity model and the
# third-party notices are bundled as resources, which NSIS installs next to the executables.
# Copies the installer to dist/installers/ (git-ignored) and prints its path, size, SHA-256.
# The installer also registers the atomik-kb:// URL scheme for the user (HKCU\Software\Classes\atomik-kb, written from
# plugins.deep-link in tauri.conf.json; ADR-0040, docs/development/packaging.md): a link opens the app at the page.
# The first run compiles SQLCipher and OpenSSL from source (several minutes); let it finish.
#
# -Root   tree to build (default: this repo). auto-build.ps1 passes its build worktree.
# -OutDir where installers are copied (default: <this repo>\dist\installers).
# A dirty tree gets a "-dirty" file-name suffix (the embedded semver is unchanged).

[CmdletBinding()]
param(
  [string]$Version,
  [string]$Root,
  [string]$OutDir
)

$ErrorActionPreference = "Continue"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $Root) { $Root = $repo }
$Root = (Resolve-Path $Root).Path
if (-not $OutDir) { $OutDir = Join-Path $repo "dist\installers" }
Set-Location $Root

# The sherpa-onnx native libraries and the Silero voice activity model of dictation (ADR-0053), fetched once into
# .build/speech of this repo (idempotent, SHA-256 checked). dev-env.ps1 then sets SHERPA_ONNX_LIB_DIR and KB_SPEECH_VAD.
try { & (Join-Path $repo "scripts\fetch-speech-deps.ps1") } catch { Write-Error "fetch-speech-deps.ps1 failed: $($_.Exception.Message)"; exit 1 }
. (Join-Path $repo "scripts\dev-env.ps1")
if (-not $env:SHERPA_ONNX_LIB_DIR -or -not $env:KB_SPEECH_VAD) { Write-Error "SHERPA_ONNX_LIB_DIR and KB_SPEECH_VAD are not set: run scripts\fetch-speech-deps.ps1"; exit 1 }

if (-not $Version) {
  $count = (git -C $Root rev-list --count HEAD).Trim()
  if (-not $count) { Write-Error "cannot count commits; pass -Version"; exit 1 }
  $Version = "0.1.$count"
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') { Write-Error "version must be MAJOR.MINOR.PATCH (got '$Version')"; exit 1 }

$dirty = [bool](git -C $Root status --porcelain --untracked-files=no)
$desktop = Join-Path $Root "apps\desktop"
$winConf = Join-Path $desktop "src-tauri\tauri.windows.conf.json"
if (-not (Test-Path $winConf)) { Write-Error "missing $winConf (is it committed?)"; exit 1 }
$bundleDir = Join-Path $Root "target\release\bundle\nsis"

# Remove stale installers so the one we pick up is the one this run built.
if (Test-Path $bundleDir) { Get-ChildItem $bundleDir -Filter "*-setup.exe" | Remove-Item -Force }

# The stdio bridge of the MCP server (ADR-0037) ships next to the app. It is built here and copied where
# tauri expects a sidecar (binaries/kb-mcp-bridge-<target triple>.exe, git-ignored); the merge file
# tauri.bridge.conf.json (passed below) is what adds bundle.externalBin, and it is NOT in tauri.conf.json:
# tauri-build checks the sidecar's path at compile time, which would break every dev build and test run.
$triple = ((rustc -vV | Select-String -Pattern '^host:').Line -replace '^host:\s*', '').Trim()
if (-not $triple) { Write-Error "cannot read the host target triple from rustc -vV"; exit 1 }
Write-Host "Building kb-mcp-bridge ($triple)"
cargo build -p kb-mcp --bin kb-mcp-bridge --release
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build of kb-mcp-bridge failed (exit $LASTEXITCODE)"; exit 1 }
$targetDir = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
$bridge = Join-Path $targetDir "release\kb-mcp-bridge.exe"
if (-not (Test-Path $bridge)) { Write-Error "missing $bridge after the build"; exit 1 }
$binaries = Join-Path $Root "apps\desktop\src-tauri\binaries"
New-Item -ItemType Directory -Force $binaries | Out-Null
Copy-Item $bridge (Join-Path $binaries "kb-mcp-bridge-$triple.exe") -Force
$bridgeConf = Join-Path $Root "apps\desktop\src-tauri\tauri.bridge.conf.json"
if (-not (Test-Path $bridgeConf)) { Write-Error "missing $bridgeConf (is it committed?)"; exit 1 }

# The dictation sidecar (ADR-0053, docs/architecture/dictation.md section 7) ships the same way, with what it loads:
# stage-speech.ps1 builds kb-speech.exe, copies it to binaries/kb-speech-<triple>.exe and stages the sherpa-onnx DLLs and
# silero_vad.onnx in the same folder; tauri.bridge.conf.json adds the sidecar to bundle.externalBin and the staged files
# (and THIRD-PARTY-NOTICES.txt) to bundle.resources, so that NSIS installs them next to the executables.
& (Join-Path $PSScriptRoot "stage-speech.ps1") -Root $Root -Triple $triple
if ($LASTEXITCODE -ne 0) { Write-Error "staging the dictation sidecar failed (exit $LASTEXITCODE)"; exit 1 }

# Version override as a temp JSON file (quoting inline JSON is fragile in PowerShell 5.1).
$cfg = Join-Path ([IO.Path]::GetTempPath()) ("kb-tauri-version-{0}.json" -f $PID)
[IO.File]::WriteAllText($cfg, ('{{"version":"{0}"}}' -f $Version), (New-Object Text.UTF8Encoding($false)))
Write-Host "Building atomik-kb $Version from $Root (dirty: $dirty)"

Set-Location $desktop
try {
  pnpm tauri build --ci --config $cfg --config $bridgeConf
  $code = $LASTEXITCODE
} finally {
  Remove-Item $cfg -Force -ErrorAction SilentlyContinue
  Set-Location $Root
}
if ($code -ne 0) { Write-Error "tauri build failed (exit $code)"; exit 1 }

$built = Get-ChildItem $bundleDir -Filter "*-setup.exe" -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $built) { Write-Error "no *-setup.exe found in $bundleDir"; exit 1 }

New-Item -ItemType Directory -Force $OutDir | Out-Null
$name = $built.Name
if ($dirty) { $name = $name -replace '_x64-setup\.exe$', '-dirty_x64-setup.exe' }
$final = Join-Path $OutDir $name
Copy-Item $built.FullName $final -Force

$hash = (Get-FileHash $final -Algorithm SHA256).Hash.ToLower()
Write-Host ""
Write-Host "Installer: $final"
Write-Host ("Size:      {0:N0} bytes ({1:N1} MB)" -f (Get-Item $final).Length, ((Get-Item $final).Length / 1MB))
Write-Host "SHA-256:   $hash"
if ($dirty) { Write-Warning "working tree had uncommitted changes to tracked files; installer built from them (file marked -dirty)" }
