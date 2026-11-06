# Puts the screen in the mode the app wants, and says what it really is. The host puts $width, $height,
# $hz, $hdr and $expected first, the last saying that the display driver should be there. Runs in the user's session, as only that can change a display. On the basic display
# it changes nothing. The last line is what the app reads.
$source = @'
using System; using System.Runtime.InteropServices; using System.Text;
public class Screen {
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
// PowerShell has no copy of wingdi.h, so these are its structures as offsets.
public static class Colour {
  const uint OnlyActivePaths = 2;
  const int PathSize = 72, ModeSize = 64, TargetAdapter = 20, TargetId = 28;
  const int GetAdvancedColorInfo = 9, SetAdvancedColorState = 10, SetHdrState = 16;
  const int HeaderSize = 20, InfoSize = 32, SetSize = 24;
  const uint EnabledBit = 2;
  [DllImport("user32.dll")] static extern int GetDisplayConfigBufferSizes(uint flags, out uint paths, out uint modes);
  [DllImport("user32.dll")] static extern int QueryDisplayConfig(uint flags, ref uint paths, byte[] pathData, ref uint modes, byte[] modeData, IntPtr topology);
  [DllImport("user32.dll")] static extern int DisplayConfigGetDeviceInfo(byte[] packet);
  [DllImport("user32.dll")] static extern int DisplayConfigSetDeviceInfo(byte[] packet);
  static byte[] Packet(int type, int size, byte[] paths) {
    byte[] packet = new byte[size];
    BitConverter.GetBytes(type).CopyTo(packet, 0);
    BitConverter.GetBytes(size).CopyTo(packet, 4);
    Array.Copy(paths, TargetAdapter, packet, 8, 8);
    Array.Copy(paths, TargetId, packet, 16, 4);
    return packet;
  }
  // Our display is the only one by now, so it's the first path.
  static byte[] FirstPath() {
    uint pathCount, modeCount;
    if (GetDisplayConfigBufferSizes(OnlyActivePaths, out pathCount, out modeCount) != 0) return null;
    byte[] paths = new byte[pathCount * PathSize], modes = new byte[modeCount * ModeSize];
    if (QueryDisplayConfig(OnlyActivePaths, ref pathCount, paths, ref modeCount, modes, IntPtr.Zero) != 0) return null;
    return pathCount > 0 ? paths : null;
  }
  public static bool IsOn() {
    byte[] paths = FirstPath();
    if (paths == null) return false;
    byte[] info = Packet(GetAdvancedColorInfo, InfoSize, paths);
    if (DisplayConfigGetDeviceInfo(info) != 0) return false;
    return (BitConverter.ToUInt32(info, HeaderSize) & EnabledBit) != 0;
  }
  public static void Set(bool on) {
    byte[] paths = FirstPath();
    if (paths == null) return;
    // 24H2 added its own request. The older one still answers there, but may not change anything.
    foreach (int request in new[] { SetAdvancedColorState, SetHdrState }) {
      byte[] packet = Packet(request, SetSize, paths);
      packet[HeaderSize] = (byte)(on ? 1 : 0);
      DisplayConfigSetDeviceInfo(packet);
    }
  }
}
'@
Add-Type $source
$ourName = 'RDPUDD Chained DD'
$takeOver = 'C:\gpupv\idd\take-over-display.ps1'
$current = -1
$attachedToDesktop = 1
$primaryDevice = 4
$sizeRateAndPosition = 0x580020
$updateRegistryPrimary = 0x11
$hdrSettleSeconds = 2
$monitorTries = 25
$safeHz = 60
$said = @()

function All-Displays {
    for ($i = 0; $i -lt 16; $i++) {
        $device = New-Object Screen+DD
        $device.cb = [Runtime.InteropServices.Marshal]::SizeOf($device)
        if (-not [Screen]::EnumDisplayDevices([NullString]::Value, $i, [ref]$device, 0)) { break }
        $device
    }
}
function New-Mode { $mode = New-Object Screen+DM; $mode.Size = [Runtime.InteropServices.Marshal]::SizeOf($mode); $mode }
function Current-Mode($name) { $mode = New-Mode; [void][Screen]::EnumDisplaySettings($name, $current, [ref]$mode); $mode }
function Has-Mode($name) {
    for ($index = 0; ; $index++) {
        $mode = New-Mode
        if (-not [Screen]::EnumDisplaySettings($name, $index, [ref]$mode)) { return $false }
        if ($mode.W -eq $width -and $mode.H -eq $height -and $mode.Freq -eq $hz) { return $true }
    }
}
function Our-Display { All-Displays | Where-Object { $_.Str -eq $ourName } | Select-Object -First 1 }

$ours = Our-Display
for ($try = 0; $expected -eq 1 -and -not $ours -and $try -lt $monitorTries; $try++) {
    Start-Sleep 1
    $ours = Our-Display
}
if ($ours) {
    $others = @(All-Displays | Where-Object { $_.Str -ne $ourName -and ($_.Flags -band $attachedToDesktop) })
    if ($others.Count -gt 0 -or -not ($ours.Flags -band $primaryDevice)) {
        # Told a mode it hasn't got, Windows leaves ours at 800x600, where a later change fails.
        $firstHz = if (Has-Mode $ours.Name) { $hz } else { $safeHz }
        & $takeOver $width $height $firstHz
        $ours = Our-Display
    }
    $now = Current-Mode $ours.Name
    if ($now.W -ne $width -or $now.H -ne $height -or $now.Freq -ne $hz) {
        if (Has-Mode $ours.Name) {
            $now.W = $width; $now.H = $height; $now.Freq = $hz; $now.x = 0; $now.y = 0
            $now.Fields = $sizeRateAndPosition
            $said += "mode: $([Screen]::ChangeDisplaySettingsEx($ours.Name, [ref]$now, [IntPtr]::Zero, $updateRegistryPrimary, [IntPtr]::Zero))"
            $said += "apply: $([Screen]::ChangeDisplaySettingsEx([NullString]::Value, [IntPtr]::Zero, [IntPtr]::Zero, 0, [IntPtr]::Zero))"
        } else {
            $said += "no mode at ${width}x${height} and $hz Hz"
        }
    }
    if ([Colour]::IsOn() -ne ($hdr -eq 1)) {
        [Colour]::Set($hdr -eq 1)
        Start-Sleep $hdrSettleSeconds
    }
    $now = Current-Mode $ours.Name
    $on = if ([Colour]::IsOn()) { 1 } else { 0 }
    $said += "screen ours $($now.W)x$($now.H) $($now.Freq) hdr $on"
} else {
    $now = Current-Mode ([NullString]::Value)
    $said += "screen basic $($now.W)x$($now.H) $($now.Freq) hdr 0"
}
$said | Out-File C:\gpupv\cap\apply-display.txt -Encoding ascii
