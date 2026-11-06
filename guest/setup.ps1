# Runs as SYSTEM at the end of Windows setup. It installs the agent, stops the machine going to sleep,
# and puts the driver where the guest looks for it. finish.ps1 does the part that needs the account.
$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force C:\gpupv, C:\gpupv\cap | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'agent.ps1') C:\gpupv\agent.ps1 -Force
Copy-Item (Join-Path $PSScriptRoot 'token.txt') C:\gpupv\token.txt -Force
Copy-Item (Join-Path $PSScriptRoot 'setmode.ps1') C:\gpupv\setmode.ps1 -Force
New-NetFirewallRule -DisplayName 'gpupv-agent' -Direction Inbound -Protocol TCP -LocalPort 8765 -Action Allow | Out-Null
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File C:\gpupv\agent.ps1'
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName 'gpupv-agent' -Action $action -Trigger $trigger -User 'SYSTEM' -RunLevel Highest -Settings $settings -Force | Out-Null
powercfg /change standby-timeout-ac 0
powercfg /change monitor-timeout-ac 0
powercfg /hibernate off
# The largest firmware mode wins with this, and the app picks that with the VGA memory size.
bcdedit /set '{current}' highestmode on
# Windows looks here for the driver's user-mode half, under the name the daemon gives it.
$store = 'C:\Windows\System32\HostDriverStore\FileRepository\nv_dispi.inf_amd64_gpupv'
New-Item -ItemType Directory -Force $store | Out-Null
robocopy (Join-Path $PSScriptRoot 'nvidia') $store /E /NP /NFL /NDL /NJH /NJS
Start-ScheduledTask -TaskName 'gpupv-agent'
'setup done' | Out-File C:\gpupv\setup.log
