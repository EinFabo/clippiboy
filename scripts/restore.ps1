Add-Type @'
using System;
using System.Runtime.InteropServices;
public class R { [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd); }
'@
$p = Get-Process -Name clippiboy -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if ($p) { [void][R]::ShowWindow($p.MainWindowHandle, 3); Write-Output "Fenster maximiert" } else { Write-Output "kein Fenster" }
