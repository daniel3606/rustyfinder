import AppKit

/// The menu bar, built in code (the app has no nib). Items with a nil target
/// travel the responder chain: list commands land on the focused file list,
/// tab/pane/navigation commands on the window controller.
/// Default shortcuts follow docs/spec/keyboard-shortcuts.md.
enum MainMenu {
    private static let recentsDelegate = RecentFoldersMenuDelegate()
    private static let connectionsDelegate = ConnectionsMenuDelegate()
    private static let toolsDelegate = ToolsMenuDelegate()
    private static let workspacesDelegate = WorkspacesMenuDelegate()
    /// Commands > Tools, refilled whenever the tool list changes.
    private(set) static var toolsMenu: NSMenu?

    static func build() -> NSMenu {
        let main = NSMenu()
        for menu in [appMenu(), fileMenu(), editMenu(), commandsMenu(), viewMenu(), goMenu(), favoritesMenu()] {
            main.addItem(submenu(menu))
        }
        let window = windowMenu()
        main.addItem(submenu(window))
        NSApp.windowsMenu = window
        let help = helpMenu()
        main.addItem(submenu(help))
        NSApp.helpMenu = help
        return main
    }

    private static func submenu(_ menu: NSMenu) -> NSMenuItem {
        let item = NSMenuItem(title: menu.title, action: nil, keyEquivalent: "")
        item.submenu = menu
        return item
    }

