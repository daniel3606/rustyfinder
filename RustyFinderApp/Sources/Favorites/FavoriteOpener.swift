import AppKit

/// Opening a favorite of any kind, and saving things as favorites, for
/// one browser window.
extension MainWindowController {
    // MARK: Opening

    /// Open a favorite the way clicking it in the sidebar does. Option
    /// (`otherPane`) opens a folder in the other pane and reveals a file;
    /// `newWindow` opens a workspace beside this window.
    func openFavorite(_ favorite: FavoriteRecord, otherPane: Bool = false, newTab: Bool = false, newWindow: Bool = false) {
        switch favorite.kind {
        case .path:
            openPathFavorite(favorite, otherPane: otherPane, newTab: newTab)
        case .connection, .share:
            guard let server = favorite.server else { return }
            openServer(server, path: favorite.path, favorite: favorite)
        case .synclet:
            openSynclet(favorite)
        case .renamePreset:
            openRenamePreset(favorite)
        case .savedSearch:
            openSavedSearch(favorite)
        case .workspace:
            guard let id = UInt64(favorite.payload),
                  let workspace = AppEnvironment.shared.core.workspaces().first(where: { $0.id == id })
            else {
                return showProblem(title: "The workspace “\(favorite.name)” no longer exists.", message: "Remove the favorite and save the workspace again.")
            }
            (NSApp.delegate as? AppDelegate)?.openWorkspace(workspace, replacing: newWindow ? nil : self)
        }
    }

    /// A path saved under a server favorite: connect, then open it.
    func openFavoritePath(_ child: FavoritePathRecord, of favorite: FavoriteRecord) {
        guard let server = favorite.server else { return }
        openServer(server, path: child.path, favorite: favorite)
    }

