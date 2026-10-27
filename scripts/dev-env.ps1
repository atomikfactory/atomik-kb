# Dot-source at the start of a PowerShell command on this development machine:
#   . .\scripts\dev-env.ps1; cargo build -p kb-core
#
# 1. Puts the per-user toolchains on PATH (shells started before the install do not
#    see them): rustup/cargo, portable Strawberry Perl and NASM (needed to build the
#    vendored OpenSSL that SQLCipher links), pnpm.
# 2. If the loopback IPv4-only proxy from scripts/dev-ipv4-proxy.mjs is listening,
#    routes cargo and rustup downloads through it. This machine has no IPv6
#    connectivity but receives AAAA records, and dual-stack connects fail with
#    os error 10013; the proxy only ever dials IPv4. Nothing else is proxied.

$progs = Join-Path $env:LOCALAPPDATA "Programs"
$env:Path = "$env:USERPROFILE\.cargo\bin;$progs\StrawberryPerl\perl\bin;$progs\StrawberryPerl\c\bin;$progs\NASM;$env:APPDATA\npm;$env:Path"

# A plain socket probe with a 200 ms cap: Get-NetTCPConnection costs seconds in Windows PowerShell.
$proxyListening = $false
try {
  $probe = New-Object System.Net.Sockets.TcpClient
  $proxyListening = $probe.ConnectAsync("127.0.0.1", 3128).Wait(200) -and $probe.Connected
  $probe.Close()
} catch { }
if ($proxyListening) {
  $env:CARGO_HTTP_PROXY = "http://127.0.0.1:3128"
  $env:RUSTUP_HTTPS_PROXY = "http://127.0.0.1:3128"
} else {
  Write-Warning "IPv4 proxy not running; if cargo/rustup downloads fail with os error 10013, start it: Start-Process node -ArgumentList 'scripts\dev-ipv4-proxy.mjs','3128' -WindowStyle Hidden"
}

# 3. Dictation: where scripts/fetch-speech-deps.ps1 put the sherpa-onnx native libraries and the
#    Silero voice activity model, when it has run. SHERPA_ONNX_LIB_DIR makes the build of kb-speech-native use
#    them instead of downloading them (and copies the DLLs next to the executables in target\<profile>);
#    KB_SPEECH_VAD is where a debug build of the shell looks for the voice activity model. Nothing is set
#    when they are not there.
$speechRoot = Join-Path (Split-Path -Parent $PSScriptRoot) ".build\speech"
$sherpaLib = Join-Path $speechRoot "sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib\lib"
if (Test-Path -LiteralPath (Join-Path $sherpaLib "sherpa-onnx-c-api.dll")) {
  $env:SHERPA_ONNX_LIB_DIR = $sherpaLib
}
$speechVad = Join-Path $speechRoot "silero_vad.onnx"
if (Test-Path -LiteralPath $speechVad) {
  $env:KB_SPEECH_VAD = $speechVad
}
