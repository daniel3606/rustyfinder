import AppKit
import DiskArbitration

/// Navigation chrome of a browser window: tab dragging and drops, the
/// title and path bars, volumes going away, ejecting, restored server
/// tabs reconnecting, and the scripted-check hooks for all of it.
extension MainWindowController {
    /// Called once at the end of `init`.
    func installChrome() {
        let workspace = NSWorkspace.shared.notificationCenter
        workspace.addObserver(self, selector: #selector(volumeDidUnmount(_:)), name: NSWorkspace.didUnmountNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(themeChanged), name: .rustyfinderThemeChanged, object: nil)
        let icon: (UInt64) -> (image: NSImage, key: String)? = { [weak self] tab in self?.tabIcon(tab) }
        left.tabIconOverride = icon
        right.tabIconOverride = icon
        sidebar.host = self
        // New windows take the last-changed title and path bar choice.
        if chrome.pathBarHidden != Prefs.bool("view.pathBarHidden") { setPathBarHidden(Prefs.bool("view.pathBarHidden")) }
        if Prefs.bool("view.titleBarHidden") { setTitleHidden(true) }
        window?.appearance = nil
    }

    // MARK: Tab icons

    /// Search, tag and favorite tabs have icons of their own.
    private func tabIcon(_ tab: UInt64) -> (image: NSImage, key: String)? {
        if let tag = chrome.tagTabs[tab], searchTitles[tab] != nil {
            let image = NSImage(systemSymbolName: "tag", accessibilityDescription: "Tag")!
            if let color = TagStore.color(for: tag) {
                let tinted = image.copy() as! NSImage
                tinted.isTemplate = false
                tinted.lockFocus()
                color.set()
                NSRect(origin: .zero, size: tinted.size).fill(using: .sourceAtop)
                tinted.unlockFocus()
                return (tinted, "tag")
            }
            return (image, "tag")
        }
        if searchTitles[tab] != nil {
            return (NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: "Search")!, "search")
        }
        guard let icon = chrome.tabIcons[tab], let record = (layout.left.tabs + layout.right.tabs).first(where: { $0.id == tab }) else {
            return nil
        }
        // The favorite's icon stays while the tab is inside its location.
        let inside = record.conn == icon.conn
            && (icon.root == "/" || record.path == icon.root || record.path.hasPrefix(icon.root + "/"))
        return inside ? (icon.image, icon.key) : nil
    }

    // MARK: Tab dragging and drops

    var paneWindowId: UInt64 { session.id() }

    func pane(_ side: PaneSide, move tab: UInt64, to index: Int) {
        session.moveTab(tab: tab, side: side, index: UInt32(max(0, index)))
    }

    /// Files dropped on a tab (into its location) or on a path-bar segment.
    func pane(_ side: PaneSide, drop paths: [String], onTab tab: UInt64?, path: String?, modifiers: NSEvent.ModifierFlags) -> Bool {
        let record = tab.flatMap { id in (layout.left.tabs + layout.right.tabs).first { $0.id == id } }
        let conn: UInt64
        let target: String
        if let record {
            guard record.pending.isEmpty else { return false }
            conn = record.conn
            target = fileList(for: record.id)?.content.path ?? record.path
        } else if let path {
            conn = fileList(for: currentTab(side))?.conn ?? 0
            target = path
        } else {
            return false
        }
        // Nothing goes into an archive or a search listing.
        if conn != 0, ArchiveCenter.shared.isArchive(conn) { return false }
        if let id = record?.id, fileList(for: id)?.search != nil { return false }
        guard let mode = DropRules.mode(sources: paths, target: target, modifiers: modifiers, remote: conn != 0) else { return false }
        chrome.lastDrop = "\(mode == .move ? "move" : "copy") \(paths.count) -> \(target.folderDisplayName)"
        DropRules.perform(sources: paths, target: target, mode: mode, window: window, windowId: session.id(), dst: conn)
        return true
    }

