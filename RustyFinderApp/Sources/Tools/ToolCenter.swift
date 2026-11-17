import AppKit

/// The user's custom tools: the list, running one, and showing what it
/// printed. Commands and quoting live in the Rust core.
@MainActor
final class ToolCenter {
    static let shared = ToolCenter()
    private var core: RustyFinderApp { AppEnvironment.shared.core }

    private(set) lazy var tools: [ToolRecord] = core.tools()
    /// Runs in flight: activity id -> (tool name, show its output).
    private var running: [UInt64: (name: String, showOutput: Bool)] = [:]
    /// What the last finished tool printed (scripted checks).
    private(set) var lastOutput = "-"

    /// Save a changed list (order included); returns it as stored.
    @discardableResult
    func save(_ list: [ToolRecord]) -> [ToolRecord] {
        tools = core.setTools(tools: list)
        if let menu = MainMenu.toolsMenu { ToolsMenuDelegate.fill(menu) }
        NotificationCenter.default.post(name: .rustyfinderToolsChanged, object: nil)
        MainWindowController.toolsChanged()
        return tools
    }

    func tool(id: UInt64) -> ToolRecord? {
        tools.first { $0.id == id }
    }

    /// Run `tool` for a window. `cwd` is the active pane's folder if it is
    /// on this Mac, else the home folder.
    func run(_ tool: ToolRecord, context: ToolContext, cwd: String, window: UInt64) {
        guard let id = core.runTool(window: window, tool: tool, context: context, cwd: cwd) else { return }
        running[id] = (tool.name, tool.showOutput)
    }

    /// Called for every finished activity.
    func activityFinished(_ report: FinishRecord) {
        guard let run = running.removeValue(forKey: report.id) else { return }
        let output = report.output ?? ""
        lastOutput = output.replacingOccurrences(of: "\n", with: "⏎")
        if run.showOutput {
            OutputWindowController.show(title: run.name, text: output)
        }
    }

    /// Menu items for the tools, each running its tool in the key window.
    func menuItems(contextMenuOnly: Bool) -> [NSMenuItem] {
        tools.filter { !contextMenuOnly || $0.inContextMenu }.map { tool in
            let item = NSMenuItem(title: tool.name, action: #selector(MainWindowController.runTool(_:)), keyEquivalent: "")
            item.representedObject = tool.id
            // A bare key would swallow typing in the file list.
            if !contextMenuOnly, tool.shortcut.contains("+"), let key = ShortcutKeys.menuKey(for: tool.shortcut.lowercased()) {
                item.keyEquivalent = key.key
                item.keyEquivalentModifierMask = key.modifiers
            }
            return item
        }
    }
}

/// Fills Commands > Tools. The menu is filled at launch and after every
/// change as well, so tool shortcuts work before it is ever opened.
final class ToolsMenuDelegate: NSObject, NSMenuDelegate {
    func menuNeedsUpdate(_ menu: NSMenu) {
        MainActor.assumeIsolated { Self.fill(menu) }
    }

    @MainActor
    static func fill(_ menu: NSMenu) {
        menu.removeAllItems()
        for item in ToolCenter.shared.menuItems(contextMenuOnly: false) { menu.addItem(item) }
        if !menu.items.isEmpty { menu.addItem(.separator()) }
        menu.addItem(NSMenuItem(title: "Edit Tools…", action: #selector(AppDelegate.editTools(_:)), keyEquivalent: ""))
    }
}

/// Opening things in other apps. Scripted checks run unattended (often at
/// night), so there the request is recorded instead of carried out.
@MainActor
enum ExternalApps {
    private(set) static var lastRequest = "-"
    /// True in a scripted check: requests are recorded, not carried out.
    static var isDryRun: Bool {
        ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"] != nil && DebugSnapshot.keys != nil
    }
    private static var dryRun: Bool { isDryRun }

    /// Note a request that a scripted check would otherwise have made.
    static func record(_ request: String) {
        lastRequest = request
    }

    /// Hand a URL to the system (a `vnc://` address goes to Screen Sharing).
    static func open(_ url: URL) {
        lastRequest = "open \(url.absoluteString)"
        guard !dryRun else { return }
        NSWorkspace.shared.open(url)
    }

    /// Open `urls` with the app at `app` (a bundle path).
    static func open(_ urls: [URL], withAppAt app: String) {
        lastRequest = "\((app as NSString).lastPathComponent) <- \(urls.map(\.path).joined(separator: ", "))"
        guard !dryRun else { return }
        NSWorkspace.shared.open(urls, withApplicationAt: URL(fileURLWithPath: app), configuration: NSWorkspace.OpenConfiguration())
    }

    /// The Terminal application: the one named in `RUSTYFINDER_TERMINAL_APP`
    /// (a bundle path), else Apple's Terminal.
    static var terminalApp: String {
        if let override = ProcessInfo.processInfo.environment["RUSTYFINDER_TERMINAL_APP"] { return override }
        let chosen = AppEnvironment.shared.settings.terminalApp
        return chosen.isEmpty ? "/System/Applications/Utilities/Terminal.app" : chosen
    }
}
