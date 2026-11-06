# Sets the desktop to the largest mode the screen offers.
# Windows keeps the desktop size it last used for a monitor and scales it up when the screen is
# bigger, and a monitor with no EDID always counts as the same monitor. Runs in the user's session.
$src = @'
using System; using System.Runtime.InteropServices;
public class Mode {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DM {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Name; public short a, b, Size, Extra;
    public int Fields, x, y, Orient, Fixed; public short c1, c2, c3, c4, c5;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Form; public short LogPix, Bits;
    public int W, H, DFlags, Freq, i1, i2, i3, i4, i5, i6, i7, i8; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplaySettings(string d, int mode, ref DM dm);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int ChangeDisplaySettingsEx(string d, ref DM dm, IntPtr w, uint flags, IntPtr p);
}
'@
Add-Type $src
$modes = @()
for ($i = 0; ; $i++) {
  $m = New-Object Mode+DM; $m.Size = [Runtime.InteropServices.Marshal]::SizeOf($m)
  if (-not [Mode]::EnumDisplaySettings('\\.\DISPLAY1', $i, [ref]$m)) { break }
  $modes += [pscustomobject]@{ W = $m.W; H = $m.H; Bits = $m.Bits; Hz = $m.Freq }
}
$current = New-Object Mode+DM; $current.Size = [Runtime.InteropServices.Marshal]::SizeOf($current)
[void][Mode]::EnumDisplaySettings('\\.\DISPLAY1', -1, [ref]$current)
$out = @("was $($current.W)x$($current.H), $($modes.Count) modes: " + (($modes | % { "$($_.W)x$($_.H)" } | Select -Unique) -join ' '))
$best = $modes | Sort-Object { $_.W * $_.H }, Bits | Select -Last 1
if ($best.W -ne $current.W -or $best.H -ne $current.H) {
  $current.W = $best.W; $current.H = $best.H
  $current.Fields = 0x180000  # DM_PELSWIDTH, DM_PELSHEIGHT
  $result = [Mode]::ChangeDisplaySettingsEx('\\.\DISPLAY1', [ref]$current, [IntPtr]::Zero, 1, [IntPtr]::Zero)
  $out += "set $($best.W)x$($best.H): result $result"
}
$now = New-Object Mode+DM; $now.Size = [Runtime.InteropServices.Marshal]::SizeOf($now)
[void][Mode]::EnumDisplaySettings('\\.\DISPLAY1', -1, [ref]$now)
$out += "now $($now.W)x$($now.H) at $($now.Freq) Hz"
$out | Out-File C:\gpupv\cap\setmode.txt -Encoding ascii
