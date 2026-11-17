# Test TLS fixture

A self-signed certificate for `localhost` / `127.0.0.1`, used only by the
FTP-with-TLS and WebDAV-over-HTTPS tests against local scripted servers.
`localhost.p12` holds its key under the password `fixture`. It protects
nothing and is published on purpose; never trust it outside these tests.
