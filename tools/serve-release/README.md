# serve-release

A minimal static file server over a raw TCP socket, used to exercise `o824 update --fetch`
against a locally served release. It is a test helper, not part of the product.

```
powershell -File tools/serve-release/serve.ps1 -Directory <release-dir> -Port 8765
```

`HttpListener` would be shorter, but it goes through HTTP.sys and wants a URL reservation or
administrator rights on this machine. A socket does not.

Paths with spaces matter: start it with `-File "<path>"` quoted, or `Start-Process` will split
the argument at the first space.
