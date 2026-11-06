# Signs a build on this machine and installs it. The certificate is made here, trusted only here, and its private key is deleted
# before the install, so nothing can be signed with it afterwards. BUILDING.md says what that means for the machine.
# Usage, as an administrator: sign-and-install.ps1 FOLDER, which holds vdd_settings.xml, and pkg with GuesthouseDisplay.dll and GuesthouseDisplay.inf.
param([Parameter(Mandatory)][string]$Folder)
$ErrorActionPreference = 'Continue'
$folder = $Folder
$package = "$folder\pkg"
$settingsFolder = 'C:\VirtualDisplayDriver'
$storeKey = 'HKLM:\SOFTWARE\Microsoft\SystemCertificates\TrustedPublisher\Certificates'
$hardwareId = 'Root\GuesthouseDisplay'
$subject = 'CN=Guesthouse display driver, this machine only'
$yearsValid = 10

"secure boot: $(Confirm-SecureBootUEFI)"
New-Item -ItemType Directory -Force $settingsFolder | Out-Null
Copy-Item "$folder\vdd_settings.xml" $settingsFolder -Force

# The catalog names every file in the folder, so it's made outside and moved in.
$catalog = "$folder\GuesthouseDisplay.cat"
Remove-Item "$package\GuesthouseDisplay.cat", $catalog -ErrorAction SilentlyContinue
New-FileCatalog -Path $package -CatalogFilePath $catalog -CatalogVersion 2.0 | Out-Null
Move-Item $catalog "$package\GuesthouseDisplay.cat" -Force

$certificate = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -CertStoreLocation Cert:\LocalMachine\My `
    -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddYears($yearsValid)
"certificate: $($certificate.Thumbprint)"
$public = New-Object Security.Cryptography.X509Certificates.X509Certificate2 (, $certificate.Export('Cert'))

if (-not (Test-Path $storeKey)) { New-Item -Path $storeKey -Force | Out-Null }
foreach ($name in 'Root', 'TrustedPublisher') {
    $store = New-Object Security.Cryptography.X509Certificates.X509Store $name, 'LocalMachine'
    $store.Open('ReadWrite')
    $store.Add($public)
    $store.Close()
}
"in Root: $(Test-Path "Cert:\LocalMachine\Root\$($certificate.Thumbprint)")"
"in TrustedPublisher: $(Test-Path "Cert:\LocalMachine\TrustedPublisher\$($certificate.Thumbprint)")"

$signed = Set-AuthenticodeSignature -FilePath "$package\GuesthouseDisplay.cat" -Certificate $certificate -HashAlgorithm SHA256
"signing: $($signed.Status) $($signed.StatusMessage)"
Remove-Item "Cert:\LocalMachine\My\$($certificate.Thumbprint)" -DeleteKey
"private key still there: $(Test-Path "Cert:\LocalMachine\My\$($certificate.Thumbprint)")"

$source = @'
using System; using System.Runtime.InteropServices;
public class Dev {
  [StructLayout(LayoutKind.Sequential)] public struct SP { public int cbSize; public Guid ClassGuid; public int DevInst; public IntPtr Reserved; }
  [DllImport("setupapi.dll", SetLastError=true)] public static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid g, IntPtr h);
  [DllImport("setupapi.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool SetupDiCreateDeviceInfoW(IntPtr s, string name, ref Guid g, string desc, IntPtr h, int flags, ref SP d);
  [DllImport("setupapi.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool SetupDiSetDeviceRegistryPropertyW(IntPtr s, ref SP d, int prop, byte[] b, int n);
  [DllImport("setupapi.dll", SetLastError=true)] public static extern bool SetupDiCallClassInstaller(int f, IntPtr s, ref SP d);
  [DllImport("newdev.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool UpdateDriverForPlugAndPlayDevicesW(IntPtr h, string id, string inf, int flags, out bool reboot);
  public static string Make(string id, string inf) {
    Guid g = new Guid("4d36e968-e325-11ce-bfc1-08002be10318");
    IntPtr s = SetupDiCreateDeviceInfoList(ref g, IntPtr.Zero);
    SP d = new SP(); d.cbSize = Marshal.SizeOf(d);
    if (!SetupDiCreateDeviceInfoW(s, "Display", ref g, null, IntPtr.Zero, 1, ref d)) return "create " + Marshal.GetLastWin32Error();
    byte[] b = System.Text.Encoding.Unicode.GetBytes(id + "\0\0");
    if (!SetupDiSetDeviceRegistryPropertyW(s, ref d, 1, b, b.Length)) return "hwid " + Marshal.GetLastWin32Error();
    if (!SetupDiCallClassInstaller(0x19, s, ref d)) return "register " + Marshal.GetLastWin32Error();
    bool r; bool ok = UpdateDriverForPlugAndPlayDevicesW(IntPtr.Zero, id, inf, 1, out r);
    return "update " + ok + " error " + Marshal.GetLastWin32Error() + " reboot " + r;
  }
}
'@
Add-Type $source
pnputil /add-driver "$package\GuesthouseDisplay.inf" 2>&1 | Select-Object -Last 4
[Dev]::Make($hardwareId, "$package\GuesthouseDisplay.inf")
Start-Sleep 6
Get-PnpDevice -Class Display | ForEach-Object { "$($_.Status) $($_.Problem) $($_.FriendlyName) $($_.InstanceId)" }
