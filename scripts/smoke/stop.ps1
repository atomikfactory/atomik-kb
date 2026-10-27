# Stops the app and the `pnpm tauri dev` tree started by launch.ps1. Matches command lines
# of the launcher cmd, the tauri CLI / vite node processes and the cargo run; never the
# repository path alone (the calling shell has it in its own command line).
$ErrorActionPreference = "Continue"
Get-Process -Name "atomik-kb-desktop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
$procs = Get-CimInstance Win32_Process | Where-Object {
  $_.CommandLine -and (
    ($_.Name -eq "cmd.exe" -and $_.CommandLine -match "pnpm.{0,40}tauri.{0,10}dev") -or
    ($_.Name -eq "node.exe" -and $_.CommandLine -match "tauri|vite") -or
    ($_.Name -eq "cargo.exe" -and $_.CommandLine -match "atomik-kb-desktop|src-tauri")
  )
}
foreach ($p in $procs) { "stopping $($p.Name) $($p.ProcessId)"; Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
Start-Sleep -Seconds 2
"app running: $([bool](Get-Process -Name 'atomik-kb-desktop' -ErrorAction SilentlyContinue)); vite listening: $([bool](Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue))"
