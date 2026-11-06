# Takes the display driver back out, when it failed to come up. Runs as SYSTEM, sent by the host. With
# the device gone Windows goes back to the basic display by itself. The certificate goes too, as a root
# nothing needs shouldn't stay.
$hardwareId = 'Root\GuesthouseDisplay'
# Windows puts the name in quotes, as it has a comma in it.
$subject = '*Guesthouse display driver, this machine only*'
Unregister-ScheduledTask guesthouse-display -Confirm:$false -ea 0
Get-PnpDevice -Class Display -ea 0 | Where-Object {
    (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName DEVPKEY_Device_HardwareIds -ea 0).Data -contains $hardwareId
} | ForEach-Object { pnputil /remove-device $_.InstanceId }
Get-WindowsDriver -Online -ea 0 | Where-Object OriginalFileName -like '*guesthousedisplay.inf' |
    ForEach-Object { pnputil /delete-driver $_.Driver /uninstall /force }
foreach ($store in 'Root', 'TrustedPublisher') {
    Get-ChildItem "Cert:\LocalMachine\$store" | Where-Object Subject -like $subject | Remove-Item
}
# Only a run that died part way leaves one here, and that one still has its key.
Get-ChildItem Cert:\LocalMachine\My | Where-Object Subject -like $subject |
    ForEach-Object { Remove-Item "Cert:\LocalMachine\My\$($_.Thumbprint)" -DeleteKey }
'removed'
