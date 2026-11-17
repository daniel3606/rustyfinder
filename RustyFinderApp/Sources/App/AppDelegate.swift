import AppKit

@main
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private(set) var windowControllers: [MainWindowController] = []
    private var isTerminating = false
    /// The last request from outside (service, link, Dock), for scripted checks.
    var lastOutsideRequest = "-"
    private let serviceProvider = ServiceProvider()

    static func main() {
        let args = CommandLine.arguments
        // ssh runs this binary to ask its questions (password, passphrase,
        // host key). In that role it only relays the question to the
        // running app and prints the answer: no UI, no settings.
        if ProcessInfo.processInfo.environment["RUSTYFINDER_ASKPASS_SOCKET"] != nil, args.count >= 2 {
            exit(askpassRelay(prompt: args[1]))
        }
        // Headless check used by scripts: list a folder through the full
        // Rust -> UniFFI -> Swift path and print the result.
        if let i = args.firstIndex(of: "--selftest") {
            let path = args.count > i + 1 ? args[i + 1] : NSHomeDirectory()
            exit(SelfTest.run(path: path))
        }

        // RustyFinder restores its own windows and tabs (from the Rust session), so
        // AppKit's window restoration is switched off. Left on, AppKit also
        // blocks launch with a "reopen windows?" alert after an unclean exit.
        UserDefaults.standard.register(defaults: [
            "NSQuitAlwaysKeepsWindows": false,
            "ApplePersistenceIgnoreState": true,
        ])

        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        app.run()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let menu = MainMenu.build()
        NSApp.mainMenu = menu
        // The menus as built are the default shortcuts; the user's
        // changes go on top.
        ShortcutCenter.shared.install(menu)
        let env = AppEnvironment.shared
        ActivityCenter.shared.start()

        // A folder given on the command line opens in a fresh window.
        let reserved = DebugSnapshot.consumedArguments
        let pathArgument = CommandLine.arguments.dropFirst().first { !$0.hasPrefix("-") && !reserved.contains($0) }

        if let pathArgument {
            openWindow(restoring: nil, leftPath: pathArgument)
        } else {
            let saved = env.settings.restoreWindows ? env.core.savedWindows() : []
            if saved.isEmpty {
                openWindow(restoring: nil)
            } else {
                saved.forEach { openWindow(restoring: $0) }
            }
        }
        NSApp.servicesProvider = serviceProvider
        NSUpdateDynamicServices()
        ThemeCenter.shared.apply()
        WelcomeWindowController.showIfFirstLaunch()
        NSApp.activate(ignoringOtherApps: true)
        DebugSnapshot.runIfRequested()
    }

    @objc func showFavoriteManager(_ sender: Any?) {
        FavoriteManagerWindowController.show()
    }

    @objc func showWelcome(_ sender: Any?) {
        WelcomeWindowController.show()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { openWindow(restoring: nil) }
        return true
    }

    /// Quitting with work in flight would abort it: ask first.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let core = AppEnvironment.shared.core
        guard core.hasActive(window: nil) else { return .terminateNow }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Activities are still running."
        alert.informativeText = "Quitting stops them. Files copied so far stay where they are."
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Quit")
        guard alert.runModal() == .alertSecondButtonReturn else { return .terminateCancel }
        windowControllers.forEach { core.stopAllActivities(window: $0.session.id()) }
        return .terminateNow
    }

    func applicationWillTerminate(_ notification: Notification) {
        isTerminating = true
        RemoteFiles.shared.cleanUp()
        ArchiveCenter.shared.cleanUp()
        PreviewCache.shared.cleanUp()
        QuickLookDownloads.cleanUp()
        saveSession()
    }

    @objc func showSettings(_ sender: Any?) {
        SettingsWindowController.show()
    }

    // MARK: Workspaces

    /// Open a workspace in a window of its own; with `replacing`, that
    /// window makes way for it.
    @discardableResult
    func openWorkspace(_ workspace: WorkspaceRecord, replacing old: MainWindowController?) -> MainWindowController {
        // A window in full screen keeps its space: leave it first.
        if let window = old?.window, window.styleMask.contains(.fullScreen) { window.toggleFullScreen(nil) }
        let controller = openWindow(restoring: workspace.window)
        controller.workspaceId = workspace.id
        old?.close()
        return controller
    }

    @objc func openWorkspaceFromMenu(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? UInt64,
              let workspace = AppEnvironment.shared.core.workspaces().first(where: { $0.id == id })
        else { return }
        // Option (the alternate item) opens it beside the current window.
        let front = sender.isAlternate ? nil : NSApp.mainWindow?.windowController as? MainWindowController
        openWorkspace(workspace, replacing: front ?? (sender.isAlternate ? nil : windowControllers.last))
    }

    @objc func editTools(_ sender: Any?) {
        ToolsWindowController.show()
    }

    @objc func newWindow(_ sender: Any?) {
        openWindow(restoring: nil)
    }

    @discardableResult
    func openWindow(restoring saved: SavedWindowRecord?, leftPath: String? = nil) -> MainWindowController {
        let controller = MainWindowController(restoring: saved, leftPath: leftPath)
        windowControllers.append(controller)
        controller.onClose = { [weak self, weak controller] in
            guard let self else { return }
            self.windowControllers.removeAll { $0 === controller }
            // A window closed before quitting must not come back, so the
            // session is rewritten without it right away.
            if !self.isTerminating { self.saveSession() }
        }
        controller.showWindow(nil)
        return controller
    }

    /// Write the open windows to disk for the next launch.
    func saveSession() {
        let windows = windowControllers.map { $0.savedJSON() }
        AppEnvironment.shared.core.saveSession(windows: windows)
    }

    /// The hidden-files switch is global: it applies to every tab of every window.
    func setShowHidden(_ show: Bool) {
        AppEnvironment.shared.update { $0.showHidden = show }
        windowControllers.forEach { $0.session.setShowHidden(show: show) }
    }
}
