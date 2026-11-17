import AppKit

/// Checks before items on this Mac are renamed, moved or deleted
/// (edge-cases §4):
/// - an item iCloud is still syncing can't be changed now: say so;
/// - a locked item would make the operation fail: offer to unlock it.
@MainActor
enum LocalPreflight {
    /// The question on screen (scripted checks).
    private(set) static var lastAlert: NSAlert?
    private(set) static var lastMessage = "-"

    /// Run `proceed` once `paths` may be changed. `verb` completes "…can’t
    /// be <verb> right now" ("moved to the Trash", "renamed"). Remote items
    /// (`conn` other than 0) need none of this.
    static func run(_ paths: [String], conn: UInt64, verb: String, window: NSWindow?, proceed: @escaping () -> Void) {
        guard conn == 0, !paths.isEmpty else { return proceed() }
        if let busy = paths.first(where: CloudItems.isBusy) {
            let alert = NSAlert()
            alert.alertStyle = .warning
            alert.messageText = "“\(display(busy))” can’t be \(verb) right now."
            alert.informativeText = "iCloud is still syncing it. Try again when it has finished uploading or downloading."
            alert.addButton(withTitle: "OK")
            show(alert, in: window) { _ in }
            return
        }
        let locked = paths.filter(isLocked)
        guard !locked.isEmpty else { return proceed() }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = locked.count == 1
            ? "“\(display(locked[0]))” is locked."
            : "\(locked.count) items are locked."
        alert.informativeText = "Locked items can’t be \(verb). Unlock \(locked.count == 1 ? "it" : "them") and continue?"
        alert.addButton(withTitle: "Unlock and Continue")
        let cancel = alert.addButton(withTitle: "Cancel")
        cancel.keyEquivalent = "\u{1b}"
        show(alert, in: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            if let failed = locked.first(where: { !unlock($0) }) {
                let problem = NSAlert()
                problem.messageText = "“\(display(failed))” couldn’t be unlocked."
                problem.informativeText = "You may not have permission to change it."
                show(problem, in: window) { _ in }
                return
            }
            proceed()
        }
    }

    /// The user-immutable flag ("Locked" in Get Info).
    static func isLocked(_ path: String) -> Bool {
        (try? URL(fileURLWithPath: path).resourceValues(forKeys: [.isUserImmutableKey]))?.isUserImmutable == true
    }

    private static func unlock(_ path: String) -> Bool {
        var url = URL(fileURLWithPath: path)
        var values = URLResourceValues()
        values.isUserImmutable = false
        return (try? url.setResourceValues(values)) != nil && !isLocked(path)
    }

    private static func display(_ path: String) -> String {
        FileManager.default.displayName(atPath: path)
    }

    private static func show(_ alert: NSAlert, in window: NSWindow?, done: @escaping (NSApplication.ModalResponse) -> Void) {
        lastAlert = alert
        lastMessage = alert.messageText
        guard let window else {
            done(alert.runModal())
            lastAlert = nil
            return
        }
        alert.beginSheetModal(for: window) { response in
            lastAlert = nil
            done(response)
        }
    }
}
