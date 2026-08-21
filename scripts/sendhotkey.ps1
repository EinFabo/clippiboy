param([string]$Key = "B")

Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Keyb {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
'@

$VK_CONTROL = 0x11
$VK_SHIFT   = 0x10
$vk = [byte][char]$Key
$KEYUP = 0x0002

[Keyb]::keybd_event($VK_CONTROL, 0, 0, [UIntPtr]::Zero)
[Keyb]::keybd_event($VK_SHIFT, 0, 0, [UIntPtr]::Zero)
[Keyb]::keybd_event($vk, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60
[Keyb]::keybd_event($vk, 0, $KEYUP, [UIntPtr]::Zero)
[Keyb]::keybd_event($VK_SHIFT, 0, $KEYUP, [UIntPtr]::Zero)
[Keyb]::keybd_event($VK_CONTROL, 0, $KEYUP, [UIntPtr]::Zero)
Write-Output "sent Ctrl+Shift+$Key"
