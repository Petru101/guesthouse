# Sent by the host through the agent. The host puts $helper, the helper in Base64, first. Sending it
# each time means any machine with the agent gets it, however old, and always the current version.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
[IO.File]::WriteAllBytes('C:\gpupv\clipboard.ps1', [Convert]::FromBase64String($helper))
$action = New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -STA -WindowStyle Hidden -ExecutionPolicy Bypass -File C:\gpupv\clipboard.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName gpupv-clipboard -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-clipboard
'started'
