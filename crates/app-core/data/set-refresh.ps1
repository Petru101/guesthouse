# Sets the desktop's refresh rate, keeping its size. The host puts $wanted, in Hz, first. Runs in the
# user's session, as only that can change a display. A display with no mode at this rate stays as it is.
$source = @'
using System; using System.Runtime.InteropServices;
public class Rate {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DM {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Name; public short a, b, Size, Extra;
    public int Fields, x, y, Orient, Fixed; public short c1, c2, c3, c4, c5;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Form; public short LogPix, Bits;
    public int W, H, DFlags, Freq, i1, i2, i3, i4, i5, i6, i7, i8; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplaySettings(string d, int mode, ref DM dm);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int ChangeDisplaySettingsEx(string d, ref DM dm, IntPtr w, uint flags, IntPtr p);
}
'@
Add-Type $source
$current = -1
$updateRegistry = 1
$displayFrequency = 0x400000
function New-Mode { $mode = New-Object Rate+DM; $mode.Size = [Runtime.InteropServices.Marshal]::SizeOf($mode); $mode }
$now = New-Mode
[void][Rate]::EnumDisplaySettings([NullString]::Value, $current, [ref]$now)
$found = $false
for ($index = 0; -not $found; $index++) {
    $mode = New-Mode
    if (-not [Rate]::EnumDisplaySettings([NullString]::Value, $index, [ref]$mode)) { break }
    $found = $mode.W -eq $now.W -and $mode.H -eq $now.H -and $mode.Freq -eq $wanted
}
if (-not $found) {
    $said = "no mode at $wanted Hz, staying at $($now.Freq) Hz"
} elseif ($now.Freq -eq $wanted) {
    $said = "already at $wanted Hz"
} else {
    $now.Freq = $wanted
    $now.Fields = $displayFrequency
    $result = [Rate]::ChangeDisplaySettingsEx([NullString]::Value, [ref]$now, [IntPtr]::Zero, $updateRegistry, [IntPtr]::Zero)
    $after = New-Mode
    [void][Rate]::EnumDisplaySettings([NullString]::Value, $current, [ref]$after)
    $said = "result $result, now at $($after.Freq) Hz"
}
$said | Out-File C:\gpupv\cap\set-refresh.txt -Encoding ascii
