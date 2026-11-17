import AppKit

extension Notification.Name {
    /// Posted after the user's tag list changes (Settings → Tags).
    static let rustyfinderTagsChanged = Notification.Name("RustyFinderTagsChanged")
}

/// Per-window state for the navigation chrome that lives beside the
/// window controller: tab icons, tag listings, saved-search queries,
/// reconnecting tabs.
@MainActor
final class WindowChrome {
    /// A tab opened from a favorite with its own icon keeps that icon
    /// while it stays inside the favorite's location.
    struct TabIcon {
        let conn: UInt64
        let root: String
        let image: NSImage
        /// The favorite's icon value (scripted checks).
        let key: String
    }

    var tabIcons: [UInt64: TabIcon] = [:]
    /// Tabs listing a tag (a Spotlight search for it), by tag name.
    var tagTabs: [UInt64: String] = [:]
    /// The last search of each tab, so it can be saved as a favorite.
    var searches: [UInt64: (query: String, spotlight: Bool, path: String)] = [:]
    /// Restored remote tabs being reconnected, and the last failure of each.
    var reconnecting: Set<UInt64> = []
    var reconnectErrors: [UInt64: String] = [:]
    var pathBarHidden = false
    var titleHidden = false
    /// Last drop on a tab or path-bar segment, and last volume event
    /// (scripted checks).
    var lastDrop = "-"
    var lastVolumeEvent = "-"
    var ejectPrompt: NSAlert?
}

/// Icons for favorites, in the sidebar, tabs and Quick Open.
@MainActor
enum FavoriteIcons {
    /// Built-in choices for the Add to Favorites sheet: (title, SF Symbol).
    static let choices: [(String, String)] = [
        ("Home", "house"), ("Folder", "folder"), ("Server", "server.rack"), ("Cloud", "cloud"),
        ("Phone", "iphone"), ("Film", "film"), ("Developer", "hammer"), ("Music", "music.note"),
        ("Photos", "photo"), ("Documents", "doc"), ("Downloads", "arrow.down.circle"), ("Desktop", "menubar.dock.rectangle"),
        ("Star", "star"), ("Briefcase", "briefcase"), ("Globe", "globe"), ("Terminal", "terminal"),
        ("Network", "network"), ("Archive", "archivebox"),
    ]

    /// The symbol a kind of favorite shows when it has no icon of its own.
    static func kindSymbol(_ kind: FavoriteKindRecord) -> String? {
        switch kind {
        case .path: return nil
        case .connection: return "network"
        case .share: return "externaldrive.connected.to.line.below"
        case .synclet: return "arrow.triangle.2.circlepath"
        case .renamePreset: return "character.cursor.ibeam"
        case .savedSearch: return "magnifyingglass"
        case .workspace: return "rectangle.split.2x1"
        }
    }

    /// An icon value as an image: an SF Symbol name, or an image file.
    static func image(icon: String) -> NSImage? {
        guard !icon.isEmpty else { return nil }
        if icon.hasPrefix("/") {
            guard let image = NSImage(contentsOfFile: icon) else { return nil }
            image.size = NSSize(width: 16, height: 16)
            return image
        }
        return NSImage(systemSymbolName: icon, accessibilityDescription: nil)
    }

    static func image(for favorite: FavoriteRecord) -> NSImage {
        if let own = image(icon: favorite.icon) { return own }
        if let symbol = kindSymbol(favorite.kind), let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil) {
            return image
        }
        var isDir: ObjCBool = false
        if FileManager.default.fileExists(atPath: favorite.path, isDirectory: &isDir), !isDir.boolValue {
            let icon = NSWorkspace.shared.icon(forFile: favorite.path)
            icon.size = NSSize(width: 16, height: 16)
            return icon
        }
        return RowPresenter.shared.folderIcon(atPath: favorite.path)
    }

    /// True if the favorite has an icon of its own (shown on its tabs).
    static func hasOwnIcon(_ favorite: FavoriteRecord) -> Bool { !favorite.icon.isEmpty }
}

/// Servers as favorites remember them.
@MainActor
enum ServerSpecs {
    static let schemes: [(RemoteProtocol, String)] = [
        (.sftp, "sftp"), (.ftp, "ftp"), (.ftps, "ftps"), (.webDav, "webdav"), (.webDavs, "webdavs"),
        (.s3, "s3"), (.smb, "smb"), (.afp, "afp"), (.nfs, "nfs"), (.vnc, "vnc"),
    ]

    static func scheme(_ proto: RemoteProtocol) -> String {
        schemes.first { $0.0 == proto }?.1 ?? "sftp"
    }

    static func proto(_ scheme: String) -> RemoteProtocol? {
        schemes.first { $0.1 == scheme.lowercased() }?.0
    }

    static func isShare(_ proto: RemoteProtocol) -> Bool {
        [.smb, .afp, .nfs].contains(proto)
    }

    /// The server a connection URL names ("sftp://bob@host:2222"), with
    /// the keychain account the app stores its password under.
    static func spec(url: String) -> ServerSpecRecord? {
        guard let parsed = parseConnectionUrl(text: url) else { return nil }
        let scheme = Self.scheme(parsed.protocol)
        return ServerSpecRecord(
            protocol: scheme, host: parsed.host, port: parsed.port, user: parsed.user,
            keychain: Keychain.account(host: parsed.host, user: parsed.user, port: parsed.port, scheme: scheme),
            options: [:])
    }

    /// What it takes to connect to `spec`, opening at `path`. The password
    /// comes from the keychain.
    static func params(_ spec: ServerSpecRecord, path: String) -> ServerParams? {
        guard let proto = proto(spec.protocol) else { return nil }
        let password = spec.keychain.isEmpty ? nil : Keychain.password(for: spec.keychain)
        return ServerParams(
            protocol: proto, host: spec.host, port: spec.port, user: spec.user, password: password,
            remotePath: path.isEmpty ? nil : path, passive: spec.options["passive"] != "false",
            encoding: spec.options["encoding"], acceptInvalidCerts: false, region: spec.options["region"])
    }
}

extension FavoriteDraftRecord {
    /// A draft with everything a favorite holds, for editing it.
    init(_ favorite: FavoriteRecord) {
        self.init(
            name: favorite.name, path: favorite.path, icon: favorite.icon, kind: favorite.kind,
            server: favorite.server, payload: favorite.payload)
    }

    static func path(_ name: String, _ path: String, icon: String = "") -> FavoriteDraftRecord {
        FavoriteDraftRecord(name: name, path: path, icon: icon, kind: .path, server: nil, payload: "")
    }
}