    private static func item(
        _ title: String, _ action: Selector?, _ key: String = "",
        _ mods: NSEvent.ModifierFlags = .command, tag: Int = 0
    ) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.keyEquivalentModifierMask = mods
        item.tag = tag
        return item
    }

    private static func key(_ special: Int) -> String {
        String(UnicodeScalar(special)!)
    }

    private static func appMenu() -> NSMenu {
        let menu = NSMenu()
        menu.addItem(item("About RustyFinder", #selector(NSApplication.orderFrontStandardAboutPanel(_:))))
        menu.addItem(.separator())
        menu.addItem(item("Settings…", #selector(AppDelegate.showSettings(_:)), ","))
        menu.addItem(.separator())
        menu.addItem(item("Hide RustyFinder", #selector(NSApplication.hide(_:)), "h"))
        menu.addItem(item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]))
        menu.addItem(item("Show All", #selector(NSApplication.unhideAllApplications(_:))))
        menu.addItem(.separator())
        menu.addItem(item("Quit RustyFinder", #selector(NSApplication.terminate(_:)), "q"))
        return menu
    }

    private static func fileMenu() -> NSMenu {
        let menu = NSMenu(title: "File")
        menu.addItem(item("New Window", #selector(AppDelegate.newWindow(_:)), "n"))
        menu.addItem(item("New Tab", #selector(MainWindowController.newTab(_:)), "t"))
        menu.addItem(.separator())
        menu.addItem(item("New Folder", #selector(FileListViewController.newFolder(_:)), "n", [.command, .shift]))
        menu.addItem(item("New Folder with Selection", #selector(FileListViewController.newFolderWithSelection(_:)), "n", [.command, .control]))
        menu.addItem(item("New File", #selector(FileListViewController.newFile(_:)), "n", [.command, .option, .shift]))
        menu.addItem(.separator())
        menu.addItem(item("Open", #selector(FileListViewController.openSelection(_:)), "o"))
        menu.addItem(item("Open in New Tab", #selector(FileListViewController.openInNewTab(_:)), key(NSDownArrowFunctionKey), [.command, .option]))
        menu.addItem(item("Open in Other Pane", #selector(FileListViewController.openInOtherPane(_:)), key(NSDownArrowFunctionKey), [.command, .option, .shift]))
        menu.addItem(item("Edit", #selector(FileListViewController.editSelection(_:)), "e"))
        menu.addItem(.separator())
        menu.addItem(item("Get Info", #selector(MainWindowController.getInfo(_:)), "i"))
        menu.addItem(item("Calculate Folder Size", #selector(FileListViewController.calculateFolderSize(_:)), "s", [.command, .shift]))
        menu.addItem(item("Calculate All Folder Sizes", #selector(FileListViewController.calculateAllFolderSizes(_:))))
        menu.addItem(item("Quick Look", #selector(FileListViewController.toggleQuickLook(_:)), "y"))
        menu.addItem(item("Slideshow", #selector(FileListViewController.quickLookSlideshow(_:)), "y", [.command, .option]))
        menu.addItem(.separator())
        menu.addItem(item("Share…", #selector(FileListViewController.shareSelection(_:))))
        menu.addItem(item("Print…", #selector(FileListViewController.printSelection(_:)), "p"))
        menu.addItem(.separator())
        menu.addItem(item("Close Tab", #selector(MainWindowController.closeTab(_:)), "w"))
        menu.addItem(item("Close Window", #selector(NSWindow.performClose(_:)), "w", [.command, .shift]))
        menu.addItem(item("Close Other Tabs", #selector(MainWindowController.closeOtherTabs(_:)), "w", [.command, .option]))
        return menu
    }

    private static func editMenu() -> NSMenu {
        let menu = NSMenu(title: "Edit")
        // Titles are filled in on validation: "Undo Move to Trash".
        menu.addItem(item("Undo", FileTableView.undoAction, "z"))
        menu.addItem(item("Redo", FileTableView.redoAction, "z", [.command, .shift]))
        menu.addItem(.separator())
        menu.addItem(item("Cut", #selector(NSText.cut(_:)), "x"))
        menu.addItem(item("Copy", #selector(NSText.copy(_:)), "c"))
        menu.addItem(item("Paste", #selector(NSText.paste(_:)), "v"))
        menu.addItem(item("Move Items Here", #selector(FileListViewController.moveItemsHere(_:)), "v", [.command, .option]))
        menu.addItem(.separator())
        menu.addItem(item("Quick Select…", #selector(MainWindowController.quickSelect(_:)), "s"))
        menu.addItem(item("Select All", #selector(NSResponder.selectAll(_:)), "a"))
        menu.addItem(item("Select All Files", #selector(FileListViewController.selectAllFiles(_:))))
        menu.addItem(item("Select All Folders", #selector(FileListViewController.selectAllFolders(_:))))
        menu.addItem(item("Find", #selector(MainWindowController.find(_:)), "f"))
        menu.addItem(item("Deselect All", #selector(FileListViewController.deselectAll(_:)), "a", [.command, .option]))
        // Shown in place of Deselect All while Shift is held.
        let invert = item("Invert Selection", #selector(FileListViewController.invertSelection(_:)), "a", [.command, .option, .shift])
        invert.isAlternate = true
        menu.addItem(invert)
        return menu
    }

    private static func commandsMenu() -> NSMenu {
        let menu = NSMenu(title: "Commands")
        let f5 = key(NSF5FunctionKey), f6 = key(NSF6FunctionKey)
        menu.addItem(item("Copy to", #selector(MainWindowController.copyToOtherPane(_:)), f5, []))
        menu.addItem(item("Move to", #selector(MainWindowController.moveToOtherPane(_:)), f6, []))
        menu.addItem(item("Copy to as…", #selector(MainWindowController.copyToAs(_:))))
        menu.addItem(.separator())
        // Return is handled by the list itself, so a text field keeps it.
        menu.addItem(item("Rename", #selector(FileListViewController.rename(_:))))
        menu.addItem(item("Duplicate", #selector(FileListViewController.duplicate(_:)), "d"))
        menu.addItem(item("Make Alias", #selector(FileListViewController.makeAlias(_:)), "a", [.command, .control]))
        menu.addItem(item("Make Symlink", #selector(FileListViewController.makeSymlink(_:)), "a", [.command, .control, .shift]))
        menu.addItem(item("Make Alias in Other Pane", #selector(MainWindowController.makeAliasInOtherPane(_:))))
        menu.addItem(item("Make Symlink in Other Pane", #selector(MainWindowController.makeSymlinkInOtherPane(_:))))
        menu.addItem(.separator())
        let delete = String(UnicodeScalar(NSBackspaceCharacter)!)
        menu.addItem(item("Move to Trash", #selector(FileListViewController.moveToTrash(_:)), delete))
        menu.addItem(item("Delete Immediately…", #selector(FileListViewController.deleteImmediately(_:)), delete, [.command, .option]))
        menu.addItem(item("Move App to Trash with Related Files…", #selector(FileListViewController.deleteAppWithRelatedFiles(_:))))
        menu.addItem(item("Empty Trash…", #selector(MainWindowController.emptyTrash(_:)), delete, [.command, .shift]))
        menu.addItem(item("Eject", #selector(MainWindowController.ejectVolume(_:)), "e", [.command, .shift]))
        menu.addItem(.separator())
        menu.addItem(item("Extract", #selector(MainWindowController.extractHere(_:)), "x", [.command, .shift]))
        menu.addItem(item("Extract to", #selector(MainWindowController.extractTo(_:)), "x", [.command, .option, .shift]))
        menu.addItem(item("Compress", #selector(MainWindowController.compress(_:)), "m", [.command, .shift]))
        menu.addItem(item("Compress with Options…", #selector(MainWindowController.compressWithOptions(_:)), "m", [.command, .option, .shift]))
        menu.addItem(item("Show Archive Contents", #selector(MainWindowController.showArchiveContents(_:))))
        menu.addItem(.separator())
        let tags = NSMenu(title: "Tags")
        for (name, _) in MainActor.assumeIsolated({ TagStore.colors }) {
            let entry = item(name, #selector(FileListViewController.toggleColorTag(_:)))
            entry.representedObject = name
            tags.addItem(entry)
        }
        tags.addItem(.separator())
        tags.addItem(item("Tags…", #selector(FileListViewController.editTags(_:))))
        let tagsItem = NSMenuItem(title: "Tags", action: nil, keyEquivalent: "")
        tagsItem.submenu = tags
        menu.addItem(tagsItem)
        menu.addItem(item("Open in Terminal", #selector(MainWindowController.openInTerminal(_:)), "r", [.command, .shift]))
        menu.addItem(item("Checksum…", #selector(MainWindowController.checksum(_:))))
        let tools = NSMenu(title: "Tools")
        tools.delegate = toolsDelegate
        MainActor.assumeIsolated { ToolsMenuDelegate.fill(tools) }
        toolsMenu = tools
        let toolsItem = NSMenuItem(title: "Tools", action: nil, keyEquivalent: "")
        toolsItem.submenu = tools
        menu.addItem(toolsItem)
        let scm = NSMenu(title: "Source Control")
        scm.addItem(item("Add", #selector(MainWindowController.scmAdd(_:))))
        scm.addItem(item("Add All", #selector(MainWindowController.scmAddAll(_:))))
        scm.addItem(item("Commit…", #selector(MainWindowController.scmCommit(_:))))
        scm.addItem(.separator())
        scm.addItem(item("Pull", #selector(MainWindowController.scmPull(_:))))
        scm.addItem(item("Push", #selector(MainWindowController.scmPush(_:))))
        let scmItem = NSMenuItem(title: "Source Control", action: nil, keyEquivalent: "")
        scmItem.submenu = scm
        menu.addItem(scmItem)
        menu.addItem(.separator())
        menu.addItem(item("Sync to…", #selector(MainWindowController.syncTo(_:)), "s", [.command, .option, .shift]))
        menu.addItem(item("Compare…", #selector(MainWindowController.compare(_:))))
        return menu
    }

    private static func viewMenu() -> NSMenu {
        let menu = NSMenu(title: "View")
        menu.addItem(item("as Icons", #selector(FileListViewController.viewAsIcons(_:)), "1"))
        menu.addItem(item("as List", #selector(FileListViewController.viewAsList(_:)), "2"))
        menu.addItem(item("as Columns", #selector(FileListViewController.viewAsColumns(_:)), "3"))
        menu.addItem(.separator())
        menu.addItem(item("Show View Options", #selector(FileListViewController.showViewOptions(_:)), "j"))
        menu.addItem(item("Save View Settings as Defaults", #selector(FileListViewController.saveViewAsDefaults(_:))))
        menu.addItem(item("Restore Default View Settings", #selector(FileListViewController.restoreDefaultView(_:)), "r", [.command, .option]))
        menu.addItem(.separator())
        menu.addItem(item("Show SCM Status", #selector(MainWindowController.toggleScmStatus(_:))))
        menu.addItem(.separator())
        menu.addItem(item("Bigger", #selector(FileListViewController.makeBigger(_:)), "+"))
        // Cmd-= is the same key without Shift; accept it too.
        let biggerUnshifted = item("Bigger", #selector(FileListViewController.makeBigger(_:)), "=")
        biggerUnshifted.isHidden = true
        biggerUnshifted.allowsKeyEquivalentWhenHidden = true
        menu.addItem(biggerUnshifted)
        menu.addItem(item("Smaller", #selector(FileListViewController.makeSmaller(_:)), "-"))
        menu.addItem(.separator())

        let sort = NSMenu(title: "Sort By")
        // The spec's numbering: 1 Name, 2 Kind, 3 Date Added, 4 Date Modified,
        // 5 Date Created, 6 Size, 7 Tags, 8 Owner, 9 Group, 0 Permissions.
        let digits = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]
        for (i, column) in SortColumn.menuOrder.enumerated() {
            sort.addItem(item(column.menuTitle, #selector(FileListViewController.sortByMenu(_:)), digits[i], [.command, .control], tag: i))
        }
        sort.addItem(.separator())
        sort.addItem(item("Descending Order", #selector(FileListViewController.toggleDescending(_:)), "o", [.command, .option]))
        sort.addItem(item("Show Folders on Top", #selector(FileListViewController.toggleFoldersFirst(_:))))
        sort.addItem(item("Always Sort Folders Alphabetically", #selector(FileListViewController.toggleFoldersByName(_:))))
        let sortItem = NSMenuItem(title: "Sort By", action: nil, keyEquivalent: "")
        sortItem.submenu = sort
        menu.addItem(sortItem)

        let group = NSMenu(title: "Group By")
        for (i, key) in GroupBy.menuOrder.enumerated() {
            group.addItem(item(key.menuTitle, #selector(FileListViewController.groupByMenu(_:)), "\(i)", [.command, .control, .option], tag: i))
        }
        let groupItem = NSMenuItem(title: "Group By", action: nil, keyEquivalent: "")
        groupItem.submenu = group
        menu.addItem(groupItem)
        menu.addItem(item("Use Alternating Background", #selector(MainWindowController.toggleAlternatingRows(_:))))
        menu.addItem(.separator())

        menu.addItem(item("Single Pane Mode", #selector(MainWindowController.toggleSinglePane(_:)), "p", [.command, .option]))
        menu.addItem(item("Show Horizontal Panes", #selector(MainWindowController.toggleHorizontalPanes(_:))))
        menu.addItem(item("Switch Pane", #selector(MainWindowController.switchPane(_:))))
        menu.addItem(item("Swap Panes", #selector(MainWindowController.swapPanes(_:))))
        menu.addItem(item("Clone Pane", #selector(MainWindowController.clonePane(_:)), "c", [.command, .shift]))
        menu.addItem(.separator())

        menu.addItem(item("Hide Sidebar", #selector(MainWindowController.toggleSidebarVisible(_:)), "s", [.command, .option]))
        menu.addItem(item("Show Preview", #selector(MainWindowController.togglePreview(_:)), "p", [.command, .shift]))
        menu.addItem(item("Show Activities", #selector(MainWindowController.showActivitiesPane(_:)), "u", [.command, .shift]))
        menu.addItem(item("Show Logs", #selector(MainWindowController.showLog(_:))))
        menu.addItem(item("Show Tab Bar", #selector(MainWindowController.toggleTabBar(_:)), "t", [.command, .shift]))
        menu.addItem(item("Show Toolbar", #selector(NSWindow.toggleToolbarShown(_:)), "t", [.command, .option]))
        menu.addItem(item("Hide Title Bar", #selector(MainWindowController.toggleTitleBar(_:))))
        menu.addItem(item("Hide Path Bar", #selector(MainWindowController.togglePathBar(_:))))
        menu.addItem(item("Show Invisible Files", #selector(MainWindowController.toggleHidden(_:)), ".", [.command, .shift]))
        menu.addItem(.separator())
        menu.addItem(item("Refresh", #selector(MainWindowController.reload(_:)), "r"))
        menu.addItem(item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control]))
        return menu
    }

    private static func goMenu() -> NSMenu {
        let menu = NSMenu(title: "Go")
        menu.addItem(item("Back", #selector(MainWindowController.goBack(_:)), "["))
        menu.addItem(item("Forward", #selector(MainWindowController.goForward(_:)), "]"))
        menu.addItem(item("Enclosing Folder", #selector(MainWindowController.goUp(_:)), key(NSUpArrowFunctionKey)))
        menu.addItem(.separator())
        let places: [(String, String, NSEvent.ModifierFlags)] = [
            ("Home", "h", [.command, .shift]), ("Desktop", "d", [.command, .shift]),
            ("Documents", "o", [.command, .shift]), ("Downloads", "l", [.command, .option]),
            ("Library", "l", [.command, .option, .shift]), ("Applications", "a", [.command, .shift]),
            ("Utilities", "u", [.command, .option]),
        ]
        for (i, place) in places.enumerated() {
            menu.addItem(item(place.0, #selector(MainWindowController.goToPlace(_:)), place.1, place.2, tag: i))
        }
        menu.addItem(.separator())
        let recents = NSMenu(title: "Recent Folders")
        recents.delegate = recentsDelegate
        let recentsItem = NSMenuItem(title: "Recent Folders", action: nil, keyEquivalent: "")
        recentsItem.submenu = recents
        menu.addItem(recentsItem)
        menu.addItem(item("Quick Open…", #selector(MainWindowController.quickOpen(_:))))
        menu.addItem(item("Show Devices…", #selector(MainWindowController.showDevices(_:)), "d", .control))
        menu.addItem(item("Go to Folder…", #selector(MainWindowController.goToFolder(_:)), "g", [.command, .shift]))
        menu.addItem(item("Connect…", #selector(MainWindowController.connect(_:)), "k"))
        menu.addItem(item("Disconnect", #selector(MainWindowController.disconnect(_:))))
        let connections = NSMenu(title: "Connections")
        connections.delegate = connectionsDelegate
        let connectionsItem = NSMenuItem(title: "Connections", action: nil, keyEquivalent: "")
        connectionsItem.submenu = connections
        menu.addItem(connectionsItem)
        menu.addItem(.separator())
        menu.addItem(item("Sync Browsing", #selector(MainWindowController.toggleSyncBrowsing(_:)), "y", [.command, .shift]))
        return menu
    }

    private static func favoritesMenu() -> NSMenu {
        let menu = NSMenu(title: "Favorites")
        menu.addItem(item("Add to Favorites…", #selector(MainWindowController.addToFavorites(_:))))
        menu.addItem(item("New Favorite…", #selector(MainWindowController.newFavorite(_:))))
        menu.addItem(item("Save Search as Favorite…", #selector(MainWindowController.saveSearchAsFavorite(_:))))
        menu.addItem(item("Show Favorites", #selector(AppDelegate.showFavoriteManager(_:)), "f", [.command, .option]))
        menu.addItem(.separator())
        menu.addItem(item("Save Workspace…", #selector(MainWindowController.saveWorkspace(_:))))
        menu.addItem(item("Update Workspace", #selector(MainWindowController.updateWorkspace(_:))))
        let workspaces = NSMenu(title: "Workspaces")
        workspaces.delegate = workspacesDelegate
        let workspacesItem = NSMenuItem(title: "Workspaces", action: nil, keyEquivalent: "")
        workspacesItem.submenu = workspaces
        menu.addItem(workspacesItem)
        return menu
    }

    private static func helpMenu() -> NSMenu {
        let menu = NSMenu(title: "Help")
        menu.addItem(item("RustyFinder Help", #selector(AppDelegate.showRustyFinderHelp(_:)), "?"))
        menu.addItem(item("Welcome to RustyFinder", #selector(AppDelegate.showWelcome(_:))))
        return menu
    }

    private static func windowMenu() -> NSMenu {
        let menu = NSMenu(title: "Window")
        menu.addItem(item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"))
        menu.addItem(item("Zoom", #selector(NSWindow.performZoom(_:))))
        menu.addItem(.separator())
        menu.addItem(item("Next Tab", #selector(MainWindowController.nextTab(_:)), "\t", .control))
        menu.addItem(item("Previous Tab", #selector(MainWindowController.previousTab(_:)), "\t", [.control, .shift]))
        for i in 0..<5 {
            menu.addItem(item("Select Tab \(i + 1)", #selector(MainWindowController.selectTabNumber(_:)), "\(i + 1)", .control, tag: i))
        }
        menu.addItem(.separator())
        menu.addItem(item("Move Tab to Other Pane", #selector(MainWindowController.moveTabToOtherPane(_:)), "t", [.command, .option, .shift]))
        menu.addItem(item("Pin Tab", #selector(MainWindowController.togglePinTab(_:)), "p", [.command, .option, .shift]))
        menu.addItem(.separator())
        return menu
    }
}

/// Fills Favorites > Workspaces. Choosing one replaces the front window;
/// with Option it opens in a new window.
final class WorkspacesMenuDelegate: NSObject, NSMenuDelegate {
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let all = MainActor.assumeIsolated { AppEnvironment.shared.core.workspaces() }
        if all.isEmpty {
            menu.addItem(NSMenuItem(title: "No Workspaces", action: nil, keyEquivalent: ""))
            return
        }
        for workspace in all {
            let item = NSMenuItem(title: workspace.name, action: #selector(AppDelegate.openWorkspaceFromMenu(_:)), keyEquivalent: "")
            item.representedObject = workspace.id
            menu.addItem(item)
            let other = NSMenuItem(title: "\(workspace.name) in New Window", action: #selector(AppDelegate.openWorkspaceFromMenu(_:)), keyEquivalent: "")
            other.representedObject = workspace.id
            other.isAlternate = true
            other.keyEquivalentModifierMask = .option
            menu.addItem(other)
        }
    }
}

/// Fills Go > Connections with the open remote sessions. Reachable even
/// when the sidebar is hidden.
final class ConnectionsMenuDelegate: NSObject, NSMenuDelegate {
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let open = ConnectionCenter.shared.connections
        if open.isEmpty {
            menu.addItem(NSMenuItem(title: "No Connections", action: nil, keyEquivalent: ""))
            return
        }
        for connection in open {
            let item = NSMenuItem(title: connection.label, action: #selector(MainWindowController.goToConnection(_:)), keyEquivalent: "")
            item.representedObject = connection
            menu.addItem(item)
        }
    }
}

/// Fills Go > Recent Folders each time it opens. Reachable even when the
/// sidebar (or its Recent Folders group) is hidden.
final class RecentFoldersMenuDelegate: NSObject, NSMenuDelegate {
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let recents = AppEnvironment.shared.core.recentFolders()
        if recents.isEmpty {
            menu.addItem(NSMenuItem(title: "No Recent Folders", action: nil, keyEquivalent: ""))
            return
        }
        for path in recents {
            let item = NSMenuItem(title: path.abbreviatingHome, action: #selector(MainWindowController.goToRecent(_:)), keyEquivalent: "")
            item.representedObject = path
            menu.addItem(item)
        }
    }
}
