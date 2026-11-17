import AppKit

/// Toolbar buttons for Compress, Sync, Compare and the user's tools
/// ("Show in toolbar"), and Open in Terminal on an SFTP server.
extension MainWindowController {
    enum AreaItem {
        static let compress = NSToolbarItem.Identifier("rustyfinder.compress")
        static let sync = NSToolbarItem.Identifier("rustyfinder.sync")
        static let compare = NSToolbarItem.Identifier("rustyfinder.compare")
        static let toolPrefix = "rustyfinder.tool."

        static func tool(_ id: UInt64) -> NSToolbarItem.Identifier {
            NSToolbarItem.Identifier("\(toolPrefix)\(id)")
        }

        static func toolId(_ item: NSToolbarItem.Identifier) -> UInt64? {
            item.rawValue.hasPrefix(toolPrefix) ? UInt64(item.rawValue.dropFirst(toolPrefix.count)) : nil
        }
    }

    /// Tools the user put in the toolbar, in their order.
    private static var toolbarTools: [ToolRecord] {
        ToolCenter.shared.tools.filter(\.inToolbar)
    }

    var areaDefaultToolbarItems: [NSToolbarItem.Identifier] {
        [AreaItem.compress, AreaItem.sync, AreaItem.compare] + Self.toolbarTools.map { AreaItem.tool($0.id) }
    }

    var areaAllowedToolbarItems: [NSToolbarItem.Identifier] {
        ToolCenter.shared.tools.map { AreaItem.tool($0.id) }
    }

    func areaToolbarItem(_ id: NSToolbarItem.Identifier) -> NSToolbarItem? {
        func button(_ label: String, _ symbol: String, _ action: Selector, tip: String) -> NSToolbarItem {
            let item = NSToolbarItem(itemIdentifier: id)
            item.label = label
            item.paletteLabel = label
            item.toolTip = tip
            item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
            item.target = self
            item.action = action
            item.isBordered = true
            return item
        }
        switch id {
        case AreaItem.compress:
            return button("Compress", "archivebox", #selector(compress(_:)), tip: "Compress the selection into a zip")
        case AreaItem.sync:
            return button("Sync", "arrow.triangle.2.circlepath", #selector(syncTo(_:)), tip: "Sync to the other pane…")
        case AreaItem.compare:
            return button("Compare", "doc.on.doc", #selector(compare(_:)), tip: "Compare two files…")
        default:
            guard let toolId = AreaItem.toolId(id), let tool = ToolCenter.shared.tool(id: toolId) else { return nil }
            let item = button(tool.name, "hammer", #selector(runToolbarTool(_:)), tip: "Run “\(tool.name)”")
            item.tag = Int(truncatingIfNeeded: toolId)
            return item
        }
    }

    @objc func runToolbarTool(_ sender: NSToolbarItem) {
        guard let toolId = AreaItem.toolId(sender.itemIdentifier) else { return }
        // The menu path reads the tool from the item it was sent by.
        let item = NSMenuItem()
        item.representedObject = toolId
        runTool(item)
    }

    /// Bring this window's toolbar in line with the tools: add the ones
    /// newly marked "Show in toolbar", drop removed ones, refresh names.
    func syncToolbarTools() {
        guard let toolbar = window?.toolbar else { return }
        let wanted = Self.toolbarTools
        // Removed or no longer wanted.
        for index in toolbar.items.indices.reversed() {
            let item = toolbar.items[index]
            guard let id = AreaItem.toolId(item.itemIdentifier) else { continue }
            if let tool = wanted.first(where: { $0.id == id }) {
                item.label = tool.name
                item.toolTip = "Run “\(tool.name)”"
            } else {
                toolbar.removeItem(at: index)
            }
        }
        // New ones go after the last tool button (or after Compare).
        for tool in wanted where !toolbar.items.contains(where: { $0.itemIdentifier == AreaItem.tool(tool.id) }) {
            let anchor = toolbar.items.lastIndex {
                AreaItem.toolId($0.itemIdentifier) != nil || $0.itemIdentifier == AreaItem.compare
            }
            toolbar.insertItem(withItemIdentifier: AreaItem.tool(tool.id), at: anchor.map { $0 + 1 } ?? toolbar.items.count)
        }
    }

    /// Called when the tool list changes.
    static func toolsChanged() {
        for controller in (NSApp.delegate as? AppDelegate)?.windowControllers ?? [] {
            controller.syncToolbarTools()
        }
    }

    // MARK: Open in Terminal on a server

    /// Only SFTP connections have a shell to open.
    func canOpenRemoteTerminal(_ list: FileListViewController) -> Bool {
        guard list.isRemote, !list.isArchive, let path = list.content.path else { return false }
        return AppEnvironment.shared.core.sshShellCommand(connection: list.conn, path: path) != nil
    }

    /// Open the Terminal app with an ssh session to the server, in the
    /// selected folder or the pane's.
    func openRemoteTerminal(_ list: FileListViewController) {
        guard let folder = list.selectedFolder ?? list.content.path,
              let command = AppEnvironment.shared.core.sshShellCommand(connection: list.conn, path: folder)
        else { return }
        if ExternalApps.isDryRun {
            ExternalApps.record("ssh: \(command)")
            return
        }
        // A .command file is what Terminal (and most others) run when
        // opened; it removes itself as it starts.
        let dir = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("RustyFinder-ssh")
        let script = dir.appendingPathComponent("\(UUID().uuidString).command")
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try "#!/bin/sh\nrm -f -- \"$0\"\n\(command)\n".write(to: script, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        } catch {
            return
        }
        ExternalApps.open([script], withAppAt: ExternalApps.terminalApp)
    }
}
