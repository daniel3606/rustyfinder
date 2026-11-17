# Tools: custom commands, Terminal, checksum, git, misc (P1)

## 1. Custom tools (Settings → Tools)

A **tool** is a user-defined shell command that runs on the current selection.

### 1.1 Definition fields
- **Name** (required. A new tool is called "Untitled" until renamed, and renaming updates every list at once.)
- **Command** (required, multi-line allowed). An empty command can't be saved.
- **Show output** (on/off): when on, stdout and stderr appear in an output window. When off, the tool runs silently and only failures are reported.
- **Run for every selected file** (on/off): when on, the command runs once per selected item, with the per-item placeholders filled for that item. When off, it runs once with lists.
- **Keyboard shortcut** (optional; conflicts are checked as in keyboard-shortcuts.md §2).
- **Show in toolbar** (adds a toolbar item for the tool) and **Show in context menu** *(RustyFinder choice: on by default)*.
- A **placeholder palette** ("Drag placeholders into your command from here") inserts tokens by drag or click.

### 1.2 Placeholders
| Token | Value |
|---|---|
| `$SOURCE_PATH` | Active pane's current folder (local path, or remote path) |
| `$TARGET_PATH` | Other pane's current folder |
| `$SOURCE_SELECTION_PATHS` | Full paths of the selected items in the active pane |
| `$SOURCE_SELECTION_NAMES` | Names of the selected items in the active pane |
| `$TARGET_SELECTION_PATHS` / `$TARGET_SELECTION_NAMES` | The same for the other pane's selection |
| `$SOURCE_HOSTNAME` / `$SOURCE_USERNAME` | Host and user of the active pane's remote connection (empty for local) |

**Quoting:** every path or name substituted is shell-quoted as **one argument**. Lists expand to several separately quoted arguments. Spaces, quotes, `$`, backticks, newlines and non-ASCII characters in names must never split or inject arguments.

### 1.3 Execution
- Runs through the user's login shell (`$SHELL -l -c`), so commands are evaluated exactly as in Terminal: aliases aren't needed, but `$PATH`, globbing and pipes behave the same. "Inherit $PATH from the shell" is on by default.
- The working directory is the active pane's folder if it is local, otherwise the home folder.
- Each run is an activity. Stop sends SIGTERM, then SIGKILL after 5 s *(RustyFinder choice)*.
- A non-zero exit shows "Task exit code: N" in the output window, or as an alert when output is hidden.
- Output windows are ordinary windows. Closing one must not leave stray windows behind (Stage Manager).
- Tools appear in: the context menu of items, the Commands menu (or a Tools submenu), Quick Open → Execute Command, the toolbar (if enabled), and their keyboard shortcut.

### 1.4 Management
In Settings: add (+), remove (−) with confirmation ("delete N tools?"), duplicate, edit, multi-select, and reorder by drag. The Edit and − buttons are disabled with no selection. Esc in the editor while creating a new tool **cancels** the creation, and never deletes an existing tool. New tools are added at the end of the list.

## 2. Open in Terminal (⌘⇧R)
Opens the Terminal application chosen in Settings (Terminal, iTerm, Hyper, Kitty, Warp, Ghostty, or another app) at the active pane's local folder. If a folder is selected, it opens at that folder *(RustyFinder choice)*. Remote locations: for SFTP, it opens an `ssh` session to the host and changes to the remote path *(RustyFinder choice)*. For other protocols, the command is disabled.

## 3. Checksum
- Commands → Checksum… on selected files opens a Checksum window.
- Algorithms: **MD5, SHA-1, SHA-256, SHA-512** (selector; default SHA-256 *(RustyFinder choice)*).
- One row per file: name, algorithm, digest (lowercase hex). Progress for large files, with cancel. Remote files are streamed (downloaded) to compute the digest.
- An optional "compare with" field highlights rows whose digest matches pasted text, ignoring case and whitespace.
- Rows can be multi-selected and copied. The copied text is **CSV** (`name,algorithm,digest` per line) so it pastes into spreadsheets.

## 4. Git (Source Control) support

- "Show SCM Status" (View menu / View Options): in a folder inside a git working tree, each item shows its status: modified, added, untracked, ignored, conflicted, clean. List view has an **SCM Status** column. Icon and Column view show a badge. Status comes from the system `git` (porcelain status) and refreshes after file changes. Outside a repo nothing is shown and no error appears.
- Commands (Source Control menu and context menu), each run as an activity "SCM Operation: …":
  - **Add** (stage the selected items), **Add All**
  - **Commit…** opens a sheet with a multi-line message field. Commit is disabled while the message is empty.
  - **Pull**, **Push**: progress and output go to the log. Authentication uses the user's git credential setup. Failures show git's message.
- Remote locations: SCM is not available.

## 5. Other tool-like commands
- **Get Info / permissions:** preview-quicklook.md.
- **Calculate Folder Size:** views.md §2.5.
- **Copy Dropbox Link:** cloud.md.
- **Share…:** system share sheet.

## 6. Acceptance tests

- **TL-01** Given the tool `echo $SOURCE_SELECTION_NAMES` with output on, and files `a b.txt` and `c"d.txt` selected, then the output is `a b.txt c"d.txt`, and the command got exactly 2 arguments after echo.
- **TL-02** Given "Run for every selected file" on and 3 files selected, then the command runs 3 times.
- **TL-03** Given a tool using a command installed in `/opt/homebrew/bin` (on the shell PATH but not the default GUI PATH), then it runs successfully.
- **TL-04** Given a tool that exits with code 2, then the output window shows exit code 2.
- **TL-05** Given a new tool being created, when Esc is pressed in the editor, then no tool is added and existing tools are unchanged.
- **TL-06** Given a file `hello` containing `hello\n`, then SHA-256 shows `5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03`.
- **TL-07** Given 2 rows are copied from the Checksum window, then the pasteboard has 2 CSV lines.
- **TL-08** Given a git repo with one modified tracked file and one untracked file, with SCM status shown, then those two items show modified and untracked respectively.
- **TL-09** Given Commit with an empty message, then the Commit button is disabled.
- **TL-10** Given the Terminal app is Terminal and the pane is at `/tmp/x y`, when ⌘⇧R runs, then a Terminal window opens with its working directory at `/tmp/x y`.
