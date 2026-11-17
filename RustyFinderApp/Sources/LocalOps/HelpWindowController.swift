import AppKit

/// Help (⌘?): the README that ships inside the app, shown in a window.
/// Nothing is fetched from the network.
final class HelpWindowController: NSWindowController {
    private(set) static var current: HelpWindowController?
    private let textView = NSTextView()

    static func show() {
        if current == nil { current = HelpWindowController() }
        current?.showWindow(nil)
        current?.window?.makeKeyAndOrderFront(nil)
    }

    /// The README bundled with the app.
    static var readme: URL? { Bundle.main.url(forResource: "README", withExtension: "md") }

    private init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 620),
            styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "RustyFinder Help"
        window.isReleasedWhenClosed = false
        window.center()
        super.init(window: window)
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.documentView = textView
        textView.isEditable = false
        textView.isSelectable = true
        textView.textContainerInset = NSSize(width: 16, height: 16)
        textView.autoresizingMask = [.width]
        textView.isVerticallyResizable = true
        textView.textContainer?.widthTracksTextView = true
        textView.setAccessibilityLabel("Help")
        window.contentView = scroll
        textView.textStorage?.setAttributedString(Self.text())
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    /// The README with its inline Markdown (bold, code, links) rendered and
    /// its lines kept; headings are set in bold.
    static func text() -> NSAttributedString {
        guard let url = readme, let source = try? String(contentsOf: url, encoding: .utf8) else {
            return NSAttributedString(string: "The help file is missing from this copy of RustyFinder.")
        }
        let body = NSFont.systemFont(ofSize: NSFont.systemFontSize)
        let result = NSMutableAttributedString()
        for raw in source.components(separatedBy: "\n") {
            // The README opens with a little HTML for GitHub (icon,
            // centred title): keep the title as a heading, drop the rest.
            var line = raw
            if raw.hasPrefix("<") || raw.hasPrefix("  <") {
                let inner = raw.replacingOccurrences(of: "<[^>]+>", with: "", options: .regularExpression)
                    .trimmingCharacters(in: .whitespaces)
                if inner.isEmpty { continue }
                line = raw.contains("<h1") ? "# " + inner : inner
            }
            let level = line.prefix(while: { $0 == "#" }).count
            let text = level > 0 ? String(line.dropFirst(level)).trimmingCharacters(in: .whitespaces) : line
            let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
            let parsed = (try? NSAttributedString(AttributedString(markdown: text, options: options)))
                ?? NSAttributedString(string: text)
            let piece = NSMutableAttributedString(attributedString: parsed)
            let font = level > 0 ? NSFont.systemFont(ofSize: max(13, 22 - CGFloat(level) * 3), weight: .bold) : body
            piece.enumerateAttribute(.font, in: NSRange(location: 0, length: piece.length)) { value, range, _ in
                if value == nil || level > 0 { piece.addAttribute(.font, value: font, range: range) }
            }
            piece.addAttribute(.foregroundColor, value: NSColor.labelColor, range: NSRange(location: 0, length: piece.length))
            result.append(piece)
            result.append(NSAttributedString(string: "\n", attributes: [.font: body]))
        }
        return result
    }

    var debugSummary: String {
        let text = textView.string
        return "\(window?.title ?? "") chars=\(text.count) first=\(text.split(separator: "\n").first ?? "")"
    }
}

extension AppDelegate {
    /// Help ▸ RustyFinder Help (⌘?).
    @objc func showRustyFinderHelp(_ sender: Any?) {
        HelpWindowController.show()
    }
}
