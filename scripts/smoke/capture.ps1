# Dot-source: defines Capture-Window (PrintWindow works from tool shells where CopyFromScreen is black).
Add-Type -AssemblyName System.Drawing
if (-not ("Win" -as [type])) {
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
}
function Capture-Window([IntPtr]$hwnd, [string]$path) {
  $rect = New-Object Win+RECT
  [Win]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
  $w = $rect.Right - $rect.Left; $h = $rect.Bottom - $rect.Top
  if ($w -le 0 -or $h -le 0) { "window has no size"; return }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc()
  $ok = [Win]::PrintWindow($hwnd, $hdc, 2)
  $g.ReleaseHdc($hdc); $g.Dispose()
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
  "captured ok=$ok -> $path ($w x $h)"
}
