# Makes our indirect display the only display. Runs at logon in the user's session, as Windows only lets a session change this.
# It does nothing once that's so: Windows remembers the arrangement, for this set of monitors, in its own database.
# Usage: take-over-display.ps1 WIDTH HEIGHT HZ LOG
param([int]$Width = 1920, [int]$Height = 1080, [int]$Hz = 60, [string]$Log = "$env:ProgramData\Guesthouse\display.log")
$ourName = 'RDPUDD Chained DD'
$tries = 30
$pauseSeconds = 1
$attachedToDesktop = 1
$primaryDevice = 4

$source = @'
using System; using System.Runtime.InteropServices;
public class Screens {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DM {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Name; public short a, b, Size, Extra;
    public int Fields, x, y, Orient, Fixed; public short c1, c2, c3, c4, c5;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Form; public short LogPix, Bits;
    public int W, H, DFlags, Freq, i1, i2, i3, i4, i5, i6, i7, i8; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DD {
    public int cb; [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Name;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Str; public int Flags;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Id;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Key; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplayDevices(string d, uint i, ref DD dd, uint f);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplaySettings(string d, int mode, ref DM dm);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int ChangeDisplaySettingsEx(string d, ref DM dm, IntPtr w, uint flags, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int ChangeDisplaySettingsEx(string d, IntPtr dm, IntPtr w, uint flags, IntPtr p);
}
'@
Add-Type $source
New-Item -ItemType Directory -Force (Split-Path $Log) | Out-Null
function Say($text) { "$(Get-Date -Format s) $text" | Out-File $Log -Append -Encoding ascii }

function All-Displays {
    for ($i = 0; $i -lt 16; $i++) {
        $device = New-Object Screens+DD
        $device.cb = [Runtime.InteropServices.Marshal]::SizeOf($device)
        if (-not [Screens]::EnumDisplayDevices([NullString]::Value, $i, [ref]$device, 0)) { break }
        $device
    }
}

$ours = $null
for ($try = 0; $try -lt $tries -and -not $ours; $try++) {
    $ours = All-Displays | Where-Object { $_.Str -eq $ourName } | Select-Object -First 1
    if (-not $ours) { Start-Sleep $pauseSeconds }
}
if (-not $ours) { Say 'our display never came up, leaving things as they are'; return }

$others = @(All-Displays | Where-Object { $_.Str -ne $ourName -and ($_.Flags -band $attachedToDesktop) })
if (($ours.Flags -band $primaryDevice) -and $others.Count -eq 0) { Say "$($ours.Name) is already the only display"; return }

$size = [Runtime.InteropServices.Marshal]::SizeOf([type][Screens+DM])
$mode = New-Object Screens+DM
$mode.Size = $size
[void][Screens]::EnumDisplaySettings($ours.Name, -1, [ref]$mode)
$mode.W = $Width; $mode.H = $Height; $mode.Freq = $Hz; $mode.x = 0; $mode.y = 0
$sizeRateAndPosition = 0x580020
$updateRegistryPrimaryNoReset = 0x10000011
Say "making $($ours.Name) primary: $([Screens]::ChangeDisplaySettingsEx($ours.Name, [ref]$mode, [IntPtr]::Zero, $updateRegistryPrimaryNoReset, [IntPtr]::Zero))"

# Both in one change: done apart, Windows drops ours to 800x600 at 30, where a mode change fails.
$updateRegistryNoReset = 0x10000001
$sizeAndPosition = 0x180020
foreach ($other in $others) {
    $off = New-Object Screens+DM
    $off.Size = $size
    $off.Fields = $sizeAndPosition
    Say "turning off $($other.Name): $([Screens]::ChangeDisplaySettingsEx($other.Name, [ref]$off, [IntPtr]::Zero, $updateRegistryNoReset, [IntPtr]::Zero))"
}
Say "applying: $([Screens]::ChangeDisplaySettingsEx([NullString]::Value, [IntPtr]::Zero, [IntPtr]::Zero, 0, [IntPtr]::Zero))"
