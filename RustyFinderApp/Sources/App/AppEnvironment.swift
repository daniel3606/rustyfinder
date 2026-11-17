import AppKit

extension Notification.Name {
    /// Posted after any setting changes.
    static let rustyfinderSettingsChanged = Notification.Name("RustyFinderSettingsChanged")
    /// Posted after favorites change.
    static let rustyfinderFavoritesChanged = Notification.Name("RustyFinderFavoritesChanged")
}

/// The one Rust app object plus a cached copy of the settings.
@MainActor
final class AppEnvironment {
    static let shared = AppEnvironment()

    let core: RustyFinderApp
    private(set) var settings: AppSettings

    private init() {
        // Scripted checks point this at a temp folder so they never touch
        // the user's real settings or saved session.
        let override = ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"]
        let dir = override ?? Self.defaultSupportDir()
        core = RustyFinderApp(supportDir: dir, home: NSHomeDirectory())
        settings = core.settings()
    }

    /// The app used to be called Hoist. Carry its settings and saved
    /// session over the first time, so the rename loses nothing.
    private static func defaultSupportDir() -> String {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = base.appendingPathComponent("RustyFinder")
        let old = base.appendingPathComponent("Hoist")
        let fm = FileManager.default
        if !fm.fileExists(atPath: dir.path), fm.fileExists(atPath: old.path) {
            try? fm.moveItem(at: old, to: dir)
        }
        return dir.path
    }

    func update(_ change: (inout AppSettings) -> Void) {
        change(&settings)
        core.setSettings(settings: settings)
        NotificationCenter.default.post(name: .rustyfinderSettingsChanged, object: nil)
    }

    func favoritesChanged() {
        NotificationCenter.default.post(name: .rustyfinderFavoritesChanged, object: nil)
    }
}
