# Registers take-over-display.ps1 to run at each logon, for the installer to call once. Runs as an administrator.
# Usage: install-take-over.ps1 FOLDER WIDTH HEIGHT HZ USER
param([string]$Folder, [int]$Width, [int]$Height, [int]$Hz, [string]$User)
$taskName = 'guesthouse-display'
$script = Join-Path $Folder 'take-over-display.ps1'
$arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$script`" $Width $Height $Hz"
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arguments
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $User
$principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Force | Out-Null
"registered $taskName for $User"
