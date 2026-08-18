param([int]$X = 0, [int]$Y = 0)

Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Ui {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

$proc = Get-Process -Name clippiboy -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "ClippiBoy-Fenster nicht gefunden"; exit 1 }
$hwnd = $proc.MainWindowHandle

[void][Ui]::ShowWindow($hwnd, 9)
[void][Ui]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 500

$r = New-Object Ui+RECT
[void][Ui]::GetWindowRect($hwnd, [ref]$r)
[void][Ui]::SetCursorPos(($r.Left + $X), ($r.Top + $Y))
Start-Sleep -Milliseconds 150
[Ui]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60
[Ui]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Write-Output "Klick bei $($r.Left + $X),$($r.Top + $Y)"
