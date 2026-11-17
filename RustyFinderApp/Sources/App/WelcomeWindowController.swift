import AppKit

/// Full Disk Access: whether RustyFinder has it, and the prompt that
/// explains why it helps (panes-tabs §9).
@MainActor
enum FullDiskAccess {
    /// The prompt on screen, for scripted checks.
    private(set) static var lastPrompt = "-"
    private static var promptedThisLaunch = false

    /// True if a folder only such apps can read is readable. Scripted
    /// checks say what to pretend with `RUSTYFINDER_FDA=granted|denied`.
    static var hasAccess: Bool {
        if let forced = ProcessInfo.processInfo.environment["RUSTYFINDER_FDA"] { return forced == "granted" }
        let probe = fullDiskAccessProbe(home: NSHomeDirectory())
        return (try? FileManager.default.contentsOfDirectory(atPath: probe)) != nil
    }

    static let settingsURL = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!

    static func openSystemSettings() {
        ExternalApps.open(settingsURL)
    }

    /// A folder could not be read. If it is one macOS guards and the user
    /// has not chosen to go on without access, explain once per launch.
    static func loadFailed(path: String, error: RustyFinderError, in window: NSWindow?) {
        // macOS privacy protection refuses with EPERM ("Operation not
        // permitted"); ordinary Unix permissions give EACCES ("Permission
        // denied"), which Full Disk Access would not fix.
        guard case .PermissionDenied(let message) = error, message.contains("Operation not permitted"),
              isPrivacyProtected(path: path, home: NSHomeDirectory())
        else { return }
        prompt(in: window)
    }

    /// The general prompt: open System Settings, or continue without (a
    /// choice that is remembered).
    static func prompt(in window: NSWindow?) {
        guard !AppEnvironment.shared.settings.fdaDismissed, !promptedThisLaunch, !hasAccess, let window,
              window.attachedSheet == nil
        else { return }
        promptedThisLaunch = true
        let alert = NSAlert()
        alert.messageText = "RustyFinder needs Full Disk Access to show some folders."
        alert.informativeText = "macOS keeps folders such as Mail, Messages and Safari from apps without Full Disk Access. "
            + "Turn it on for RustyFinder in System Settings › Privacy & Security › Full Disk Access."
        alert.addButton(withTitle: "Open System Settings")
        alert.addButton(withTitle: "Continue without Full Disk Access")
        lastPrompt = alert.messageText
        alert.beginSheetModal(for: window) { response in
            lastPrompt = "-"
            if response == .alertFirstButtonReturn {
                openSystemSettings()
            } else {
                AppEnvironment.shared.update { $0.fdaDismissed = true }
            }
        }
    }
}

/// The welcome shown on first launch: pick a theme, grant Full Disk
/// Access (or go on without), start. Help › Welcome to RustyFinder shows
/// it again.
final class WelcomeWindowController: NSWindowController, NSWindowDelegate {
    private(set) static var current: WelcomeWindowController?
    private let themePopup = NSPopUpButton()
    private let fdaStatus = NSTextField(labelWithString: "")
    private let continueBox = NSButton(checkboxWithTitle: "Continue without Full Disk Access", target: nil, action: nil)

    static func show() {
        if current == nil { current = WelcomeWindowController() }
        current?.refresh()
        current?.showWindow(nil)
        current?.window?.makeKeyAndOrderFront(nil)
    }

    /// First launch only.
    static func showIfFirstLaunch() {
        if !AppEnvironment.shared.settings.welcomeShown { show() }
    }

    private init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 360),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Welcome to RustyFinder"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.center()
        super.init(window: window)
        window.delegate = self
        buildUI()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        // Closing counts as done: the welcome is not forced again.
        AppEnvironment.shared.update { $0.welcomeShown = true }
        if Self.current === self { Self.current = nil }
    }

    private func buildUI() {
        guard let content = window?.contentView else { return }
        let title = NSTextField(labelWithString: "Welcome to RustyFinder")
        title.font = .systemFont(ofSize: 22, weight: .semibold)
        let intro = NSTextField(wrappingLabelWithString: "Two panes, tabs and everything on your Mac and your servers. Two quick choices and you are set.")
        intro.textColor = .secondaryLabelColor

        let themeTitle = NSTextField(labelWithString: "1. Choose a theme")
        themeTitle.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
        themePopup.target = self
        themePopup.action = #selector(themeChosen(_:))

        let fdaTitle = NSTextField(labelWithString: "2. Full Disk Access")
        fdaTitle.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
        let fdaText = NSTextField(wrappingLabelWithString: "To show protected folders (Mail, Messages, Safari and others), macOS needs you to allow RustyFinder in System Settings.")
        fdaText.textColor = .secondaryLabelColor
        let open = NSButton(title: "Open System Settings", target: self, action: #selector(openSettings(_:)))
        fdaStatus.textColor = .secondaryLabelColor

        let start = NSButton(title: "Start using RustyFinder", target: self, action: #selector(start(_:)))
        start.keyEquivalent = "\r"
        start.bezelColor = .controlAccentColor

        let stack = NSStackView(views: [
            title, intro, themeTitle, themePopup, fdaTitle, fdaText, NSStackView(views: [open, fdaStatus]), continueBox, start,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.setCustomSpacing(18, after: intro)
        stack.setCustomSpacing(18, after: themePopup)
        stack.setCustomSpacing(18, after: continueBox)
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 22, right: 26)
        stack.translatesAutoresizingMaskIntoConstraints = false
        for label in [intro, fdaText] { label.widthAnchor.constraint(equalToConstant: 460).isActive = true }
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: content.topAnchor),
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: content.bottomAnchor),
        ])
    }

    private func refresh() {
        let core = AppEnvironment.shared.core
        themePopup.removeAllItems()
        for theme in core.themes() {
            themePopup.addItem(withTitle: theme.name)
            themePopup.lastItem?.representedObject = theme.id
        }
        let current = core.currentTheme().id
        themePopup.selectItem(at: themePopup.itemArray.firstIndex { ($0.representedObject as? String) == current } ?? 0)
        fdaStatus.stringValue = FullDiskAccess.hasAccess ? "Granted" : "Not granted yet"
        continueBox.state = AppEnvironment.shared.settings.fdaDismissed ? .on : .off
    }

    @objc private func themeChosen(_ sender: Any?) {
        guard let id = themePopup.selectedItem?.representedObject as? String else { return }
        if AppEnvironment.shared.core.selectTheme(id: id) { ThemeCenter.shared.reload() }
    }

    @objc private func openSettings(_ sender: Any?) {
        FullDiskAccess.openSystemSettings()
    }

    @objc private func start(_ sender: Any?) {
        let skip = continueBox.state == .on
        AppEnvironment.shared.update {
            $0.welcomeShown = true
            if skip { $0.fdaDismissed = true }
        }
        close()
    }

    // MARK: Scripted checks

    /// `theme=<name>`, `continue` (tick "Continue without"), `fda`
    /// (Open System Settings), `start`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        switch parts[0] {
        case "theme":
            if parts.count > 1 { themePopup.selectItem(withTitle: parts[1]) }
            themeChosen(nil)
        case "continue": continueBox.performClick(nil)
        case "fda": openSettings(nil)
        case "start": start(nil)
        default: break
        }
    }

    func debugDump() -> String {
        "theme=\(themePopup.titleOfSelectedItem ?? "") fda=\(fdaStatus.stringValue) continue=\(continueBox.state == .on)"
    }
}
