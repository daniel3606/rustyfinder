# Network shares: SMB, AFP, NFS (and VNC hand-off) (P1)

Unlike SFTP/FTP/S3, these protocols are **mounted by the operating system**. RustyFinder asks the system to mount the share, then browses the mount point as a local volume. Every local behavior then applies: file-ops, conflicts, tags, Quick Look, Spotlight (if the server indexes).

## 1. Connecting

- Connect panel protocols: **SMB**, **AFP**, **NFS**, plus **VNC** (which only hands the URL to the system Screen Sharing app).
- Fields: Server (host or IP), Share (optional path), Username, Password (SMB/AFP), Port (Advanced), Color.
- URLs pasted in Server (`smb://user@nas/Media`, `nfs://host/export`, `afp://…`) are parsed as in remote-sftp §1.
- Mounting uses the system network-mount service, without the system's own UI. RustyFinder supplies the credentials, and they are stored in the keychain.
- If no share is given for SMB, RustyFinder lists the server's shares and the user picks one ("Open Share"). The list matches what Finder shows: hidden `$` admin shares are excluded and names are written the same way.
- Failures show the system error code and a readable message ("Can't mount network drive: (code)").
- Bonjour-discovered servers (SMB/AFP) appear in the sidebar **Shares** group. Clicking one opens the share picker.

## 2. While mounted

- The mount appears in the sidebar under **Shares** with free space. Context menu: **Eject**.
- Panes treat the mount as a normal local location. Changes by others show up after Refresh (⌘R). RustyFinder's own changes (tags, renames) show up at once.
- Copying to a share keeps the modification date (file-ops §4). RustyFinder must not reset dates on SMB.
- AppleDouble companions on shares without native xattrs are handled as in edge-cases §2.
- Case-insensitive behavior is detected per mount (SMB is usually case-insensitive and case-preserving). Case-only renames use the temporary-name path when needed (edge-cases §1).
- Dropping the connection (server off) makes panes on that share show an error and offer to return to the starting directory. RustyFinder never hangs on a stale mount: file system calls on network mounts run off the UI thread with a timeout *(RustyFinder choice: 15 s)*.
- Ejecting a share that panes are still showing moves them to the starting directory first.

## 3. Favorites
A share favorite stores the URL and username, with the password in the keychain. Opening it mounts the share if needed, then shows the share path. A favorite to a share that is already mounted just opens the mount.

## 4. Acceptance tests

- **SM-01** Given the URL `smb://guest@localhost/Public` (a test Samba or macOS file sharing) entered, when Connect is clicked, then the share mounts and the pane shows its root.
- **SM-02** Given an SMB server with shares `Media`, `Backup`, `IPC$`, when connecting without a share, then the picker lists Media and Backup only.
- **SM-03** Given a file copied to an SMB share, then its modification date on the share equals the source's.
- **SM-04** Given the server goes offline while a pane shows the share, when the user refreshes, then an error appears within 15 s and the UI stays responsive.
- **SM-05** Given a mounted share, when Eject is chosen in the sidebar, then the share unmounts and any pane on it moves to the starting directory.
- **SM-06** Given an NFS URL `nfs://host/export`, then the export mounts and its contents list.
