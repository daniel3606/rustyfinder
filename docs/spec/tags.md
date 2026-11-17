# Tags (P2)

RustyFinder reads and writes **Finder tags** (the system's user tags extended attribute), so tags are shared with Finder and Spotlight. The legacy single color **label** is also read and shown as the matching color tag.

## 1. Tag list (Settings → Tags)

- Shows the user's tags with their colors, the 7 standard colors plus custom tags. The standard color tags are named in the macOS language.
- Each tag has a "show in sidebar" toggle (also in the sidebar tag's context menu: Hide from Sidebar).
- Context menu: **Rename** (renames the tag on every item that has it, found through Spotlight, with progress) and **Delete** (removes it from the list. RustyFinder asks whether to also remove it from all items *(RustyFinder choice)*. A multi-delete confirms the count).
- Drag to reorder. The order drives the sidebar and the order of tag dots in views.
- Each **color tag** can have a keyboard shortcut (keyboard-shortcuts.md §2). Pressing it toggles that tag on the selection.

## 2. Assigning tags

- **Context menu** of items: a row of color tag swatches, plus "Tags…". Clicking a swatch toggles that color tag on every selected item. The "Clear" pseudo-color is not offered as a tag. Adding a tag that an item already has never duplicates it.
- **Tags…** opens a pop-over with a token field. Autocompletion comes from the tag list (§1). Enter or Tab confirms a token. Removing a token removes the tag. Typing a new name creates a new tag. Esc leaves the field.
- **Preview pane / Info:** an editable, expandable Tags field with the same autocompletion. It shows **all** tags of the item.
- **Drop on a sidebar tag:** dropping items on a tag in the sidebar adds that tag to them.
- Assigning tags is immediate. There's no Apply button. Each change is one undo step.
- Works on local volumes, **external drives**, and **iCloud Drive**. Removing the last remaining tag of an item on iCloud Drive must work. Tags on items in remote protocols (SFTP and so on) are not supported, and the commands are disabled there.

## 3. Displaying tags
- Up to three color dots per item in all view modes (views.md §2.2). A **Tags** column in List view. Grouping and sorting by Tags.
- On iCloud Drive, tag colors must show their real colors (not gray).

## 4. Browsing by tag
- Sidebar **Tags** group: clicking a tag lists every item with that tag (Spotlight query over the home folder and mounted indexed volumes). The tab shows the tag icon and name. The sidebar selection stays on that tag while the listing is shown.
- Opening a result with "Open in New Tab" shows its real parent path.
- Tag listings don't appear in Recent Folders.

## 5. Acceptance tests

- **TG-01** Given a file and the Red swatch clicked twice, then the file ends with no Red tag (toggle), and Finder agrees.
- **TG-02** Given 3 files selected and "Work" added in the Tags pop-over, then all 3 have "Work". Typing "Wo" offered "Work" as a completion.
- **TG-03** Given a file with tag Blue, when Blue is added again through the pop-over, then the file still has exactly one Blue.
- **TG-04** Given files dropped on the sidebar tag "Urgent", then they get the tag.
- **TG-05** Given the tag "Old" renamed to "Archive" in Settings, then every item formerly tagged "Old" now has "Archive".
- **TG-06** Given the Green color tag bound to ⌃⌥G, when it is pressed on 2 selected files, then both get Green.
- **TG-07** Given an iCloud Drive file with a single tag, when the tag is removed, then the file has no tags.
- **TG-08** Given a file with an old-style color label (orange) and no tags, then an orange dot is shown.
