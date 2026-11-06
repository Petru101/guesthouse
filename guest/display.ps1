# Lists what the logged-in session sees. Runs as the user, because session 0 has no display.
$src = @'
using System; using System.Runtime.InteropServices;
public class Disp {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DD {
    public int cb; [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Name;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Str; public int Flags;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Id;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Key; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DM {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Name; public short a, b, Size, Extra;
    public int Fields, x, y, Orient, Fixed; public short c1, c2, c3, c4, c5;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Form; public short LogPix, Bits;
    public int W, H, DFlags, Freq, i1, i2, i3, i4, i5, i6, i7, i8; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplayDevices(string d, uint i, ref DD dd, uint f);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplaySettings(string d, int mode, ref DM dm);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
}
'@
Add-Type $src
$out = @()
$out += "session $((Get-Process -Id $PID).SessionId) user $env:USERNAME screen $([Disp]::GetSystemMetrics(0))x$([Disp]::GetSystemMetrics(1)) monitors $([Disp]::GetSystemMetrics(80))"
for ($i = 0; $i -lt 12; $i++) {
  $d = New-Object Disp+DD; $d.cb = [Runtime.InteropServices.Marshal]::SizeOf($d)
  if (-not [Disp]::EnumDisplayDevices([NullString]::Value, $i, [ref]$d, 0)) { break }
  $m = New-Object Disp+DM; $m.Size = [Runtime.InteropServices.Marshal]::SizeOf($m)
  $cur = [Disp]::EnumDisplaySettings($d.Name, -1, [ref]$m)
  $out += "$($d.Name) '$($d.Str)' flags 0x$('{0:x}' -f $d.Flags) current=$cur $($m.W)x$($m.H) $($m.Freq) Hz id $($d.Id)"
  for ($j = 0; $j -lt 4; $j++) {
    $c = New-Object Disp+DD; $c.cb = [Runtime.InteropServices.Marshal]::SizeOf($c)
    if (-not [Disp]::EnumDisplayDevices($d.Name, $j, [ref]$c, 0)) { break }
    $out += "   monitor '$($c.Str)' flags 0x$('{0:x}' -f $c.Flags) $($c.Id)"
  }
}
$out | Out-File C:\gpupv\cap\display.txt -Encoding ascii
