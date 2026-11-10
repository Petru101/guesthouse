# Says what a finished install is missing. The host puts first $expected, how many driver files it
# sent, and $store_folder, the host driver store folder they went to, and the account's name in.
$user = '@USER@'
$missing = @()
$store = Join-Path 'C:\Windows\System32\HostDriverStore\FileRepository' $store_folder
$count = @(Get-ChildItem $store -Recurse -File -ea 0).Count
if ($count -ne $expected) { $missing += "driver files: $count of $expected" }
if (-not (bcdedit /enum '{current}' | Select-String 'highestmode\s+Yes')) { $missing += 'highestmode' }
$names = Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Video' -ea 0 |
    ForEach-Object { (Get-ItemProperty (Join-Path $_.PSPath '0000') -ea 0).DriverDesc }
if (-not ($names -match 'RDPUDD Chained DD')) { $missing += 'the display name Vulkan needs' }
foreach ($task in 'gpupv-agent', 'gpupv-setmode') {
    if (-not (Get-ScheduledTask $task -ea 0)) { $missing += "the task $task" }
}
# The group is found by its number, as its name is in Windows' own language: Administratoren,
# Administratorzy. A failed lookup says why, so a report has the cause in it.
$administrators = 'S-1-5-32-544'
try {
    $account = Get-LocalUser -Name $user -ErrorAction Stop
    $members = @(Get-LocalGroupMember -SID $administrators -ErrorAction Stop)
    if (-not ($members | Where-Object { $_.SID -eq $account.SID })) { $missing += "$user as an administrator" }
} catch {
    $missing += "$user as an administrator ($($_.Exception.Message))"
}
foreach ($path in 'C:\gpupv\cap', 'C:\gpupv\setmode.ps1') {
    if (-not (Test-Path $path)) { $missing += $path }
}
if ($missing) { $missing -join ', ' } else { 'all there' }
