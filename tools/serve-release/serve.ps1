# A minimal static file server over a raw TCP socket, used to exercise `o824 update --fetch`
# against a locally served release.
#
# `HttpListener` would be shorter, but it goes through HTTP.sys and wants a URL reservation or
# administrator rights. A socket does not, and this script is not part of the product.
param(
    [Parameter(Mandatory = $true)][string]$Directory,
    [int]$Port = 8765
)

$ErrorActionPreference = "Stop"
$server = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
$server.Start()
Write-Output "serving $Directory on http://127.0.0.1:$Port/"

while ($true) {
    $client = $server.AcceptTcpClient()
    try {
        $stream = $client.GetStream()
        $reader = New-Object System.IO.StreamReader($stream)
        $request = $reader.ReadLine()
        # Drain the headers; the request line is all this server needs.
        while ($reader.ReadLine() -ne "") { }
        $name = ""
        if ($request -match "^GET\s+/([^\s]*)") { $name = $matches[1] }
        $name = [System.Uri]::UnescapeDataString($name)
        $path = if ($name) { Join-Path $Directory $name } else { Join-Path $Directory "index.html" }

        if (Test-Path -LiteralPath $path -PathType Leaf) {
            $bytes = [System.IO.File]::ReadAllBytes($path)
            $header = "HTTP/1.1 200 OK`r`nContent-Type: application/octet-stream`r`nContent-Length: $($bytes.Length)`r`nConnection: close`r`n`r`n"
        } else {
            $bytes = [System.Text.Encoding]::ASCII.GetBytes("not found")
            $header = "HTTP/1.1 404 Not Found`r`nContent-Length: $($bytes.Length)`r`nConnection: close`r`n`r`n"
        }
        $headerBytes = [System.Text.Encoding]::ASCII.GetBytes($header)
        $stream.Write($headerBytes, 0, $headerBytes.Length)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()
    } finally {
        $client.Close()
    }
}
