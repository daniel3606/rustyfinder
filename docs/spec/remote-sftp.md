# Remote: SFTP (P1), plus the shared Connect panel

## 1. Connect panel (shared by all remote protocols)

- Opened with ⌘K (Go → Connect) or the toolbar Connect button. It appears **as a tab-like panel inside the pane** where the connection will open (the active pane, or the side set in "Open remote connections in"). It is never restored as a tab on relaunch.
- Fields depend on the protocol. Common fields: **Protocol** (popup), **Name** (optional, used for favorites), **Server**, **Username**, **Password**. **Show Advanced** reveals: **Port**, **Remote Path** (initial folder), **Local Path** (paired local folder for sync browsing *(RustyFinder choice)*), **Color** (a tint for the tab and favorite), and protocol-specific options.
- **Pasting a URL into Server** parses it: `sftp://user:pass@host:2222/path` fills in protocol, username, password, port and remote path. The protocol switches when the scheme names a supported one (sftp, ftp, ftps, http(s)/dav(s), s3, smb, afp, nfs, vnc). The Server field keeps only the host.
- The **Connect** button is enabled only when all required fields for the chosen protocol are filled.
- The Password field is single-line. Pasted line breaks are removed.
- **Add to Favorites** saves the connection (password in the keychain) to a favorite group, then connects.
- Passwords and tokens are always stored in the macOS keychain, never in RustyFinder's own files.
- Connection errors are shown inline in the panel ("Authorization failed", "Can't resolve host", timeout), and the fields stay editable.

## 2. SFTP connection behavior

- RustyFinder uses the **system OpenSSH client** (`ssh` with the sftp subsystem). So everything in the user's `~/.ssh/config` applies: Host aliases, User, Port, IdentityFile, ProxyJump/ProxyCommand, Include, Ciphers and so on. A username set only in the config file is enough, and the Username field may be empty.
- Fixed client options RustyFinder passes (behavioral contract):
  - connect timeout **60 s**
  - keep-alive probe every **30 s**. The session is declared dead after **2** unanswered probes (about 60–90 s).
  - at most **1** password prompt per attempt (a wrong password fails fast and RustyFinder re-asks the user, instead of ssh re-asking itself)
  - no X11 forwarding and no port forwarding
  - no fallback to rsh
  - when the user supplied a **password** and no key, public-key authentication is not attempted (avoids "too many authentication failures" from agent keys); when a key is supplied, it is used
- **Authentication:** password; private key file (OpenSSH, PEM, PKCS#8; ed25519, ecdsa, rsa), optionally passphrase-protected (prompt "Enter passphrase for key …", with an option to store it in the keychain); ssh-agent / config-defined keys; **keyboard-interactive** including **multi-factor** codes. Each server prompt (for example "Verification code") is shown in a RustyFinder dialog. No configuration is needed.
- **Host keys:** checked against `~/.ssh/known_hosts`.
  - Unknown host: a dialog shows the key type and the **SHA256 fingerprint** and asks to accept. Accepting adds it to known_hosts.
  - **Changed** key: a strong warning that names the host and the new fingerprint. Connecting needs an explicit override, and the default button is Cancel.
- **Initial folder:** the Remote Path if set, otherwise the login (home) directory, **not** `/`. ⌘↑ can still go above home if the server allows it.
- **Encoding:** names are treated as UTF-8. Bytes that aren't valid UTF-8 are shown with `�`, and operations still address the original bytes (edge-cases §1). A per-connection encoding override is available under Advanced *(RustyFinder choice)*.
- Disconnecting (sidebar context menu, closing the last tab using the session, or Disconnect) **closes the SSH session** (no leftover ssh processes).
- After sleep/wake or a network change, a dead session is detected (keep-alive or first failed request) and reconnected once automatically.

## 3. Operations over SFTP

- Listing shows: name, size, modification time (seconds), permissions, **owner and group names** (shown in Info), symlink targets.
- Rename (case-only renames included), delete (folders recursively), make folder, new empty file, **create and edit symlinks**, change permissions (setstat), set the modification time after upload.
- Uploads and downloads stream. **Resume** writes or reads at an offset (conflicts.md §6). It works even with older OpenSSH servers.
- Transfers run in parallel up to "Max. simultaneous transfers". Each concurrent transfer may use its own SFTP channel or session *(RustyFinder choice)*.
- A replace upload writes to a temporary name in the target folder and then renames it over the target (conflicts.md §4). If the server refuses rename-over-existing, RustyFinder deletes the target and then renames.
- Zero-byte downloads must give zero-byte files. Non-empty downloads must never give empty files.
- Drag-and-drop between a remote pane and anything else defaults to **copy** in every view mode (file-ops §3).
- Server-side copy is not assumed. Copying remote→remote on the same server streams through RustyFinder *(RustyFinder choice; may use `cp` via exec only when explicitly enabled)*.

## 4. Acceptance tests

Test fixture: a local OpenSSH `sshd` on a high port with a test user and a key pair, generated by the test harness.

- **SF-01** Given `Host t` with HostName, Port and User in `~/.ssh/config`, when the user connects with Server `t` and an empty Username, then the connection succeeds.
- **SF-02** Given an unknown host, when connecting, then a fingerprint dialog shows a `SHA256:` fingerprint. After Accept, known_hosts has the host, and the next connect doesn't ask.
- **SF-03** Given the host key changed, when connecting, then a warning is shown and nothing connects unless the user overrides.
- **SF-04** Given a passphrase-protected ed25519 key, when connecting, then the passphrase is asked once and the connection succeeds.
- **SF-05** Given a server requiring keyboard-interactive with a second prompt "Verification code", then RustyFinder shows that prompt and connects after a correct code.
- **SF-06** Given a connection with no Remote Path, then the first listing is the user's home directory.
- **SF-07** Given an unresponsive server (the TCP connection accepted, then silence), when connecting, then an error appears within about 60 s and the UI stays responsive.
- **SF-08** Given a 100 MB download stopped at 30 % and restarted with Resume, then the final file's SHA-256 matches the source.
- **SF-09** Given a folder containing a file whose name is the bytes `0xff 0x41`, then the listing shows `�A` and the file can be downloaded.
- **SF-10** Given an upload replacing `index.html`, then at no point during the upload does a listing of the target folder lack `index.html`.
- **SF-11** Given the user disconnects, then no `ssh` child process of RustyFinder is left running for that host.
- **SF-12** Given a pasted `sftp://bob@example.org:2222/var/www` in Server, then protocol = SFTP, user = bob, port = 2222, host = example.org and path = /var/www.
- **SF-13** Given a file's mode is changed to 600 in Info, then `stat` on the server shows 600.
