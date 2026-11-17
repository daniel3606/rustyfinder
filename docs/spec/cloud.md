# Cloud services: Google Drive, Dropbox, OneDrive, iCloud Drive (P2)

## 1. OAuth providers (Google Drive, Dropbox, OneDrive)

### 1.1 Connecting
1. In the Connect panel, choose the provider and click Connect. The panel has no username or password fields for these providers.
2. RustyFinder opens the provider's authorization page in the user's **default browser** (OAuth 2.0 authorization code with PKCE) and shows "Log into your … account in your browser to authorize RustyFinder" with a Cancel button.
3. The browser hands control back to RustyFinder through a registered custom URL scheme (redirect). RustyFinder exchanges the code for tokens.
4. The **refresh token** is stored in the keychain. The macOS keychain may ask the user to allow this. If the user denies keychain storage, the connection still works for the session but won't survive a relaunch.
5. "Add to Favorites" runs the same flow and saves the favorite with the account's display name.

Access tokens are refreshed silently when they expire. If the refresh is rejected (revoked), the user is asked to authorize again.

### 1.2 Shared rules
- Quotas and rate limits: replies like 429 or "rate limit exceeded" are retried after the server's stated delay (file-ops §5.5).
- Names with any Unicode characters (Japanese, Cyrillic, emoji) must upload, download and rename without errors.
- Transfers use resumable or chunked upload sessions for large files (Google: resumable upload; Dropbox: upload session; OneDrive: upload session), so they survive transient failures by resuming the session.
- Many parallel operations (bulk delete of thousands of items) must not lose items: requests are throttled and retried until every item is done or definitively failed.

### 1.3 Google Drive
- The root shows **My Drive**, **Shared with me**, and **Shared Drives**. Opening Shared Drives lists them and they can be browsed.
- Drive allows duplicate names in one folder. RustyFinder shows duplicates as separate rows *(RustyFinder choice: append " (2)" to the display name only)*. Operations target the right file ID.
- Google-native documents (Docs, Sheets, Slides) are shown with their type. Downloading exports them to the matching Office format (docx, xlsx, pptx) *(RustyFinder choice)*. They have no size.
- Folder listings must not recurse unexpectedly. Copying folders recreates the structure and the contents.
- Replacing a folder (conflict choice Replace) works.

### 1.4 Dropbox
- Personal and **Dropbox Teams** (team spaces and member folders) are supported.
- **Copy Dropbox Link**: for items inside the user's **local** Dropbox folder (or on a Dropbox connection), the context menu offers to create or get a shared link and copy it to the pasteboard.
- Too many simultaneous operations must not cause failures (batch and throttle).

### 1.5 OneDrive (personal and business)
- Folders and files list once each (no duplicates). Items can be renamed in the root.
- Downloaded folders contain their files.
- Transfers retry on OneDrive's transient errors (5xx, throttling), with back-off.

## 2. iCloud Drive (local, through the system)

iCloud Drive is browsed as a local folder (`~/Library/Mobile Documents` and the Desktop/Documents if synced). RustyFinder adds:
- **Download state badges:** a cloud icon on items that live only in the cloud, in List, Column and Icon views, and in the preview pane.
- **Download Now** (context menu, or the cloud button in the preview pane): asks the system to download the item and shows progress.
- **Remove Download** (context menu; works on several selected items at once): evicts the local copy and keeps it in iCloud. Failure gives "Unable to Remove Download".
- Opening, previewing or Quick Looking a cloud-only **media** file must not download it unless the user asks. A placeholder icon is shown instead.
- If iCloud reports an item as busy, an alert explains why the action isn't possible right now (edge-cases §4).
- Tags work, with true colors (tags.md).

## 3. Acceptance tests

- **CL-01** Given Google Drive is chosen and Connect clicked, then the default browser opens the authorization page. After approval, RustyFinder lists My Drive, Shared with me and Shared Drives.
- **CL-02** Given a revoked Dropbox token, when a Dropbox favorite is opened, then RustyFinder asks to authorize again rather than showing a generic error.
- **CL-03** Given a file named `日本語テスト.txt`, when it is uploaded to Dropbox, then it succeeds and the name matches.
- **CL-04** Given 2,000 files deleted from a Dropbox folder, then all 2,000 are gone when the activity finishes, or the failures are listed.
- **CL-05** Given an iCloud Drive file that is cloud-only, then it shows a cloud badge. After "Download Now" the badge goes away.
- **CL-06** Given 3 downloaded iCloud files and "Remove Download", then all 3 become cloud-only.
- **CL-07** Given a cloud-only iCloud video selected with the preview pane open, then no download starts automatically.
- **CL-08** Given a file in `~/Dropbox`, then "Copy Dropbox Link" puts a `https://www.dropbox.com/…` link on the pasteboard.
