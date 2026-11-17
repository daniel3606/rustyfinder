import AppKit

/// What a tool printed, in an ordinary window. Each run gets its own;
/// closing one releases it.
final class OutputWindowController: NSWindowController, NSWindowDelegate {
    private static var open: [OutputWindowController] = []
    static var openCount: Int { open.count }

    static func show(title: String, text: String) {
        let controller = OutputWindowController(title: title, text: text)
        open.append(controller)
        controller.showWindow(nil)
    }

    private init(title: String, text: String) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 380),
            styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.center()
        let scroll = NSTextView.scrollableTextView()
        if let view = scroll.documentView as? NSTextView {
            view.isEditable = false
            view.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
            view.string = text.isEmpty ? "(no output)" : text
            view.textContainerInset = NSSize(width: 8, height: 8)
            view.setAccessibilityLabel("Output of \(title)")
        }
        window.contentView = scroll
        super.init(window: window)
        window.delegate = self
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        Self.open.removeAll { $0 === self }
    }
}
