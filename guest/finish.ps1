# Sent by the host through the agent until it says finished. Setup logs in as defaultuser0 first and
# can restart, so the account only counts once it has a desktop of its own.
if (-not (Get-Process explorer -IncludeUserName -ea 0 | ? UserName -match '\\gpupv$')) { 'not logged in yet'; return }
$action = New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File C:\gpupv\setmode.ps1'
$trigger = New-ScheduledTaskTrigger -AtLogOn -User gpupv
$principal = New-ScheduledTaskPrincipal -UserId gpupv -LogonType Interactive
Register-ScheduledTask -TaskName gpupv-setmode -Action $action -Trigger $trigger -Principal $principal -Force | Out-Null
'finished'
