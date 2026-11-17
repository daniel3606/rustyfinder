import AppKit

/// Git status as a small badge on an item's icon (Icon and Column view),
/// and the Source Control commands for the item context menu.
@MainActor
enum ScmBadge {
    static func letter(_ status: ScmStatus) -> String {
        switch status {
        case .modified: return "M"
        case .added: return "A"
        case .untracked: return "?"
        case .ignored: return "!"
        case .conflicted: return "U"
        }
    }

    static func name(_ status: ScmStatus) -> String {
        switch status {
        case .modified: return "Modified"
        case .added: return "Added"
        case .untracked: return "Untracked"
        case .ignored: return "Ignored"
        case .conflicted: return "Conflicted"
        }
    }

    static func color(_ status: ScmStatus) -> NSColor {
        switch status {
        case .modified: return .systemOrange
        case .added: return .systemGreen
        case .untracked: return .systemBlue
        case .ignored: return .systemGray
        case .conflicted: return .systemRed
        }
    }

    private static var cache: [String: NSImage] = [:]

    /// A rounded square in the status colour with its letter, `side` points.
    static func image(_ status: ScmStatus, side: CGFloat = 16) -> NSImage {
        let key = "\(letter(status))\(Int(side))"
        if let cached = cache[key] { return cached }
        let image = NSImage(size: NSSize(width: side, height: side), flipped: false) { rect in
            let path = NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), xRadius: side / 4, yRadius: side / 4)
            color(status).setFill()
            path.fill()
            NSColor.white.withAlphaComponent(0.9).setStroke()
            path.lineWidth = 1
            path.stroke()
            let font = NSFont.systemFont(ofSize: side * 0.68, weight: .bold)
            let text = NSAttributedString(string: letter(status), attributes: [.font: font, .foregroundColor: NSColor.white])
            let size = text.size()
            text.draw(at: NSPoint(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2))
            return true
        }
        image.accessibilityDescription = name(status)
        cache[key] = image
        return image
    }

    /// An image view showing the badge (hidden for clean items), for list
    /// code to lay over an icon's corner.
    static func makeView() -> NSImageView {
        let view = NSImageView()
        view.imageScaling = .scaleProportionallyUpOrDown
        view.isHidden = true
        view.translatesAutoresizingMaskIntoConstraints = false
        return view
    }

    static func show(_ status: ScmStatus?, in view: NSImageView) {
        view.isHidden = status == nil
        view.image = status.map { image($0) }
        view.toolTip = status.map(name)
        view.setAccessibilityLabel(status.map { "Source control: \(name($0))" })
    }

    /// Source Control ▸ Add, Add All, Commit…, Pull, Push for the item
    /// context menu (the same commands as the menu bar's).
    static func contextMenuItem() -> NSMenuItem {
        let menu = NSMenu(title: "Source Control")
        for (title, action) in [
            ("Add", #selector(MainWindowController.scmAdd(_:))),
            ("Add All", #selector(MainWindowController.scmAddAll(_:))),
            ("Commit…", #selector(MainWindowController.scmCommit(_:))),
            ("Pull", #selector(MainWindowController.scmPull(_:))),
            ("Push", #selector(MainWindowController.scmPush(_:))),
        ] {
            menu.addItem(NSMenuItem(title: title, action: action, keyEquivalent: ""))
        }
        let item = NSMenuItem(title: "Source Control", action: nil, keyEquivalent: "")
        item.submenu = menu
        return item
    }
}
