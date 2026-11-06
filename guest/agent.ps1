# Runs PowerShell sent by the host. Only QEMU's port forward on the host's loopback reaches it, and
# each request has to start with this machine's token.

# Vulkan won't use a display unless it looks like a remote session's, and it goes by the name. The
# key can show up after setup or on a later boot, so this looks every time. Windows reads it at boot.
function Name-Display {
    Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Video' -ea 0 | ForEach-Object { Join-Path $_.PSPath '0000' } |
        Where-Object { (Get-ItemProperty $_ -ea 0).DriverDesc -eq 'Microsoft Basic Display Adapter' } |
        ForEach-Object { Set-ItemProperty $_ DriverDesc 'Microsoft Basic Display Adapter (RDPUDD Chained DD)' }
}
Name-Display
$token = (Get-Content C:\gpupv\token.txt -Raw).Trim()
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Any, 8765)
$listener.Start()
while ($true) {
    $client = $listener.AcceptTcpClient()
    try {
        $stream = $client.GetStream()
        $reader = [System.IO.StreamReader]::new($stream)
        $writer = [System.IO.StreamWriter]::new($stream)
        $parts = $reader.ReadLine().Split(' ', 2)
        if ($parts[0] -eq $token) {
            Name-Display
            $script = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($parts[1]))
            try { $out = Invoke-Expression $script 2>&1 | Out-String -Width 220 } catch { $out = "ERROR: $_" }
            $writer.Write($out)
            $writer.Flush()
        }
    } catch {
    } finally {
        $client.Close()
    }
}
