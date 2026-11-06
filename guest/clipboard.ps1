# Runs in the user's session, which the agent cannot reach as SYSTEM, while a viewer that shares the
# clipboard is open. It only answers: on loopback, and only requests that start with the agent's token.
Add-Type -AssemblyName System.Windows.Forms
$token = (Get-Content C:\gpupv\token.txt -Raw).Trim()
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 8766)
$listener.Start()
while ($true) {
    $client = $listener.AcceptTcpClient()
    try {
        $stream = $client.GetStream()
        $parts = [System.IO.StreamReader]::new($stream).ReadLine().Split(' ')
        if ($parts[0] -eq $token) {
            $writer = [System.IO.StreamWriter]::new($stream)
            if ($parts[1] -eq 'get') {
                $writer.Write([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([Windows.Forms.Clipboard]::GetText())))
            } else {
                [Windows.Forms.Clipboard]::SetText([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($parts[1])))
                $writer.Write('set')
            }
            $writer.Flush()
        }
    } catch {
        # Another program has the clipboard open. The next request tries again.
    } finally {
        $client.Close()
    }
}