    /// PT-08: each folder dropped on "+" opens in a tab of its own.
    func pane(_ side: PaneSide, openTabsFor folders: [String]) {
        for folder in folders { _ = session.newTab(side: side, path: folder) }
    }

    // MARK: Title bar and path bar

    @objc func toggleTitleBar(_ sender: Any?) {
        setTitleHidden(!chrome.titleHidden)
        Prefs.set("view.titleBarHidden", chrome.titleHidden)
        scheduleSave()
    }

    func setTitleHidden(_ hidden: Bool) {
        chrome.titleHidden = hidden
        window?.titleVisibility = hidden ? .hidden : .visible
        window?.titlebarAppearsTransparent = hidden
    }

    @objc func togglePathBar(_ sender: Any?) {
        setPathBarHidden(!chrome.pathBarHidden)
        Prefs.set("view.pathBarHidden", chrome.pathBarHidden)
        scheduleSave()
    }

    func setPathBarHidden(_ hidden: Bool) {
        chrome.pathBarHidden = hidden
        left.setPathBarHidden(hidden)
        right.setPathBarHidden(hidden)
    }

    /// Menu titles for the chrome commands; nil for actions it does not own.
    func validateChrome(_ item: NSValidatedUserInterfaceItem) -> Bool? {
        let menuItem = item as? NSMenuItem
        switch item.action {
        case #selector(toggleTitleBar(_:)):
            menuItem?.title = chrome.titleHidden ? "Show Title Bar" : "Hide Title Bar"
            return true
        case #selector(togglePathBar(_:)):
            menuItem?.title = chrome.pathBarHidden ? "Show Path Bar" : "Hide Path Bar"
            return true
        case #selector(saveSearchAsFavorite(_:)):
            return activeList?.search != nil && chrome.searches[activeTab] != nil
        case #selector(newFavorite(_:)):
            return window?.attachedSheet == nil && activeList?.isRemote == false
        case #selector(openSelectionInNewWindow(_:)):
            return activeList?.selectedFolder != nil && activeList?.isRemote == false
        default:
            return nil
        }
    }

    // MARK: Opening

    /// "Open in New Window" in the item context menu.
    @objc func openSelectionInNewWindow(_ sender: Any?) {
        let path = ((sender as? NSMenuItem)?.representedObject as? String) ?? activeList?.selectedFolder
        guard let path else { return }
        (NSApp.delegate as? AppDelegate)?.openWindow(restoring: nil, leftPath: path)
    }

    /// A recent folder on a server: go there on that connection.
    func openRecent(_ path: String, conn: UInt64) {
        session.navigateTo(tab: activeTab, conn: conn, path: path)
    }

    /// Tags were changed outside the lists (a drop on a sidebar tag, a
    /// rename in Settings): local lists read them again.
    func tagsChangedOnDisk() {
        for side in [PaneSide.left, .right] {
            let tab = currentTab(side)
            if fileList(for: tab)?.isRemote == false { session.reload(tab: tab) }
        }
    }

    @objc private func themeChanged() {
        sidebar.applyTheme()
        apply(layout)
    }

    // MARK: Volumes going away

    /// PT-15: a drive unplugged without ejecting. Its tabs go to the
    /// starting directory, without an error.
    @objc private func volumeDidUnmount(_ notification: Notification) {
        guard let url = notification.userInfo?[NSWorkspace.volumeURLUserInfoKey] as? URL else { return }
        volumeGone(url.path)
    }

    func volumeGone(_ path: String) {
        let moved = session.leaveVolume(root: path)
        chrome.lastVolumeEvent = "left \(path.folderDisplayName): \(moved)"
    }

