# Milestone 4 steps on the running desktop app (launch.ps1 first), through UI Automation and
# SendKeys; see README.md. The native file dialogs of the backup commands are driven too:
# they are top-level windows of the app process, found from the UIA root.
#
#   .\drive-m4.ps1 -Step kit       create the recovery kit from Settings, check three words
#   .\drive-m4.ps1 -Step lock      turn the app lock on, then "Lock now"
#   .\drive-m4.ps1 -Step unlock    on the lock screen: a wrong passphrase, then the right one
#   .\drive-m4.ps1 -Step reissue   re-issue the kit from Settings (rotates the root key), check three words
#   .\drive-m4.ps1 -Step content   title the first page "Smoke page" and type a line into it
#   .\drive-m4.ps1 -Step export    export a backup to out\m4-backup.atomikkb via the save dialog
#   .\drive-m4.ps1 -Step restore   on a fresh installation: restore that file with the kit
#   .\drive-m4.ps1 -Step verify    the restored page and search
#
# The kit words are written to out\kit.txt (git-ignored; a smoke-test secret of this
# machine) so that the restore step can type them back.
param([string]$Step = "kit", [int]$AppPid = 0)
$ErrorActionPreference = "Continue"
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
. "$PSScriptRoot\capture.ps1"
$out = Join-Path $PSScriptRoot "out"
New-Item -ItemType Directory -Force $out | Out-Null
$kitFile = "$out\kit.txt"
$backupFile = "$out\m4-backup.atomikkb"
$passphrase = "correct horse battery"
# The page drive.ps1 -Step page creates; the unlock, restore and verify steps look for it.
$pageTitle = "Smoke page"

# -AppPid picks one of several running instances (Milestone 6 two-device pass).
$app = if ($AppPid -ne 0) { Get-Process -Id $AppPid -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } } else { Get-Process -Name "atomik-kb-desktop" | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1 }
if (-not $app) { "app is not running (launch.ps1 first)"; exit 1 }
$hwnd = $app.MainWindowHandle
$root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
$desktop = [System.Windows.Automation.AutomationElement]::RootElement
$errLog = "$out\app.err.log"



function Web() {
  $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, "atomik-kb - Web content")
  return $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}
function All-El($parent) { return $parent.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition) }
function Find-El([string]$type, [string]$name, [int]$retries = 20, $parent = $null) {
  for ($i = 0; $i -lt $retries; $i++) {
    $where = if ($parent) { $parent } else { Web }
    if ($where) {
      foreach ($e in (All-El $where)) {
        $c = $e.Current
        if (($type -eq "" -or $c.ControlType.ProgrammaticName -eq "ControlType.$type") -and $c.Name -eq $name) { return $e }
      }
    }
    Start-Sleep -Milliseconds 400
  }
  return $null
}
function Find-Text([string]$name, [int]$retries = 20) { return Find-El "Text" $name $retries }
function Has-Text([string]$name, [int]$retries = 1) { return $null -ne (Find-El "" $name $retries) }
function Invoke-El($el) { $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() }
function Select-El($el) { $el.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select() }
function Set-El($el, [string]$text) { $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($text) }
# Keystrokes go through WScript.Shell: the .NET SendKeys journal hook is refused from a
# tool shell ("Access is denied").
$wsh = New-Object -ComObject WScript.Shell
function Keys([string]$keys) {
  [Win]::ShowWindow($hwnd, 9) | Out-Null
  [Win]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 300
  $wsh.SendKeys($keys)
}
# Fills a field through ValuePattern; when the field refuses it (some password fields do),
# focuses it and types through WScript.Shell (the .NET SendKeys journal hook is refused
# from a tool shell with "Access is denied").
function Type-Into($el, [string]$text) {
  try { Set-El $el $text; Start-Sleep -Milliseconds 200; return } catch { "  ValuePattern refused, typing instead" }
  [Win]::ShowWindow($hwnd, 9) | Out-Null
  [Win]::SetForegroundWindow($hwnd) | Out-Null
  $el.SetFocus()
  Start-Sleep -Milliseconds 300
  $wsh.SendKeys("^a{BACKSPACE}")
  $wsh.SendKeys($text)
  Start-Sleep -Milliseconds 200
}
function Dump([string]$filter = "") {
  foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.Name -and ($filter -eq "" -or $c.Name -like "*$filter*" -or $c.ControlType.ProgrammaticName -like "*$filter*")) { "  {0,-22} '{1}' id='{2}'" -f ($c.ControlType.ProgrammaticName -replace 'ControlType.',''), $c.Name, $c.AutomationId } }
}
function DocLog() { Get-Content $errLog | Select-String 'doc kind=|PageRename|panicked|ERROR|WARN' | Select-Object -Last 12 }
function SecLog() { Get-Content $errLog | Select-String 'security|keyring|recovery|lock|backup|rotation|rekey|panicked|ERROR|WARN' | Select-Object -Last 30 }
function Go-Settings() {
  $b = Find-El "Button" "Settings" 10
  if (-not $b) { "no Settings button"; Dump "Button"; exit 1 }
  Invoke-El $b
  Start-Sleep -Seconds 2
}
function Fail([string]$why) { "FAIL: $why"; Dump; SecLog; exit 1 }

