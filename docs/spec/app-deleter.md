# App Deleter (P2)

## 1. Trigger

- Setting "Detect application when deleting" (General; default off *(RustyFinder choice)*).
- When it is on, and the user moves an `.app` bundle to the Trash (⌘⌫, menu, toolbar, or a drag onto the Dock Trash from RustyFinder), RustyFinder opens the **App Deleter** window instead of trashing at once. Only one app can be handled at a time. With several apps selected, they are handled one after another.
- The window is never opened for permanent Delete (⌥⌘⌫). That deletes only the bundle, after the usual confirmation.

## 2. Finding related items

RustyFinder reads the app's **bundle identifier**, **name** and **team identifier** (from its signature), then searches these locations in the user domain (and the local domain `/Library/...` when readable):

- Application Support, Caches, Preferences (plist files, including ByHost), Containers, Group Containers (matched by team ID prefix), Saved Application State, Logs (including DiagnosticReports for the app), LaunchAgents (user), HTTPStorages, WebKit, Cookies, Application Scripts.

Matching rules:
- An exact bundle-ID match (the file or folder name equals the bundle ID, or starts with the bundle ID followed by a dot) is **selected** by default.
- A name match (equals the app name, case-insensitive) is selected by default **only if** the app name is at least 4 characters and not a generic word *(RustyFinder choice)*. Otherwise it is listed **unselected**.
- Partial or fuzzy matches are listed unselected.
- Items belonging to Apple, or to the system domain (`/System`), are never listed.
- The search must finish within a few seconds and show a spinner meanwhile.

## 3. Window

- Header: the app icon, name and version, plus a warning to review the list carefully (apps with short names or several installed versions can match too much).
- A list of items with checkbox, icon, name, path (abbreviated with `~`) and **size**. The app bundle itself is first and checked.
- The footer shows the total size of the checked items.
- Buttons: **Cancel**, **Move to Trash**. Move to Trash sends all checked items to the Trash as one undoable activity (undo puts all of them back).
- Items RustyFinder can't move without administrator rights trigger the privileged-helper prompt (file-ops §1).

## 4. Acceptance tests

- **AD-01** Given the setting is on and `Foo.app` (bundle ID `com.example.foo`) with `~/Library/Preferences/com.example.foo.plist` and `~/Library/Caches/com.example.foo/`, when Foo.app is moved to the Trash, then the App Deleter lists the app and both items, all checked, with sizes.
- **AD-02** Given an app named `Go` and a folder `~/Library/Application Support/Go`, then that folder is listed **unchecked**.
- **AD-03** Given the user unchecks the cache folder and clicks Move to Trash, then the app and the plist are in the Trash and the cache folder is untouched.
- **AD-04** Given ⌘Z right after, then all trashed items return to their original locations.
- **AD-05** Given the setting is off, when an app is moved to the Trash, then it is trashed directly with no window.
