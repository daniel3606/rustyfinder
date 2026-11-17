import AppKit
import UniformTypeIdentifiers

/// What the item commands do for files on a server: Copy Path gives the
/// URL (never a password) or the server path, Open With downloads first,
/// and dragging an item out downloads it where it is dropped.
extension FileListViewController {
    // MARK: Copy Path

    /// The Copy Path submenu for items on a server.
    func remoteCopyPathItem(for paths: [String]) -> NSMenuItem {
        let submenu = NSMenu()
        for (tag, title) in ["URL", "Server Path", "Name"].enumerated() {
            let item = NSMenuItem(title: title, action: #selector(copyRemotePath(_:)), keyEquivalent: "")
            item.target = self
            item.tag = tag
            item.representedObject = paths
            submenu.addItem(item)
        }
        let item = NSMenuItem(title: "Copy Path", action: nil, keyEquivalent: "")
        item.submenu = submenu
        return item
    }

    /// Tag 0: URL, 1: path on the server, 2: name. One line per item.
    @objc func copyRemotePath(_ sender: NSMenuItem) {
        guard let paths = sender.representedObject as? [String] else { return }
        let core = AppEnvironment.shared.core
        let lines = paths.map { path -> String in
            switch sender.tag {
            case 0: return core.itemUrl(connection: conn, path: path) ?? path
            case 2: return (path as NSString).lastPathComponent
            default: return path
            }
        }
        let text = lines.joined(separator: "\n")
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        RemoteFiles.shared.lastCopied = text
    }

    // MARK: Open With

    /// Applications that open files of this kind, the default first. The
    /// file is downloaded when one is chosen.
    func remoteOpenWithItem(for remotePath: String) -> NSMenuItem {
        let apps = Self.applications(forName: (remotePath as NSString).lastPathComponent)
        let submenu = NSMenu()
        for (index, app) in apps.list.prefix(24).enumerated() {
            var title = FileManager.default.displayName(atPath: app.path)
            if index == 0, apps.preferred != nil { title += " (default)" }
            let item = NSMenuItem(title: title, action: #selector(openRemoteWith(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = [app.path, remotePath]
            let icon = NSWorkspace.shared.icon(forFile: app.path)
            icon.size = NSSize(width: 16, height: 16)
            item.image = icon
            submenu.addItem(item)
            if index == 0, apps.preferred != nil, apps.list.count > 1 { submenu.addItem(.separator()) }
        }
        if apps.list.isEmpty {
            submenu.addItem(NSMenuItem(title: "No Application Found", action: nil, keyEquivalent: ""))
        }
        let item = NSMenuItem(title: "Open With", action: nil, keyEquivalent: "")
        item.submenu = submenu
        return item
    }

    static func applications(forName name: String) -> (list: [URL], preferred: URL?) {
        let ext = (name as NSString).pathExtension
        let type = UTType(filenameExtension: ext) ?? .data
        let workspace = NSWorkspace.shared
        let preferred = workspace.urlForApplication(toOpen: type)
        var list = workspace.urlsForApplications(toOpen: type)
        if let preferred {
            list.removeAll { $0 == preferred }
            list.insert(preferred, at: 0)
        }
        return (list, preferred)
    }

    @objc func openRemoteWith(_ sender: NSMenuItem) {
        guard let pair = sender.representedObject as? [String], pair.count == 2 else { return }
        RemoteFiles.shared.open(
            connection: conn, remotePath: pair[1], window: session.id(), app: URL(fileURLWithPath: pair[0]))
    }

    // MARK: Dragging out

    /// A remote item dragged out: a promise kept by downloading it where
    /// it lands, plus its server path as text (for Terminal).
    func remoteDragWriter(for path: String, row: Row) -> NSPasteboardWriting {
        let source = RemoteDragSource(connection: conn, remotePath: path, window: session.id())
        let type = row.isDir ? UTType.folder : (UTType(filenameExtension: row.extension) ?? .data)
        let provider = RemoteFilePromiseProvider(fileType: type.identifier, delegate: source)
        provider.source = source
        return provider
    }

    // MARK: Scripted checks

    /// `copyurl`, `copypath`: Copy Path on the selection; `openwith:<app>`;
    /// `drop:<folder>`: drop the selected item there as another app would.
    func debugRemote(_ text: String) {
        let paths = selection.compactMap { content.path(at: $0) }
        guard let path = paths.first, let index = selection.first, let row = content.row(at: index) else { return }
        if text == "copyurl" || text == "copypath" {
            let item = NSMenuItem()
            item.tag = text == "copyurl" ? 0 : 1
            item.representedObject = paths
            copyRemotePath(item)
        } else if text.hasPrefix("openwith:") {
            let wanted = String(text.dropFirst(9))
            let apps = Self.applications(forName: row.name).list
            guard let app = apps.first(where: { FileManager.default.displayName(atPath: $0.path) == wanted }) else {
                RemoteFiles.shared.lastOpened = "no app \(wanted)"
                return
            }
            let item = NSMenuItem()
            item.representedObject = [app.path, path]
            openRemoteWith(item)
        } else if text.hasPrefix("drop:") {
            let folder = URL(fileURLWithPath: String(text.dropFirst(5)))
            let provider = remoteDragWriter(for: path, row: row) as? RemoteFilePromiseProvider
            provider?.source?.filePromiseProvider(
                provider!, writePromiseTo: folder.appendingPathComponent(row.name)
            ) { error in
                DispatchQueue.main.async { RemoteFiles.shared.lastDrop = error == nil ? "ok \(row.name)" : "failed" }
            }
        }
    }
}

/// Keeps the promise of a dragged remote item by downloading it.
final class RemoteDragSource: NSObject, NSFilePromiseProviderDelegate {
    let connection: UInt64
    let remotePath: String
    let window: UInt64
    private let queue = OperationQueue()

    init(connection: UInt64, remotePath: String, window: UInt64) {
        self.connection = connection
        self.remotePath = remotePath
        self.window = window
    }

    func filePromiseProvider(_ provider: NSFilePromiseProvider, fileNameForType fileType: String) -> String {
        (remotePath as NSString).lastPathComponent
    }

    func filePromiseProvider(
        _ provider: NSFilePromiseProvider, writePromiseTo url: URL, completionHandler: @escaping (Error?) -> Void
    ) {
        let (connection, remotePath, window) = (connection, remotePath, window)
        DispatchQueue.main.async {
            RemoteFiles.shared.download(
                connection: connection, remotePath: remotePath, into: url.deletingLastPathComponent().path,
                name: url.lastPathComponent, window: window
            ) { report in
                guard report?.result == .success else {
                    let message = report?.message ?? "The download did not finish."
                    return completionHandler(NSError(
                        domain: "RustyFinder", code: 1, userInfo: [NSLocalizedDescriptionKey: message]))
                }
                completionHandler(nil)
            }
        }
    }

    func operationQueue(for filePromiseProvider: NSFilePromiseProvider) -> OperationQueue {
        queue
    }
}

/// The promise, plus the server path as plain text.
final class RemoteFilePromiseProvider: NSFilePromiseProvider {
    /// The delegate is held weakly by AppKit; this keeps it alive.
    var source: RemoteDragSource?

    override func writableTypes(for pasteboard: NSPasteboard) -> [NSPasteboard.PasteboardType] {
        super.writableTypes(for: pasteboard) + [.string]
    }

    override func pasteboardPropertyList(forType type: NSPasteboard.PasteboardType) -> Any? {
        if type == .string { return source?.remotePath }
        return super.pasteboardPropertyList(forType: type)
    }
}