# The native file dialogs are top-level #32770 windows of the app process, owned by the
# main window. UI Automation's root does not list them and their bottom controls are not
# in their accessibility tree, so they are driven through Win32: EnumWindows finds the
# dialog, the file-name edit is the visible Edit inside a ComboBox (id 1001 or 1148), its text is
# set with WM_SETTEXT and the default button (IDOK) is pressed with BM_CLICK.
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class DlgWins {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, string lParam);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr dlg, int id);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  public static IntPtr Find(uint pid, string title) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p != pid || !IsWindowVisible(h)) { return true; }
      var c = new StringBuilder(256); GetClassName(h, c, 256);
      var t = new StringBuilder(256); GetWindowText(h, t, 256);
      if (c.ToString() == "#32770" && t.ToString() == title) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hWnd);
  // The file-name box: the visible Edit inside a ComboBox (control id 1001 in the Save
  // dialog, 1148 in the Open dialog; the address bar's edit is invisible).
  public static IntPtr FileNameEdit(IntPtr dlg) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(dlg, (h, l) => {
      var c = new StringBuilder(256); GetClassName(h, c, 256);
      var pc = new StringBuilder(256); GetClassName(GetParent(h), pc, 256);
      int id = GetDlgCtrlID(h);
      if (c.ToString() == "Edit" && pc.ToString() == "ComboBox" && IsWindowVisible(h) && (id == 1001 || id == 1148)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static void SetText(IntPtr edit, string text) { SendMessage(edit, 0x000C, IntPtr.Zero, text); }
  public static void PressOk(IntPtr dlg) { IntPtr ok = GetDlgItem(dlg, 1); SendMessage(ok, 0x00F5, IntPtr.Zero, IntPtr.Zero); }
}
"@
function Find-Dialog([string]$name, [int]$retries = 40) {
  for ($i = 0; $i -lt $retries; $i++) {
    $h = [DlgWins]::Find([uint32]$app.Id, $name)
    if ($h -ne [IntPtr]::Zero) { return $h }
    Start-Sleep -Milliseconds 500
  }
  return [IntPtr]::Zero
}
# Types a path into the "File name:" box of a common dialog and presses its default button.
function Drive-FileDialog([string]$title, [string]$path) {
  $dlg = Find-Dialog $title
  if ($dlg -eq [IntPtr]::Zero) { Fail "no '$title' dialog appeared" }
  "dialog '$title' up (hwnd $dlg)"
  Start-Sleep -Milliseconds 800
  $edit = [DlgWins]::FileNameEdit($dlg)
  if ($edit -eq [IntPtr]::Zero) { Fail "no file-name edit in the '$title' dialog" }
  [DlgWins]::SetForegroundWindow($dlg) | Out-Null
  [DlgWins]::SetText($edit, $path)
  Start-Sleep -Milliseconds 500
  [DlgWins]::PressOk($dlg)
  Start-Sleep -Seconds 3
  if ((Find-Dialog $title 1) -ne [IntPtr]::Zero) { Fail "the '$title' dialog did not close (the path may have been refused)" }
}

# Typing without keystroke injection: characters are posted as WM_CHAR (Enter as a
# VK_RETURN key down/up pair) to the webview's browser window, the Chrome_WidgetWin_1 child
# of the Tauri window. SendKeys needs the foreground and can land in another window;
# posted messages always reach this app.
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class WebKeys {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
  public static IntPtr Browser(IntPtr root) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(root, (h, l) => { var c = new StringBuilder(256); GetClassName(h, c, 256); if (c.ToString() == "Chrome_WidgetWin_1") { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static void Text(IntPtr target, string text) {
    foreach (char ch in text) { PostMessage(target, 0x0102, (IntPtr)ch, (IntPtr)1); System.Threading.Thread.Sleep(15); }
  }
  public static void Enter(IntPtr target) {
    PostMessage(target, 0x0100, (IntPtr)0x0D, (IntPtr)0x001C0001);
    PostMessage(target, 0x0102, (IntPtr)0x0D, (IntPtr)0x001C0001);
    PostMessage(target, 0x0101, (IntPtr)0x0D, unchecked((IntPtr)(int)0xC01C0001));
  }
}
"@
function Post-Text([string]$text) {
  $target = [WebKeys]::Browser($hwnd)
  if ($target -eq [IntPtr]::Zero) { Fail "no webview browser window" }
  [WebKeys]::Text($target, $text)
}
function Post-Enter() { [WebKeys]::Enter([WebKeys]::Browser($hwnd)) }

# The 24 words shown in the kit dialog, read from the list's text nodes in order.
function Read-KitWords() {
  $list = Find-El "List" "Recovery kit words" 20
  if (-not $list) { Fail "no kit word list" }
  # WebView2 exposes each <li> as a ListItem named by the word (the number is CSS content).
  $words = @()
  foreach ($e in (All-El $list)) {
    $c = $e.Current
    if ($c.ControlType.ProgrammaticName -eq "ControlType.ListItem" -and $c.Name -match '^[a-z]+$') { $words += $c.Name }
  }
  return $words
}

switch ($Step) {
  "kit" {
    # In onboarding the kit step follows the workspace directly; in a running workspace
    # "Create now" in Settings opens the same explanation step. The kit is made on
    # "Create my recovery kit". A kit already on the screen (an earlier run stopped
    # there) is read as it is.
    $shown = Find-El "List" "Recovery kit words" 2
    $explain = if ($shown) { $null } else { Find-El "Button" "Create my recovery kit" 2 }
    if (-not $shown -and -not $explain) {
      Go-Settings
      $create = Find-El "Button" "Create now" 10
      if (-not $create) { Fail "no 'Create now' button (a kit may already exist)" }
      Invoke-El $create
      Start-Sleep -Seconds 2
      $explain = Find-El "Button" "Create my recovery kit" 10
    }
    if ($explain) {
      Capture-Window $hwnd "$out\m4-kit-explained.png"
      Invoke-El $explain
      Start-Sleep -Seconds 4
    } elseif (-not $shown) { Fail "no 'Create my recovery kit' button" }
    $words = Read-KitWords
    "kit words read: $($words.Count)"
    if ($words.Count -ne 24) { Fail "expected 24 words, got $($words.Count)" }
    Set-Content -Path $kitFile -Value ($words -join " ") -Encoding ascii
    Capture-Window $hwnd "$out\m4-kit.png"
    Invoke-El (Find-El "Button" "I saved my kit" 10)
    Start-Sleep -Seconds 1
    foreach ($e in (All-El (Web))) {
      $c = $e.Current
      if ($c.ControlType.ProgrammaticName -eq "ControlType.Edit" -and $c.Name -match '^Word (\d+)$') {
        $n = [int]$Matches[1]
        Set-El $e $words[$n - 1]
        "typed word $n"
      }
    }
    Start-Sleep -Milliseconds 500
    Invoke-El (Find-El "Button" "Check" 10)
    Start-Sleep -Seconds 3
    $done = Find-El "Button" "Done" 20
    if (-not $done) { Fail "the word check did not pass" }
    Capture-Window $hwnd "$out\m4-kit-checked.png"
    Invoke-El $done
    Start-Sleep -Seconds 2
    # From onboarding, Done lands on Home: the kit status is read in Settings either way.
    if (-not (Find-El "Text" "Verified" 2)) { Go-Settings }
    if (-not (Has-Text "Verified" 10)) { Fail "Settings does not show the kit as verified" }
    if (Has-Text "Not verified yet") { Fail "the kit is still 'Not verified yet'" }
    "kit created and verified"
    Capture-Window $hwnd "$out\m4-settings-kit.png"
    SecLog
  }
  "lock" {
    Go-Settings
    # The form may already be open from an earlier run.
    if (-not (Find-El "Edit" "Repeat the passphrase" 2)) {
      $enable = Find-El "Button" "Turn on app lock..." 10
      if (-not $enable) { Fail "no 'Turn on app lock...' button" }
      Invoke-El $enable
      Start-Sleep -Seconds 1
    }
    Type-Into (Find-El "Edit" "Passphrase" 10) $passphrase
    Type-Into (Find-El "Edit" "Repeat the passphrase" 10) $passphrase
    Invoke-El (Find-El "Button" "Turn on app lock" 10)
    Start-Sleep -Seconds 6
    if (-not (Has-Text "App lock is on." 20)) { Fail "the app lock did not turn on" }
    "app lock on"
    Capture-Window $hwnd "$out\m4-lock-on.png"
    Invoke-El (Find-El "Button" "Lock now" 10)
    Start-Sleep -Seconds 3
    if (-not (Find-El "Button" "Unlock" 20)) { Fail "no lock screen after Lock now" }
    if (Find-El "TreeItem" "" 1) { Fail "tree items are still on the screen while locked" }
    "locked: lock screen shown, nothing from the workspace left"
    Capture-Window $hwnd "$out\m4-locked.png"
    SecLog
  }
  "unlock" {
    $field = Find-El "Edit" "Passphrase" 30
    if (-not $field) { Fail "no passphrase field (not on the lock screen)" }
    Type-Into $field "not the passphrase"
    Invoke-El (Find-El "Button" "Unlock" 10)
    Start-Sleep -Seconds 6
    if (-not (Has-Text "That passphrase is not right. Try again." 20)) { Fail "a wrong passphrase was not refused" }
    "wrong passphrase refused"
    Capture-Window $hwnd "$out\m4-unlock-wrong.png"
    Type-Into (Find-El "Edit" "Passphrase" 10) $passphrase
    Invoke-El (Find-El "Button" "Unlock" 10)
    Start-Sleep -Seconds 6
    if (-not (Find-El "Button" "New page" 30)) { Fail "the workspace did not come back after unlocking" }
    "unlocked: tree is back"
    Capture-Window $hwnd "$out\m4-unlocked.png"
    SecLog
  }
  "export" {
    if (Test-Path $backupFile) { Remove-Item $backupFile }
    # The dialog may already be open from an earlier run.
    if ((Find-Dialog "Save As" 1) -eq [IntPtr]::Zero) {
      Go-Settings
      $export = Find-El "Button" "Export backup..." 10
      if (-not $export) { Fail "no 'Export backup...' button" }
      Invoke-El $export
    }
    Drive-FileDialog "Save As" $backupFile
    if (-not (Has-Text "Backup saved." 30)) { Fail "no 'Backup saved.' after the dialog" }
    if (-not (Test-Path $backupFile)) { Fail "the backup file does not exist" }
    "backup written: $((Get-Item $backupFile).Length) bytes"
    Capture-Window $hwnd "$out\m4-exported.png"
    SecLog
  }
  "restore" {
    if (-not (Test-Path $kitFile)) { Fail "no $kitFile (run the kit step first)" }
    # The Open dialog may already be up from an earlier run (the button then reads "Restoring...").
    if ((Find-Dialog "Open" 1) -eq [IntPtr]::Zero) {
      $restore = Find-El "Button" "Restore from a backup" 30
      if (-not $restore) { Fail "no 'Restore from a backup' button (not on onboarding)" }
      Invoke-El $restore
      # ADR-0059: the button opens the restore wizard; its first source is the file dialog of before.
      $fileSource = Find-El "Button" "A backup file$([char]0x2026)" 15
      if (-not $fileSource) { Fail "no 'A backup file...' source in the restore wizard" }
      Invoke-El $fileSource
    }
    Drive-FileDialog "Open" $backupFile
    $kitField = Find-El "Edit" "Recovery kit" 30
    if (-not $kitField) { $kitField = Find-El "Document" "Recovery kit" 5 }
    if (-not $kitField) { Fail "no kit entry after choosing the file" }
    "kit entry shown"
    Capture-Window $hwnd "$out\m4-restore-kit.png"
    Set-El $kitField (Get-Content $kitFile -Raw).Trim()
    Start-Sleep -Milliseconds 800
    Invoke-El (Find-El "Button" "Restore" 10)
    Start-Sleep -Seconds 8
    if (-not (Find-El "TreeItem" $pageTitle 40)) { Fail "the restored workspace did not appear" }
    "restored: tree is back"
    Capture-Window $hwnd "$out\m4-restored.png"
    SecLog
  }
  "reissue" {
    # Re-issues the kit from Settings (with the passphrase when the lock is on), saves the
    # new words to out\kit.txt and checks three of them. The database is rekeyed underneath;
    # a restart afterwards proves the promoted key opens it.
    Go-Settings
    $button = Find-El "Button" "Re-issue..." 10
    if (-not $button) { Fail "no 'Re-issue...' button" }
    Invoke-El $button
    Start-Sleep -Seconds 1
    $pass = Find-El "Edit" "App lock passphrase" 3
    if ($pass) { Type-Into $pass $passphrase; "passphrase given" }
    Invoke-El (Find-El "Button" "Re-issue kit" 10)
    Start-Sleep -Seconds 8
    if (-not (Find-El "List" "Recovery kit words" 30)) { Fail "no new kit shown after re-issue" }
    $words = Read-KitWords
    if ($words.Count -ne 24) { Fail "expected 24 words, got $($words.Count)" }
    $old = if (Test-Path $kitFile) { (Get-Content $kitFile -Raw).Trim() } else { "" }
    if (($words -join " ") -eq $old) { Fail "the re-issued kit equals the old one" }
    Set-Content -Path $kitFile -Value ($words -join " ") -Encoding ascii
    Capture-Window $hwnd "$out\m4-reissued.png"
    Invoke-El (Find-El "Button" "I saved my kit" 10)
    Start-Sleep -Seconds 1
    foreach ($e in (All-El (Web))) {
      $c = $e.Current
      if ($c.ControlType.ProgrammaticName -eq "ControlType.Edit" -and $c.Name -match '^Word (\d+)$') { Set-El $e $words[[int]$Matches[1] - 1] }
    }
    Invoke-El (Find-El "Button" "Check" 10)
    Start-Sleep -Seconds 3
    $done = Find-El "Button" "Done" 20
    if (-not $done) { Fail "the word check did not pass after re-issue" }
    Invoke-El $done
    Start-Sleep -Seconds 2
    if (-not (Has-Text "Re-issued" 10)) { Fail "Settings does not show the kit as re-issued" }
    "kit re-issued and checked"
    SecLog
  }
  "content" {
    # Gives the first untitled page a title and a line of text (the restore step looks
    # for both). The title is a button that turns into a field when invoked.
    $item = Find-El "TreeItem" "Untitled" 10
    if (-not $item) { $item = Find-El "TreeItem" $pageTitle 5 }
    if (-not $item) { Fail "no page in the tree" }
    Select-El $item
    Start-Sleep -Seconds 2
    $titleButton = Find-El "Button" "Untitled" 5
    if ($titleButton) {
      Invoke-El $titleButton
      Start-Sleep -Milliseconds 800
      $field = $null
      foreach ($e in (All-El (Web))) { $c = $e.Current; if ($c.ControlType.ProgrammaticName -eq "ControlType.Edit" -and $c.HasKeyboardFocus) { $field = $e; break } }
      if (-not $field) { Fail "no title field after invoking the title" }
      try { Set-El $field $pageTitle } catch { Post-Text $pageTitle }
      Start-Sleep -Milliseconds 300
      Post-Enter
      Start-Sleep -Seconds 1
    }
    if (-not (Find-El "TreeItem" $pageTitle 10)) { Fail "the page was not renamed to '$pageTitle'" }
    "page titled"
    $editor = Find-El "Edit" "Page content" 10
    if (-not $editor) { Fail "no page content editor" }
    $editor.SetFocus()
    Start-Sleep -Milliseconds 500
    Post-Text "Hello from the smoke test."
    Start-Sleep -Seconds 2
    if (-not (Has-Text "Hello from the smoke test." 10)) { Fail "the typed text did not appear in the editor" }
    "content typed"
    Capture-Window $hwnd "$out\m4-content.png"
    Start-Sleep -Seconds 1
    DocLog
  }
  "verify" {
    $item = Find-El "TreeItem" $pageTitle 10
    if (-not $item) { Fail "no '$pageTitle' in the tree" }
    Select-El $item
    Start-Sleep -Seconds 3
    if (-not (Has-Text "Hello from the smoke test." 10)) { Fail "the page content did not come back" }
    "page content present"
    Capture-Window $hwnd "$out\m4-restored-page.png"
    # Search through its button and posted characters (no keystroke injection).
    Invoke-El (Find-El "Button" "Search" 10)
    Start-Sleep -Milliseconds 800
    # The search box is a combobox-role input named "Search" inside the Search dialog.
    $box = Find-El "ComboBox" "Search" 10
    if (-not $box) { Fail "no search box" }
    $box.SetFocus()
    Start-Sleep -Milliseconds 300
    try { Set-El $box "smoke" } catch { Post-Text "smoke" }
    Start-Sleep -Seconds 2
    Capture-Window $hwnd "$out\m4-restored-search.png"
    # Results live inside the Search dialog: the page title and the matching line.
    $dialog = Find-El "Window" "Search" 5
    if (-not $dialog) { Fail "the search dialog is not open" }
    $hits = @()
    foreach ($e in (All-El $dialog)) { $c = $e.Current; if ($c.Name -eq "Hello from the smoke test." -or $c.Name -eq $pageTitle) { $hits += $c.Name } }
    if ($hits.Count -eq 0) { Fail "search found neither the title nor the content for 'smoke'" }
    "search hits: $(($hits | Select-Object -Unique) -join '; ')"
    SecLog
  }
  default { "unknown step $Step" }
}
