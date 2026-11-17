import AppKit

/// "Commit…": a message, and a Commit button that stays off while the
/// message is empty.
@MainActor
final class CommitSheet: NSObject, NSTextViewDelegate {
    private(set) static var current: CommitSheet?
    private let alert = NSAlert()
    private let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: 380, height: 110))
    private var commitButton: NSButton!

    override init() {
        super.init()
        alert.messageText = "Commit"
        alert.informativeText = "Commits what is staged in this repository."
        commitButton = alert.addButton(withTitle: "Commit")
        alert.addButton(withTitle: "Cancel")
        textView.delegate = self
        textView.isRichText = false
        textView.font = .systemFont(ofSize: NSFont.systemFontSize)
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.setAccessibilityLabel("Commit message")
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 380, height: 110))
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        alert.accessoryView = scroll
        alert.window.initialFirstResponder = textView
        commitButton.isEnabled = false
    }

    private var message: String {
        textView.string.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func textDidChange(_ notification: Notification) {
        commitButton.isEnabled = !message.isEmpty
    }

    func begin(on window: NSWindow, completion: @escaping (String?) -> Void) {
        Self.current = self
        alert.beginSheetModal(for: window) { [self] response in
            Self.current = nil
            completion(response == .alertFirstButtonReturn && !message.isEmpty ? message : nil)
        }
    }

    // MARK: Scripted checks

    func debugType(_ text: String) {
        textView.string = text
        textDidChange(Notification(name: NSText.didChangeNotification, object: textView))
    }

    func debugDump() -> String { "commit=\(commitButton.isEnabled ? "enabled" : "disabled")" }
}
