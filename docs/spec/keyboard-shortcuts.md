# Keyboard shortcuts (P0)

RustyFinder MUST be fully usable from the keyboard. Every menu command can be rebound. The table below is the **default binding set**. Bindings act on the active pane unless noted.

## 1. Default bindings

### App / Window
| Command | Key |
|---|---|
| Settings | ⌘, |
| Hide RustyFinder / Hide Others | ⌘H / ⌥⌘H |
| Quit | ⌘Q |
| New Window | ⌘N |
| New Tab | ⌘T |
| Close Tab / Close Window | ⌘W / ⌘⇧W |
| Close Other Tabs | ⌥⌘W |
| Minimize | ⌘M |
| Enter Full Screen | ⌃⌘F |
| Select tab 1–5 | ⌃1 … ⌃5 |
| Next tab / Previous tab | ⌃Tab / ⌃⇧Tab |
| Move Tab to Other Pane | ⌥⌘⇧T |
| Pin Tab | ⌥⌘⇧P |
| Help | ⌘? |

### File
| Command | Key |
|---|---|
| New Folder | ⌘⇧N |
| New Folder with Selection | ⌃⌘N |
| New File | ⌥⌘⇧N |
| Open | ⌘O, ⌘↓ |
| Open in New Tab | ⌥⌘↓ |
| Open in Other Pane | ⌥⇧⌘↓ |
| Get Info | ⌘I |
| Quick Look | ⌘Y (and Space) |
| Slideshow | ⌥⌘Y |
| Print | ⌘P |
| Eject | ⌘⇧E |
| Calculate Folder Size | ⌘⇧S |
| Open in Terminal | ⌘⇧R |
| Edit (external / remote editor) | ⌘E |

### Edit
| Command | Key |
|---|---|
| Undo / Redo | ⌘Z / ⌘⇧Z |
| Cut / Copy / Paste | ⌘X / ⌘C / ⌘V |
| Move Items Here | ⌥⌘V |
| Select All / Deselect All | ⌘A / ⌥⌘A |
| Invert Selection | menu (shown as an alternate of Deselect All) |
| Quick Select | ⌘S |
| Find (search field) | ⌘F |

### Commands (file operations)
| Command | Key |
|---|---|
| Copy to | F5 |
| Move to | F6 |
| Rename | Return |
| Duplicate | ⌘D |
| Make Alias / Make Symlink | ⌃⌘A / ⌃⌘⇧A |
| Move to Trash | ⌘⌫ |
| Delete immediately | ⌥⌘⌫ |
| Empty Trash | ⇧⌘⌫ |
| Compress / Compress with Options | ⌘⇧M / ⌥⌘⇧M |
| Extract / Extract to | ⌘⇧X / ⌥⌘⇧X |
| Sync to… | ⌥⌘⇧S |

### View
| Command | Key |
|---|---|
| as Icons / as List / as Columns | ⌘1 / ⌘2 / ⌘3 |
| Sort by Name, Kind, Date Added, Date Modified, Date Created, Size, Tags, Owner, Group, Permissions | ⌃⌘1 … ⌃⌘9, ⌃⌘0 |
| Descending order | ⌥⌘O |
| Group by None, Kind, Date Added, Date Modified, Date Created, Size, Tags | ⌃⌥⌘0, ⌃⌥⌘1 … ⌃⌥⌘6 |
| Show View Options | ⌘J |
| Restore Default View Settings | ⌥⌘R |
| Single Pane Mode | ⌥⌘P |
| Show/Hide Sidebar | ⌥⌘S |
| Show/Hide Tab Bar | ⌘⇧T |
| Show/Hide Toolbar | ⌥⌘T |
| Show/Hide Preview | ⌘⇧P |
| Show Activities | ⌘⇧U |
| Show Devices (pop-up) | ⌃D |
| Show Invisible Files | ⇧⌘. |
| Clone Pane | ⌘⇧C |
| Refresh | ⌘R |
| Bigger / smaller text or icons | ⌘+ / ⌘− |
| Expand / collapse folder (List view) | ⌘→ / ⌘← |

### Go
| Command | Key |
|---|---|
| Back / Forward | ⌘[ / ⌘] (mouse buttons 4 / 5) |
| Enclosing Folder | ⌘↑ |
| Home | ⌘⇧H |
| Desktop | ⌘⇧D |
| Documents | ⌘⇧O |
| Downloads | ⌥⌘L |
| Library | ⌥⌘⇧L |
| Applications | ⌘⇧A |
| Utilities | ⌥⌘U |
| Go to Folder | ⌘⇧G |
| Connect | ⌘K |
| Sync Browsing | ⌘⇧Y |
| Quick Open | **Esc** when the file list has focus (see quick-open.md) |

### Favorites
| Command | Key |
|---|---|
| Show Favorites (favorite manager) | ⌥⌘F |

Note: **Esc keeps its normal meaning** (cancel or close) in every text field, pop-up, sheet, dialog and inline rename. It opens Quick Open only when the file list itself has focus and nothing else would use the key.

## 2. Customization (Settings → Shortcuts)

- The list shows every command grouped by menu, plus extra entries for **color tags** (assign or remove each color tag) and for every **custom tool** (tools.md).
- Clicking a binding records the next key combination. All modifier combinations are allowed, including **Option + letter** and bare keys (for example Delete or F-keys). An X button in the field clears the binding.
- **Conflict detection:** if the combination is already bound, RustyFinder says which command has it and offers **Reassign** (takes it from the other command) or **Cancel**.
- "Restore Defaults" resets all bindings. The binding of each command is also shown next to it in Quick Open.
- Custom bindings apply to the menu items, so the menus always show the current binding.
- A binding made of Option plus a letter disables type-select for that letter combination only. Typing whole words still type-selects.
- A custom binding for **Move to Trash** that includes Option still moves to the Trash (Option on the *default* Delete binding means "delete immediately" only for the built-in variants).
- While inline rename or any text field is active, bare-key bindings (such as Delete) are **not** triggered.

## 3. Acceptance tests

- **KS-01** Given default bindings, when F5 is pressed with a file selected, then Copy to runs.
- **KS-02** Given ⌘⇧N is rebound to New File, when the user tries to bind New Folder to ⌘⇧N, then RustyFinder reports the conflict and Reassign moves the binding.
- **KS-03** Given Move to Trash is rebound to ⌥⌫, when it is pressed on a file, then the file goes to the Trash and is not deleted permanently.
- **KS-04** Given Delete-immediately is bound to the bare Delete key, when the user presses Delete during inline rename, then a character is erased and no file is deleted.
- **KS-05** Given the Go to Folder pop-up is open, when Esc is pressed, then the pop-up closes and Quick Open does not open.
- **KS-06** Given a custom tool with shortcut ⌃⌥T, when it is pressed with a file selected, then the tool runs on that file.
- **KS-07** Given Restore Defaults, then every binding matches §1.
- **KS-08** Given mouse button 4 is clicked over a pane, then that pane goes back in its history.
