# Gives our display driver modes it doesn't ship with: in its settings, which it reads as it starts, and
# now, by its control device. The host puts $settings, the whole settings file in base64, and $command,
# the driver's SETMODES with the same modes, first. Runs as an administrator, as both are theirs.
$source = @'
using System; using System.Runtime.InteropServices; using System.Text;
using Microsoft.Win32.SafeHandles;
public static class DriverControl {
  // CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_WRITE_DATA), IOCTL_VDD_COMMAND in the
  // driver's vdd_control_ioctl.h, which also has the interface's GUID.
  const uint Command = 0x22A000;
  const uint ListPresent = 0, GenericReadWrite = 0xC0000000, ShareReadWrite = 3, OpenExisting = 3;
  static Guid Interface = new Guid("DA9F8C2B-7E4F-49A1-9D4E-6F2B0E1A0C4D");
  [DllImport("cfgmgr32.dll", CharSet=CharSet.Unicode)] static extern int CM_Get_Device_Interface_List_SizeW(out uint size, ref Guid g, string id, uint flags);
  [DllImport("cfgmgr32.dll", CharSet=CharSet.Unicode)] static extern int CM_Get_Device_Interface_ListW(ref Guid g, string id, [Out] char[] list, uint size, uint flags);
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern SafeFileHandle CreateFileW(string path, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool DeviceIoControl(SafeFileHandle device, uint code, byte[] input, int inputSize, IntPtr output, int outputSize, out int returned, IntPtr overlapped);
  public static string Send(string command) {
    uint size;
    if (CM_Get_Device_Interface_List_SizeW(out size, ref Interface, null, ListPresent) != 0 || size <= 1) return "no control device";
    char[] list = new char[size];
    if (CM_Get_Device_Interface_ListW(ref Interface, null, list, size, ListPresent) != 0) return "no control device";
    string path = new string(list).Split('\0')[0];
    using (SafeFileHandle device = CreateFileW(path, GenericReadWrite, ShareReadWrite, IntPtr.Zero, OpenExisting, 0, IntPtr.Zero)) {
      if (device.IsInvalid) return "not opened, error " + Marshal.GetLastWin32Error();
      byte[] words = Encoding.Unicode.GetBytes(command + "\0");
      int returned;
      if (!DeviceIoControl(device, Command, words, words.Length, IntPtr.Zero, 0, out returned, IntPtr.Zero)) {
        return "refused, error " + Marshal.GetLastWin32Error();
      }
      return "sent";
    }
  }
}
'@
Add-Type $source
# Where sign-and-install.ps1 puts the settings, and the driver looks for them.
$settingsFile = 'C:\VirtualDisplayDriver\vdd_settings.xml'
try {
    [IO.File]::WriteAllBytes($settingsFile, [Convert]::FromBase64String($settings))
    "settings saved"
} catch {
    "settings not saved: $($_.Exception.Message)"
}
"modes $([DriverControl]::Send($command))"
