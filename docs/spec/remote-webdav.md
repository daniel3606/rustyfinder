# Remote: WebDAV and WebDAV over HTTPS (P1)

The Connect panel is shared (remote-sftp.md §1).

## 1. Connection

- Protocols: **WebDAV** (http, default port 80) and **WebDAV HTTPS** (https, default port 443).
- Server field: a host, or a full URL. If a URL with a path is pasted (`https://dav.example.com/remote.php/dav/files/bob/`), the host goes in Server and the path goes in **Remote Path**, so the two are kept apart.
- Username is **optional**. Anonymous or token-in-URL servers connect without one.
- Authentication: HTTP Basic (only over HTTPS, or after a warning on plain HTTP *(RustyFinder choice)*) and Digest, chosen from the server's challenge. Credentials are stored in the keychain.
- TLS certificate problems are handled as in remote-ftp §2 (warn, cancel by default, allow trusting the certificate).
- Request timeout **60 s** for metadata requests. Transfers have no total timeout, but stall for at most 60 s *(RustyFinder choice)*.
- Redirects (301/302/307/308) are followed for GET and PROPFIND, staying on the same host.

## 2. Listing

- Uses `PROPFIND` with `Depth: 1`, asking for at least: resource type (collection or not), content length, last-modified, and creation date. `Depth: infinity` is never used.
- Hrefs in the response are percent-decoded to names. The entry for the folder itself is dropped. Names are compared after normalizing trailing slashes.
- Dates are parsed in RFC 1123 (`getlastmodified`) and ISO 8601 (`creationdate`) formats. A missing date means "no date" (edge-cases §6).

## 3. Operations

| Operation | Method |
|---|---|
| Download | `GET`. **No resume** (Resume is never offered) *(RustyFinder choice; Range resume MAY be added if the server advertises Accept-Ranges)* |
| Upload | `PUT` (streamed body) |
| Make folder | `MKCOL`. Creating nested paths creates the missing parents in order |
| Delete | `DELETE` (recursive on collections) |
| Rename / move on the same server | `MOVE` with a `Destination` header (absolute URL, percent-encoded) and `Overwrite: F`. Replacing after a conflict choice uses `Overwrite: T` |
| Copy on the same server | `COPY` with `Destination` *(RustyFinder choice; otherwise stream through RustyFinder)* |
| Set modification time | `PROPPATCH` of last-modified, best effort. Many servers ignore it, so sync falls back to size plus a server-reported time |

- Percent-encoding of paths covers spaces, `#`, `%`, `?` and non-ASCII characters (UTF-8 encoded).
- `423 Locked` errors are reported as "locked by another user".
- A `507 Insufficient Storage` reply is reported as a quota error.

## 4. Acceptance tests

Fixture: a local WebDAV server (for example a scripted server) with Basic auth and an anonymous share.

- **WD-01** Given the URL `https://localhost:8443/dav/files/bob/` pasted into Server, then Server = localhost, Port = 8443, and Remote Path = /dav/files/bob/.
- **WD-02** Given an anonymous WebDAV share, when the user connects with an empty Username, then the listing appears.
- **WD-03** Given a folder `My Docs #1`, when it is listed and opened, then names display correctly and navigating into it works.
- **WD-04** Given a rename of `a.txt` to `b.txt` where `b.txt` exists, then a conflict prompt is shown (the server's 412 means the name exists). After Replace, `b.txt` has a.txt's content.
- **WD-05** Given an upload of a 1 GB file, then memory use stays bounded (streamed) and the remote size equals 1 GB.
- **WD-06** Given a PROPFIND reply that omits `creationdate`, then the listing still succeeds.
