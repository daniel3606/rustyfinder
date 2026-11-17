# Remote: FTP and FTP over TLS (P1)

The Connect panel is shared (remote-sftp.md §1).

## 1. Protocol variants

| Variant | Default port | Notes |
|---|---|---|
| FTP | 21 | plain |
| FTP TLS (explicit, "FTPES") | 21 | `AUTH TLS` after connect. Data channel protected (`PBSZ 0`, `PROT P`). |
| FTPS implicit *(RustyFinder choice; optional)* | 990 | TLS from the first byte |

Advanced options: **Passive Mode** (default **on**), Port, Remote Path, **Text Encoding** (Auto / UTF-8 / a legacy encoding list), Color.

## 2. Connection behavior

- After login, the client sends `FEAT`. If UTF8 is advertised it sends `OPTS UTF8 ON` and uses UTF-8 names. Otherwise, in **Auto** encoding, it decodes names as UTF-8 when the bytes are valid UTF-8, and otherwise with a statistical detector (falling back to Latin-1). Undecodable bytes show as `�`, and operations reuse the original bytes (edge-cases §1).
- **Passive mode:** uses `EPSV`, falling back to `PASV`. If the server's PASV reply has a private or unroutable IP different from the control connection's peer, RustyFinder uses the control connection's address instead. **Active mode** (`EPRT`/`PORT`) is used only when Passive is off. Editing an existing connection must never switch passive mode silently, whether the protocol changes or not.
- **TLS:**
  - **TLS session reuse** on the data channel: the data connection resumes the control connection's TLS session (many servers require this). This must not crash with servers that require it.
  - Certificate validation uses the system trust store. If the certificate is invalid (self-signed, wrong host, expired), a dialog warns that the server may be impersonating the host and shows the certificate. The user can Cancel (default), Continue once, or **always trust** that certificate for this host (stored in the keychain).
- **Listing:** uses `MLSD` when it is advertised, which gives exact UTC modification times with seconds, types and sizes. Otherwise `LIST -a` (falling back to `LIST`), parsed for **Unix** `ls -l` formats (including year-less recent dates and `total` lines), **Windows/IIS** (`MM-DD-YY hh:mmAM <DIR>`), and EPLF *(RustyFinder choice)*. Unknown lines are skipped, not fatal. Permissions from the Unix format are shown correctly.
- With `LIST` only, times have minute precision and are interpreted in the server's time zone (assume UTC unless the user sets a time offset). Sync uses a 60 s tolerance (sync-compare §3).
- **Timeouts:** control connect **60 s**. A data connection that doesn't open within 30 s fails the transfer *(RustyFinder choice)*. Idle control connections get a `NOOP` keep-alive every 60 s *(RustyFinder choice)*, and a timed-out control connection reconnects silently on next use.
- Several simultaneous transfers use several control connections, up to "Max. simultaneous transfers".

## 3. Operations

- Upload (`STOR`) and download (`RETR`) in binary mode (`TYPE I`).
- **Resume** with `REST <offset>` for both downloads and uploads (`REST`+`STOR`, or `APPE` as a fallback) when the server supports REST STREAM.
- Rename (`RNFR`/`RNTO`), delete (`DELE`, `RMD` recursively), make folder (`MKD`), new empty file (zero-byte `STOR`).
- After upload, the modification time is set with `MFMT` when it is advertised (or `SITE UTIME` as a fallback, ignored if unsupported).
- Permissions are changed with `SITE CHMOD` when the server supports it. Otherwise the Permissions section is read-only.
- Moving within one server uses rename.

## 4. Acceptance tests

Fixture: a local FTP server (for example a scripted test server) with UTF-8 on and off, TLS with a self-signed certificate, and a LIST-only mode.

- **FT-01** Given an FTP server behind NAT that returns a private IP in PASV, then listing and transfers still succeed using the control connection's IP.
- **FT-02** Given an FTPS-explicit server requiring TLS session reuse, then a directory listing succeeds and the app does not crash.
- **FT-03** Given a self-signed certificate, when connecting, then a trust dialog appears. Choosing always-trust means it doesn't appear on the next connect.
- **FT-04** Given a server without UTF8 that lists the CP1251 name `Привет.txt`, with encoding Auto, then the name is shown readably, or at worst with replacement characters, and can be downloaded.
- **FT-05** Given a LIST-only server returning IIS-style lines, then names, sizes, dates and folders are parsed correctly.
- **FT-06** Given a download interrupted at 50 % and resumed, then only the remaining bytes are transferred (REST sent) and the hashes match.
- **FT-07** Given Passive Mode is on, then no `PORT`/`EPRT` command is ever sent.
- **FT-08** Given an existing FTP TLS favorite with passive mode is edited (only its name changed) and saved, then it still connects in passive mode.
- **FT-09** Given a server with MLSD and MFMT, when a file with mtime 2020-01-01T00:00:00Z is uploaded, then the server's listing shows that mtime.
