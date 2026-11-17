import AppKit

/// The app's few sounds, all behind the "Play sounds" setting.
@MainActor
enum Sounds {
    /// A finished transfer, delete or trash.
    static let completion = "Pop"
    /// Undo and Redo of a file operation (file-ops §8).
    static let undo = "Tink"

    /// The last sound asked for (scripted checks, which never play one).
    private(set) static var last = "-"

    static func play(_ name: String) {
        guard AppEnvironment.shared.settings.playSounds else { return }
        last = name
        if ExternalApps.isDryRun { return }
        NSSound(named: name)?.play()
    }
}
