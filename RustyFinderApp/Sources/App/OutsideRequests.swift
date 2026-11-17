import AppKit

/// The "Reveal in RustyFinder" service (Services menu of other apps).
final class ServiceProvider: NSObject {
    /// NSMessage `revealInRustyFinder` in Info.plist's NSServices.
    @objc func revealInRustyFinder(_ pboard: NSPasteboard, userData: String?, error: AutoreleasingUnsafeMutablePointer<NSString?>) {
        let urls = (pboard.readObjects(forClasses: [NSURL.self], options: nil) as? [URL]) ?? []
        var items = urls.map(\.absoluteString)
        if urls.isEmpty, let text = pboard.string(forType: .string) { items.append(text) }
        MainActor.assumeIsolated {
            let requests = serviceRequests(items: items, home: NSHomeDirectory())
            if requests.isEmpty {
                error.pointee = "Nothing to reveal: the service needs files, folders or paths." as NSString
                return
            }
            (NSApp.delegate as? AppDelegate)?.handle(requests)
        }
    }
}

extension AppDelegate {
    /// Links (`rustyfinder://`), and files or folders dropped on the Dock
    /// icon or opened with RustyFinder.
    @objc(application:openURLs:)
    func application(_ application: NSApplication, open urls: [URL]) {
        var requests: [OutsideRequestRecord] = []
        for url in urls {
            if url.isFileURL {
                requests += serviceRequests(items: [url.absoluteString], home: NSHomeDirectory())
            } else if let request = parseAppUrl(url: url.absoluteString) {
                requests.append(request)
            }
        }
        handle(requests)
    }

    /// Carry out requests from outside in the front window (a new one if
    /// there is none). A location already open in a tab of that window is
    /// shown there instead of in a new tab (PT-16).
    func handle(_ requests: [OutsideRequestRecord]) {
        guard !requests.isEmpty else { return }
        lastOutsideRequest = requests.map { "\($0)" }.joined(separator: ";")
        let front = (NSApp.mainWindow?.windowController as? MainWindowController)
            ?? windowControllers.first { $0.window?.isVisible == true }
            ?? openWindow(restoring: nil)
        for request in requests {
            switch request {
            case .open(let path): front.show(folder: path, select: nil)
            case .reveal(let path):
                front.show(folder: (path as NSString).deletingLastPathComponent, select: (path as NSString).lastPathComponent)
            case .favorite(let id):
                if let favorite = AppEnvironment.shared.core.favorite(id: id) { front.openFavorite(favorite) }
            case .workspace(let id):
                if let workspace = AppEnvironment.shared.core.workspaces().first(where: { $0.id == id }) {
                    openWorkspace(workspace, replacing: front)
                }
            }
        }
        front.window?.makeKeyAndOrderFront(nil)
        if !ExternalApps.isDryRun { NSApp.activate(ignoringOtherApps: true) }
    }
}

extension MainWindowController {
    /// Show a local folder: in the tab that already shows it, else in a
    /// new tab of the active pane. With `select`, that item is selected.
    func show(folder: String, select name: String?) {
        let tab: UInt64
        if let open = session.tabShowing(conn: 0, path: folder) {
            tab = open
            session.selectTab(tab: open)
        } else {
            tab = session.newTab(side: layout.active, path: folder)
        }
        if let name, let list = fileList(for: tab) {
            list.reveal(names: [name], renameAfter: false)
            // The folder may still be loading: select once it has.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak list] in
                list?.reveal(names: [name], renameAfter: false)
            }
        }
    }
}
