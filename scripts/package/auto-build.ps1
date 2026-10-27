# Rebuild the Windows installer from the committed HEAD. Started by .githooks/post-commit
# (detached, hidden) or by hand:  .\scripts\package\auto-build.ps1 [-Install]
#
# - Single instance: a lock (.build\lock, holds the PID). A second trigger while a build runs
#   writes .build\pending and exits; the running build loops once more so the last commit is built.
# - Builds from a persistent detached worktree (.build\installer, its own target\ for fast
#   incremental builds), never from the dirty working tree.
# - Output: dist\installers\atomik-kb_0.1.<N>_x64-setup.exe + atomik-kb-latest-setup.exe,
#   newest 5 versioned installers kept, log in dist\installers\auto-build.log.
# - -Install runs the new installer silently (/S) unless atomik-kb is running.

[CmdletBinding()]
param([switch]$Install, [int]$Keep = 5)

$ErrorActionPreference = "Continue"
# Started from a hook, git's per-repository variables may be inherited; git -C <worktree> would
# then use the main checkout's index. The hook unsets them too.
foreach ($v in "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY") {
  Remove-Item "Env:$v" -ErrorAction SilentlyContinue
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$buildRoot = Join-Path $repo ".build"
$wt = Join-Path $buildRoot "installer"
$lock = Join-Path $buildRoot "lock"
$pending = Join-Path $buildRoot "pending"
$outDir = Join-Path $repo "dist\installers"
$log = Join-Path $outDir "auto-build.log"
New-Item -ItemType Directory -Force $buildRoot, $outDir | Out-Null

function Log($m) { Add-Content -Path $log -Value ("{0} {1}" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $m) }

function Notify($title, $text) {
  try {
    [void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
    [void][Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
    $t = [Security.SecurityElement]::Escape($title); $b = [Security.SecurityElement]::Escape($text)
    $xml.LoadXml("<toast><visual><binding template=""ToastGeneric""><text>$t</text><text>$b</text></binding></visual></toast>")
    $appId = "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe"
    [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show(
      (New-Object Windows.UI.Notifications.ToastNotification $xml))
  } catch { Log "toast failed: $($_.Exception.Message)" }
}

function Test-LockAlive {
  if (-not (Test-Path $lock)) { return $false }
  $p = (Get-Content $lock -ErrorAction SilentlyContinue | Select-Object -First 1)
  if ($p -notmatch '^\d+$') { return $false }
  return [bool](Get-Process -Id ([int]$p) -ErrorAction SilentlyContinue)
}

function Build-One {
  $sha = (git -C $repo rev-parse HEAD).Trim()
  $short = $sha.Substring(0, 7)
  $count = (git -C $repo rev-list --count $sha).Trim()
  $ver = "0.1.$count"
  $sw = [Diagnostics.Stopwatch]::StartNew()
  Log "START $ver commit $short"
  try {
    if (-not (Test-Path (Join-Path $wt ".git"))) {
      git -C $repo worktree prune
      git -C $repo worktree add --detach --force $wt $sha 2>&1 | ForEach-Object { Log "git: $_" }
    } else {
      git -C $wt checkout --detach --force $sha 2>&1 | ForEach-Object { Log "git: $_" }
    }
    git -C $wt clean -fd -e target -e node_modules -e "*/node_modules" -e "*/dist" 2>&1 | Out-Null
    if ((git -C $wt rev-parse HEAD).Trim() -ne $sha) { throw "worktree not at $short" }

    # The IPv4 download proxy must be up before dev-env.ps1 decides to use it.
    if (-not (Get-NetTCPConnection -LocalPort 3128 -State Listen -ErrorAction SilentlyContinue)) {
      Start-Process node -ArgumentList ('"' + (Join-Path $repo "scripts\dev-ipv4-proxy.mjs") + '"'), "3128" -WindowStyle Hidden
      Start-Sleep -Seconds 2
    }
    . (Join-Path $repo "scripts\dev-env.ps1")
    Push-Location $wt
    pnpm install --frozen-lockfile --prefer-offline 2>&1 | ForEach-Object { Log "pnpm: $_" }
    $pc = $LASTEXITCODE
    Pop-Location
    if ($pc -ne 0) { throw "pnpm install failed ($pc)" }

    # Use this repo's windows.ps1 (the committed one may predate it) on the worktree tree.
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo "scripts\package\windows.ps1") `
      -Root $wt -Version $ver -OutDir $outDir 2>&1 | ForEach-Object { Log "build: $_" }
    $installer = Join-Path $outDir "atomik-kb_${ver}_x64-setup.exe"
    if (-not (Test-Path $installer)) { throw "installer $installer not produced" }
    Copy-Item $installer (Join-Path $outDir "atomik-kb-latest-setup.exe") -Force

    Get-ChildItem $outDir -Filter "atomik-kb_*_x64-setup.exe" | Where-Object { $_.Name -notmatch 'dirty' } |
      Sort-Object { [version]($_.Name -replace '^atomik-kb_(\d+\.\d+\.\d+)_.*$', '$1') } -Descending |
      Select-Object -Skip $Keep | Remove-Item -Force

    Log ("OK    {0} commit {1} in {2:N0}s -> {3}" -f $ver, $short, $sw.Elapsed.TotalSeconds, $installer)
    Notify "atomik-kb $ver installer ready" $installer
    if ($Install) {
      if (Get-Process -Name "atomik-kb" -ErrorAction SilentlyContinue) {
        Log "install skipped: atomik-kb is running (close it and run $installer)"
      } else {
        $p = Start-Process -FilePath $installer -ArgumentList "/S" -Wait -PassThru
        Log "install exit code $($p.ExitCode)"
      }
    }
  } catch {
    Log ("FAIL  {0} commit {1} after {2:N0}s: {3}" -f $ver, $short, $sw.Elapsed.TotalSeconds, $_.Exception.Message)
    Notify "atomik-kb $ver installer FAILED" "See $log"
  }
}

while ($true) {
  if (Test-LockAlive) { Set-Content $pending "1"; Log "build running; marked pending"; exit 0 }
  Set-Content $lock $PID
  try {
    do {
      Remove-Item $pending -Force -ErrorAction SilentlyContinue
      Build-One
    } while (Test-Path $pending)
  } finally {
    Remove-Item $lock -Force -ErrorAction SilentlyContinue
  }
  # A trigger that arrived between the last check and the unlock.
  if (-not (Test-Path $pending)) { break }
}