    private func openPathFavorite(_ favorite: FavoriteRecord, otherPane: Bool, newTab: Bool) {
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: favorite.path, isDirectory: &isDir) else {
            return showProblem(title: "“\(favorite.name)” can’t be found.", message: "\(favorite.path.abbreviatingHome) is no longer there.")
        }
        // A bundle opens with its app unless packages are browsed as folders.
        let browsable = isDir.boolValue
            && (AppEnvironment.shared.settings.treatPackagesAsFolders || !NSWorkspace.shared.isFilePackage(atPath: favorite.path))
        if !browsable {
            if otherPane {
                // Option-click on a file favorite shows it in its folder.
                let side = layout.singlePane ? layout.active : other(layout.active)
                session.navigate(tab: currentTab(side), path: (favorite.path as NSString).deletingLastPathComponent)
                fileList(for: currentTab(side))?.reveal(names: [(favorite.path as NSString).lastPathComponent], renameAfter: false)
            } else {
                ExternalApps.record("open \(favorite.path)")
                if !ExternalApps.isDryRun { NSWorkspace.shared.open(URL(fileURLWithPath: favorite.path)) }
            }
            return
        }
        let side = otherPane && !layout.singlePane ? other(layout.active) : layout.active
        let tab: UInt64
        if newTab {
            tab = session.newTab(side: side, path: favorite.path)
        } else {
            tab = currentTab(side)
            session.navigate(tab: tab, path: favorite.path)
        }
        noteTabIcon(tab, favorite: favorite, conn: 0, root: favorite.path)
    }

    /// The pane servers open in ("Open remote connections in").
    var remoteSide: PaneSide {
        switch AppEnvironment.shared.settings.openRemoteIn {
        case .active: return layout.active
        case .left: return .left
        case .right: return .right
        }
    }

    /// Connect to a server (reusing an open connection to it) and show
    /// `path` there in the pane servers open in.
    func openServer(_ spec: ServerSpecRecord, path: String, favorite: FavoriteRecord? = nil) {
        guard let proto = ServerSpecs.proto(spec.protocol), let params = ServerSpecs.params(spec, path: path) else { return }
        let side = remoteSide
        if ServerSpecs.isShare(proto) {
            openShare(params, scheme: spec.protocol, side: side) { [weak self] message in
                if let message { self?.showProblem(title: "The share could not be opened.", message: message) }
            }
            return
        }
        let url = serverSpecUrl(spec: spec)
        let show: (UInt64, String) -> Void = { [weak self] conn, home in
            guard let self else { return }
            self.session.setActive(side: side)
            let tab = self.currentTab(side)
            let target = path.isEmpty ? home : path
            self.session.navigateTo(tab: tab, conn: conn, path: target)
            if let favorite { self.noteTabIcon(tab, favorite: favorite, conn: conn, root: "/") }
        }
        let core = AppEnvironment.shared.core
        if let open = core.connectionForUrl(url: url), let record = core.connections().first(where: { $0.id == open }) {
            return show(open, record.home)
        }
        ConnectionCenter.shared.connect(params, window: window) { [weak self] result in
            switch result {
            case .success(let connection):
                core.setConnectionUrl(connection: connection.id, url: url)
                show(connection.id, connection.home)
            case .failure(let failure):
                self?.showProblem(title: "Could not connect to \(spec.host).", message: failure.message)
            }
        }
    }

    private func noteTabIcon(_ tab: UInt64, favorite: FavoriteRecord, conn: UInt64, root: String) {
        guard FavoriteIcons.hasOwnIcon(favorite) else {
            chrome.tabIcons[tab] = nil
            return
        }
        chrome.tabIcons[tab] = .init(conn: conn, root: root, image: FavoriteIcons.image(for: favorite), key: favorite.icon)
        apply(layout)
    }

    private func openSynclet(_ favorite: FavoriteRecord) {
        guard let data = favorite.payload.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let left = object["left"] as? String, let right = object["right"] as? String
        else { return }
        let fm = FileManager.default
        guard fm.fileExists(atPath: left), fm.fileExists(atPath: right) else {
            return showProblem(title: "The synclet’s folders can’t be found.", message: "\(left.abbreviatingHome)\n\(right.abbreviatingHome)")
        }
        SyncWindowController.show(
            left: .init(conn: 0, root: left, label: left), right: .init(conn: 0, root: right, label: right),
            leftToRight: (object["direction"] as? String) != "RightToLeft", windowId: session.id())
        if let options = object["options"] as? String { SyncWindowController.current?.applySynclet(optionsJSON: options) }
    }

    private func openRenamePreset(_ favorite: FavoriteRecord) {
        guard let list = activeList, !list.inspectedSelection.isEmpty else {
            return showProblem(title: "Select the items to rename first.", message: "The preset “\(favorite.name)” renames the selected items.")
        }
        MultiRenameWindowController.show(items: list.inspectedSelection, conn: list.conn, windowId: session.id())
        MultiRenameWindowController.current?.debugSetRules(favorite.payload)
    }

    /// Items dropped on a rename preset: shown selected in the active
    /// pane, then renamed with the preset (MR-08).
    func renameDropped(_ paths: [String], with preset: FavoriteRecord) {
        guard let first = paths.first else { return }
        let folder = (first as NSString).deletingLastPathComponent
        let names = paths.filter { ($0 as NSString).deletingLastPathComponent == folder }.map { ($0 as NSString).lastPathComponent }
        let tab = activeTab
        let run = { [weak self] in
            guard let self, let list = self.fileList(for: tab) else { return }
            list.reveal(names: names, renameAfter: false)
            self.openFavorite(preset)
        }
        if activeList?.isRemote == false, activeList?.content.path == folder {
            run()
        } else {
            session.navigateTo(tab: tab, conn: 0, path: folder)
            // Once the folder has loaded, so the items can be selected.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6, execute: run)
        }
    }

    private func openSavedSearch(_ favorite: FavoriteRecord) {
        guard let data = favorite.payload.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let query = object["query"] as? String
        else { return }
        let path = (object["path"] as? String) ?? NSHomeDirectory()
        activeList?.setFilter("")
        _ = session.searchAt(tab: activeTab, connection: 0, path: path, queryJson: query, useSpotlight: object["spotlight"] as? Bool ?? false)
    }

    // MARK: Saving

    /// Favorites > New Favorite… and Add to Favorites: the selected item
    /// (or the clicked path-bar segment, or the current folder) through
    /// the sheet that asks for name, group and icon.
    @objc func newFavorite(_ sender: Any?) {
        guard let path = activeList?.content.path, activeList?.isRemote == false else { return }
        askFavorite(.path(path.folderDisplayName, path))
    }

    /// Ask for name, group and icon, then add the favorite.
    func askFavorite(_ draft: FavoriteDraftRecord, group: String = "Favorites", onAdded: ((FavoriteRecord) -> Void)? = nil) {
        guard let window, window.attachedSheet == nil else { return }
        AddFavoriteSheet(draft: draft, group: group).begin(on: window) { [weak self] result in
            guard let result else { return }
            do {
                let added = try AppEnvironment.shared.core.addFavoriteDraft(group: result.group, draft: result.draft)
                AppEnvironment.shared.favoritesChanged()
                onAdded?(added)
            } catch {
                self?.showProblem(title: (error as? RustyFinderError)?.message ?? "The favorite could not be added.", message: "Choose another name.")
            }
        }
    }

    /// The search shown in the active tab, kept as a favorite.
    @objc func saveSearchAsFavorite(_ sender: Any?) {
        guard let list = activeList, let term = list.search?.term, let saved = chrome.searches[list.tabId] else { return }
        let object: [String: Any] = ["query": saved.query, "spotlight": saved.spotlight, "path": saved.path]
        guard let data = try? JSONSerialization.data(withJSONObject: object),
              let json = String(data: data, encoding: .utf8)
        else { return }
        askFavorite(
            FavoriteDraftRecord(name: term, path: saved.path, icon: "", kind: .savedSearch, server: nil, payload: json),
            group: "Searches")
    }

    /// "Add Path to Favorite" on a server: under the server's favorite if
    /// it has one, else a new favorite for the server at that path.
    func addRemotePathToFavorites(_ path: String) {
        guard let list = activeList, list.isRemote, !list.isArchive else { return }
        let core = AppEnvironment.shared.core
        let url = core.connectionUrl(connection: list.conn)
        guard !url.isEmpty, let spec = ServerSpecs.spec(url: url) else {
            return showProblem(title: "This server can’t be saved as a favorite.", message: "Connect to it from the Connect panel first.")
        }
        if let parent = core.favoriteForServer(url: url) {
            do {
                _ = try core.addFavoritePath(parent: parent.id, name: "", path: path)
                AppEnvironment.shared.favoritesChanged()
            } catch {
                showProblem(title: (error as? RustyFinderError)?.message ?? "The path could not be added.", message: "")
            }
            return
        }
        askFavorite(
            FavoriteDraftRecord(name: spec.host, path: path, icon: "", kind: .connection, server: spec, payload: ""),
            group: "Servers")
    }
}
