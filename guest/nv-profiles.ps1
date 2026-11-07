# Sent by the host through the agent once the desktop is up. Makes the folder NVIDIA's settings store
# lives in, with a copy of the driver's own profile database in it. The driver's installer would do
# this, and a guest never runs it. A program that reads its profile through NVAPI stops at start
# without the folder, as NVAPI can't open the store. The files stay once they exist, as NVAPI writes
# them.
$store = Join-Path $env:ProgramData 'NVIDIA Corporation\Drs'
$usersSid = 'S-1-5-32-545'
$names = 'nvdrsdb0.bin', 'nvdrsdb1.bin'
$driverFolders = Join-Path $env:windir 'System32\HostDriverStore\FileRepository\nv_dispi.inf_amd64_*'

$missing = $names | Where-Object { -not (Test-Path -LiteralPath (Join-Path $store $_)) }
if (-not $missing) { "kept $store"; return }
$database = Get-ChildItem -Path (Join-Path $driverFolders 'nvdrsdb.bin') -ea 0 |
    Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
if (-not $database) { "no nvdrsdb.bin under $driverFolders"; return }
try {
    New-Item -ItemType Directory -Path $store -Force -ErrorAction Stop | Out-Null
    foreach ($name in $missing) {
        Copy-Item -LiteralPath $database.FullName -Destination (Join-Path $store $name) -Force -ErrorAction Stop
    }
    $users = (New-Object System.Security.Principal.SecurityIdentifier $usersSid).Translate([System.Security.Principal.NTAccount]).Value
    $said = & icacls $store /grant "${users}:(OI)(CI)M" /T 2>&1
    if ($LASTEXITCODE -ne 0) { throw "icacls: $said" }
    "made $store from $($database.FullName)"
} catch {
    "FAILED $store`: $($_.Exception.Message)"
}