    /// Other mounted volumes on the same disk as `path`.
    private func volumesOnSameDisk(_ path: String) -> [String] {
        guard let session = DASessionCreate(kCFAllocatorDefault) else { return [path] }
        func wholeDisk(_ volume: String) -> String? {
            guard let disk = DADiskCreateFromVolumePath(kCFAllocatorDefault, session, URL(fileURLWithPath: volume) as CFURL),
                  let whole = DADiskCopyWholeDisk(disk), let name = DADiskGetBSDName(whole)
            else { return nil }
            return String(cString: name)
        }
        guard let mine = wholeDisk(path) else { return [path] }
        let volumes = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: nil, options: [.skipHiddenVolumes]) ?? []
        return volumes.map(\.path).filter { wholeDisk($0) == mine }
    }

    /// Eject from the sidebar. A disk with several volumes asks "this one
    /// or all" unless Option (all) or Control (this one) is held (PT-13).
    /// Panes showing what goes away move off it first (SM-05).
    func eject(_ path: String, modifiers: NSEvent.ModifierFlags, siblings forced: [String]? = nil) {
        let siblings = forced ?? volumesOnSameDisk(path)
        switch ejectPlan(volumesOnDisk: UInt32(siblings.count), option: modifiers.contains(.option), control: modifiers.contains(.control)) {
        case .all:
            unmount(siblings.count > 1 ? siblings : [path], primary: path, wholeDisk: true)
        case .one:
            unmount([path], primary: path, wholeDisk: false)
        case .ask:
            guard let window else { return }
            let alert = NSAlert()
            alert.messageText = "“\(path.folderDisplayName)” is on a disk with \(siblings.count) volumes."
            alert.informativeText = "Eject only this volume, or every volume of the disk?"
            alert.addButton(withTitle: "Eject All")
            alert.addButton(withTitle: "Eject “\(path.folderDisplayName)”")
            alert.addButton(withTitle: "Cancel")
            chrome.ejectPrompt = alert
            alert.beginSheetModal(for: window) { [weak self] response in
                self?.chrome.ejectPrompt = nil
                switch response {
                case .alertFirstButtonReturn: self?.unmount(siblings, primary: path, wholeDisk: true)
                case .alertSecondButtonReturn: self?.unmount([path], primary: path, wholeDisk: false)
                default: break
                }
            }
        }
    }

    private func unmount(_ volumes: [String], primary: String, wholeDisk: Bool) {
        // Every window's panes leave first, so nothing holds the volume.
        for controller in (NSApp.delegate as? AppDelegate)?.windowControllers ?? [] {
            volumes.forEach { controller.volumeGone($0) }
        }
        chrome.lastVolumeEvent = "eject \(wholeDisk ? "all" : "one") \(volumes.map(\.folderDisplayName).joined(separator: ","))"
        ExternalApps.record(chrome.lastVolumeEvent)
        guard !ExternalApps.isDryRun else { return }
        let options: FileManager.UnmountOptions = wholeDisk ? [.allPartitionsAndEjectDisk, .withoutUI] : [.withoutUI]
        FileManager.default.unmountVolume(at: URL(fileURLWithPath: primary), options: options) { [weak self] error in
            guard let error else { return }
            // Name whoever keeps the volume busy, as Finder does.
            let users = VolumeEject.processes(using: primary)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    VolumeEject.reportFailure(URL(fileURLWithPath: primary), error, users: users, window: self?.window)
                }
            }
        }
    }

    // MARK: Restored server tabs

    /// Restored tabs on a server reconnect when first shown, with the
    /// password from the keychain. A failure shows on the tab with Retry;
    /// the tab is never replaced.
    func reconnectVisibleTabs() {
        for side in [PaneSide.left, .right] {
            let pane = tabs(side)
            let record = pane.tabs[Int(pane.selected)]
            let shown = !(layout.singlePane && side != layout.active)
            guard !record.pending.isEmpty else {
                container(side).showPending(nil, failed: false) {}
                continue
            }
            let retry: () -> Void = { [weak self] in
                self?.chrome.reconnectErrors[record.id] = nil
                self?.reconnect(record)
            }
            if let error = chrome.reconnectErrors[record.id] {
                container(side).showPending("Could not reconnect to \(record.pending): \(error)", failed: true, retry: retry)
            } else {
                container(side).showPending("Reconnecting to \(record.pending)…", failed: false, retry: retry)
                if shown { reconnect(record) }
            }
        }
    }

    private func reconnect(_ record: TabRecord) {
        guard !chrome.reconnecting.contains(record.id) else { return }
        let core = AppEnvironment.shared.core
        if let open = core.connectionForUrl(url: record.pending) {
            session.navigateTo(tab: record.id, conn: open, path: record.path)
            return
        }
        guard let spec = ServerSpecs.spec(url: record.pending), let params = ServerSpecs.params(spec, path: record.path) else {
            chrome.reconnectErrors[record.id] = "The server address is not valid."
            return reconnectVisibleTabs()
        }
        chrome.reconnecting.insert(record.id)
        ConnectionCenter.shared.connect(params, window: window) { [weak self] result in
            guard let self else { return }
            self.chrome.reconnecting.remove(record.id)
            switch result {
            case .success(let connection):
                core.setConnectionUrl(connection: connection.id, url: record.pending)
                self.session.navigateTo(tab: record.id, conn: connection.id, path: record.path)
            case .failure(let failure):
                self.chrome.reconnectErrors[record.id] = failure.message
                self.reconnectVisibleTabs()
            }
        }
    }

    // MARK: Privacy

    /// A folder could not be read: maybe a privacy refusal (panes-tabs §9).
    func noteLoadFailure(path: String, error: RustyFinderError) {
        FullDiskAccess.loadFailed(path: path, error: error, in: window)
    }

    // MARK: Scripted checks

    /// Tokens for this part; true if `token` was one.
    func chromePress(_ token: String) -> Bool {
        func value(_ prefix: String) -> String? { token.hasPrefix(prefix) ? String(token.dropFirst(prefix.count)) : nil }
        let selection = activeList?.selectedPaths ?? []
        if let title = value("sidebarclick:") {
            let option = title.hasSuffix(":alt")
            if let node = sidebar.node(titled: option ? String(title.dropLast(4)) : title) { sidebar.open(node, option: option) }
        } else if let title = value("sidebardrop:") {
            let parts = title.split(separator: ":").map(String.init)
            let flags: NSEvent.ModifierFlags = parts.dropFirst().contains("cmd") ? .command : (parts.dropFirst().contains("alt") ? .option : [])
            if let node = sidebar.node(titled: parts[0]) { sidebar.drop(paths: selection, on: node, index: -1, modifiers: flags) }
        } else if let title = value("sidebarhide:") {
            if let node = sidebar.node(titled: title), let key = node.hideKey { SidebarVisibility.set(key, hidden: true) }
            else if sidebar.node(titled: title) != nil { TagStore.setSidebar(title, shown: false) }
        } else if let spec = value("sidebarmove:") {
            // sidebarmove:<item>><index> within its group
            let parts = spec.split(separator: ">").map(String.init)
            guard parts.count == 2, let node = sidebar.node(titled: parts[0]), let index = Int(parts[1]) else { return true }
            let key: String
            switch node.kind {
            case .device: key = "device:\(node.path ?? "")"
            case .favorite(let f): key = "favorite:\(f.id)"
            default: return true
            }
            let group = (sidebar.node(titled: "Devices").map { [$0] } ?? []) + AppEnvironment.shared.core.favoriteGroups().compactMap { sidebar.node(titled: $0.name) }
            if let parent = group.first(where: { $0.children.contains(node) }) { sidebar.reorder(key: key, into: parent, index: index) }
        } else if token == "sidebareditor" {
            sidebar.menuEditor(nil)
        } else if let title = value("sidebareditor:") {
            SidebarEditorController.current?.debugToggle(title)
        } else if let spec = value("tabmove:") {
            // tabmove:<index in active pane>><left|right>:<index>
            let parts = spec.split(separator: ">").map(String.init)
            let target = parts.count == 2 ? parts[1].split(separator: ":").map(String.init) : []
            let pane = tabs(layout.active)
            guard parts.count == 2, target.count == 2, let from = Int(parts[0]), let tab = pane.tabs[safe: from], let to = Int(target[1])
            else { return true }
            self.pane(target[0] == "left" ? .left : .right, move: tab.id, to: to)
        } else if let spec = value("tabdrop:") {
            let pane = tabs(layout.active)
            if let index = Int(spec), let tab = pane.tabs[safe: index] {
                _ = self.pane(layout.active, drop: selection, onTab: tab.id, path: nil, modifiers: [])
            }
        } else if let spec = value("tabhover:") {
            let bar = container(layout.active).debugTabBar
            if let index = Int(spec), let button = bar.subviews.flatMap(\.subviews).compactMap({ $0 as? TabButton })[safe: index] {
                button.springLoad()
            }
        } else if let spec = value("plusdrop:") {
            pane(layout.active, openTabsFor: spec.split(separator: "|").map(String.init))
        } else if let spec = value("pathdrop:") {
            if let index = Int(spec) { _ = container(layout.active).debugDropOnSegment(index, paths: selection) }
        } else if let path = value("unmounted:") {
            volumeGone(path)
        } else if let spec = value("eject:") {
            // eject:<path>[:alt|:ctrl][:2] (":2" pretends the disk has two volumes)
            var parts = spec.split(separator: ":").map(String.init)
            var siblings: [String]?
            if parts.last == "2" {
                parts.removeLast()
                siblings = [parts[0], parts[0] + "-other"]
            }
            let flags: NSEvent.ModifierFlags = parts.contains("alt") ? .option : (parts.contains("ctrl") ? .control : [])
            eject(parts[0], modifiers: flags, siblings: siblings ?? [parts[0]])
        } else if let spec = value("favsheet:") {
            AddFavoriteSheet.current?.debugSet(spec)
        } else if let spec = value("fm:") {
            if spec == "open" { FavoriteManagerWindowController.show() } else { FavoriteManagerWindowController.current?.debugSet(spec) }
        } else if let name = value("openfav:") {
            let all = AppEnvironment.shared.core.favoriteGroups().flatMap(\.items)
            if let favorite = all.first(where: { $0.name == name }) { openFavorite(favorite) }
        } else if let spec = value("welcome:") {
            if spec == "show" { WelcomeWindowController.show() } else { WelcomeWindowController.current?.debugSet(spec) }
        } else if let path = value("service:") {
            let board = NSPasteboard(name: .init("dev.rustyfinder.scripted-service"))
            board.clearContents()
            board.setString(path, forType: .string)
            var error: NSString?
            ServiceProvider().revealInRustyFinder(board, userData: nil, error: &error)
            if let error { chrome.lastDrop = "service error: \(error)" }
        } else if let url = value("openurl:") {
            if let parsed = url.hasPrefix("/") ? URL(fileURLWithPath: url) : URL(string: url) {
                (NSApp.delegate as? AppDelegate)?.application(NSApp, open: [parsed])
            }
        } else if let host = value("sharepicker:") {
            ShareBrowser.shared.pickShare(host: host, scheme: "smb", in: self)
        } else if token == "fdaprompt" {
            FullDiskAccess.prompt(in: window)
        } else if let path = value("loadfail:") {
            noteLoadFailure(path: path, error: .PermissionDenied(message: "Operation not permitted"))
        } else if token == "savesearch" {
            saveSearchAsFavorite(nil)
        } else if let url = value("favshare:") {
            // A share favorite, as the Connect panel's Add to Favorites makes one.
            if let spec = ServerSpecs.spec(url: url), let parsed = parseConnectionUrl(text: url) {
                let draft = FavoriteDraftRecord(name: "Share", path: parsed.path, icon: "", kind: .share, server: spec, payload: "")
                _ = try? AppEnvironment.shared.core.addFavoriteDraft(group: "Shares", draft: draft)
                AppEnvironment.shared.favoritesChanged()
            }
        } else if token == "openwindow" {
            openSelectionInNewWindow(nil)
        } else if token == "syncfav" {
            SyncWindowController.current?.addToFavorites(nil)
        } else if token == "mrfav" {
            MultiRenameWindowController.current?.addToFavorites(nil)
        } else if let name = value("fmreveal:") {
            let all = AppEnvironment.shared.core.favoriteGroups()
            let id = all.first { $0.name == name }?.id ?? all.flatMap(\.items).first { $0.name == name }?.id
            FavoriteManagerWindowController.show(selecting: id)
        } else if let title = value("sidebarhideothers:") {
            if let node = sidebar.node(titled: title) { sidebar.hideOthers(than: node) }
        } else {
            return false
        }
        return true
    }

    /// The text of the sheet on this window, if any.
    private var sheetText: String {
        guard let sheet = window?.attachedSheet, let content = sheet.contentView else { return "-" }
        func texts(_ view: NSView) -> [String] {
            let own = (view as? NSTextField).map { [$0.stringValue] } ?? []
            return own + view.subviews.flatMap(texts)
        }
        return texts(content).filter { !$0.isEmpty }.joined(separator: " | ")
    }

    func chromeDump() -> [String] {
        let core = AppEnvironment.shared.core
        let groups = core.favoriteGroups().map { group in
            "\(group.name)[\(group.items.map { "\($0.name):\($0.kind)\($0.icon.isEmpty ? "" : "@\($0.icon)")" }.joined(separator: ","))]"
        }
        let pending = [PaneSide.left, .right].map { side in
            "\(side == .left ? "left" : "right")=\(tabs(side).tabs.map { $0.pending.isEmpty ? "-" : $0.pending }.joined(separator: ","))"
        }
        return [
            "chrome: title=\(chrome.titleHidden ? "hidden" : "shown") pathbar=\(left.isPathBarVisible ? "shown" : "hidden") fullscreen=\(window?.styleMask.contains(.fullScreen) == true)",
            "tabicons: left=\(left.tabIcons) right=\(right.tabIcons)",
            "favorites: \(groups.joined(separator: " "))",
            "favsheet: \(AddFavoriteSheet.current?.debugDump() ?? "-")",
            "favmanager: \(FavoriteManagerWindowController.current?.debugDump() ?? "-")",
            "sidebar.drop: \(sidebar.lastDrop) editor=\(SidebarEditorController.current?.debugDump() ?? "-")",
            "drop: \(chrome.lastDrop)",
            "volume: \(chrome.lastVolumeEvent) prompt=\(chrome.ejectPrompt?.messageText ?? "-")",
            "pending: \(pending.joined(separator: " ")) left.view=\(left.pendingText) right.view=\(right.pendingText)",
            "sheet: \(sheetText)",
            "outside: \((NSApp.delegate as? AppDelegate)?.lastOutsideRequest ?? "-")",
            "welcome: \(WelcomeWindowController.current?.debugDump() ?? "-") shown=\(AppEnvironment.shared.settings.welcomeShown)",
            "fda: prompt=\(FullDiskAccess.lastPrompt) dismissed=\(AppEnvironment.shared.settings.fdaDismissed)",
            "theme: \(ThemeCenter.shared.debugSummary) row=\(debugRowColor)",
            "sharepicker: \(ShareBrowser.shared.lastPicker)",
            "tags.list: \(TagStore.list.map(\.name).joined(separator: ","))",
            "recents.source: \(sidebar.recentsConnection)",
            "volumesmenu: \(paneVolumesMenu(layout.active).items.map(\.title).filter { !$0.isEmpty }.joined(separator: ","))",
            "otherrow: \(debugOtherRowColor)",
        ]
    }

    /// The selection colour the other pane's rows would use.
    private var debugOtherRowColor: String {
        guard let table = otherList?.tableView, table.numberOfRows > 0 else { return "-" }
        guard let row = table.rowView(atRow: 0, makeIfNecessary: true) as? ThemedRowView else { return "plain" }
        return row.debugSelectionColor
    }

    /// The selection colour the active list's first row would use.
    private var debugRowColor: String {
        guard let table = activeList?.tableView, table.numberOfRows > 0 else { return "-" }
        guard let row = table.rowView(atRow: 0, makeIfNecessary: false) as? ThemedRowView else { return "plain" }
        return row.debugSelectionColor
    }
}
