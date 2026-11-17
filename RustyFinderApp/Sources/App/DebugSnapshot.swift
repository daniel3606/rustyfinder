import AppKit

/// Script-driven UI checks. Both flags work in-process, so they need no
/// Accessibility or Screen Recording permission and run with the display off.
///
///   --keys "down,return,cmd+up,type:abc"   feed key presses to the key window
///   --snapshot <file.png>                  then draw the window to a PNG and quit
///   --dump                                 then print the pane state and quit
///
/// Key tokens: up down left right home end return enter delete tab space escape, a single
/// character, or `type:<text>`; prefix with cmd+ shift+ alt+ ctrl+.
/// Other tokens: `wait` (do nothing for one step), `touch:<path>` (create an
/// empty file), `rm:<path>` (delete a file or empty folder), f5 / f6,
/// `answer:<button>` (press a button of the prompt on screen, e.g.
/// `answer:replace`, `answer:skip!` to also tick "Apply to all"),
/// `confirm:<button>` (press a button of a confirmation sheet),
/// `palette:<text>` and `palettekey:<down|up|tab|return|escape>` (type and
/// press keys in Quick Open or Go to Folder), `qs:<pattern>`,
/// `qsmode:<0|1|2>` and `qskey:<return|escape>` (Quick Select),
/// `mrrules:<file with rules JSON>`, `mrmove:<from>><to>` and `mrrename`
/// (Multi-Rename), `sync:<what>` (Sync window; see its `debugSet`),
/// `menu:<title>` (choose a menu command by its title),
/// `settings:<what>` (Settings window; see its `debugSet`),
/// `workspace:<save|open|new>=<name>`,
/// `tag:<name>` (toggle a tag on the selection), `tagsedit:<a+b>` (set the
/// selection's common tags, as the Tags pop-over does), `showtag:<name>`,
/// `tool:<what>` (Tools window), `checksum:<what>` (Checksum window),
/// `commit:<text>` (type into the Commit sheet),
/// `archivepw:<password>` (answer the next archive password question),
/// `compress:<what>` (Compress with Options sheet; see its `debugSet`),
/// `search:<text>` (type into the search field), `searchkey:<return|escape>`,
/// `searchopt:<option>` (see `SearchFieldController.set(option:)`),
/// `connect:<host or sftp url>` (open an SFTP connection in the active
/// pane), `disconnect` (close the active pane's connection), `connectfill:<text>` (type into the Connect panel's Server field),
/// `previewtext:<text>` (replace the text in the preview's quick edit),
/// `previewsave` (save it), `previewmode:<preview|activities|log>`, `setoctal:<digits>` (type into the Info pane's
/// Octal field), `infoclick:<title>` (click a control of the Info pane).
@MainActor
protocol DebugDumping {
    /// Plain-text description of what the UI currently shows.
    func debugDump() -> String
}

enum DebugSnapshot {
    private static func value(of flag: String) -> String? {
        let args = CommandLine.arguments
        guard let i = args.firstIndex(of: flag), args.count > i + 1 else { return nil }
        return args[i + 1]
    }

    static var outputPath: String? { value(of: "--snapshot") }
    static var keys: String? { value(of: "--keys") }
    static var wantsDump: Bool { CommandLine.arguments.contains("--dump") }

    /// Arguments that belong to these flags, so they are not taken for a path.
    static var consumedArguments: Set<String> {
        Set([outputPath, keys].compactMap { $0 })
    }

    @MainActor
    static func runIfRequested() {
        guard outputPath != nil || keys != nil || wantsDump else { return }
        let steps = (keys ?? "").split(separator: ",").map { String($0).trimmingCharacters(in: .whitespaces) }
        let stepDelay = 0.25
        var at = 0.8  // let the first folder load
        for step in steps {
            DispatchQueue.main.asyncAfter(deadline: .now() + at) { press(step) }
            at += stepDelay
        }
        guard outputPath != nil || wantsDump else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + at + 0.4) {
            if wantsDump, let dumper = targetWindow?.windowController as? DebugDumping {
                print(dumper.debugDump())
            }
            var ok = true
            if let path = outputPath { ok = capture(to: path) }
            // exit() skips applicationWillTerminate, so save the session here
            // for checks that relaunch and expect it back.
            (NSApp.delegate as? AppDelegate)?.saveSession()
            exit(ok ? 0 : 1)
        }
    }

    /// The browser window the script drives. Helper windows (Multi-Rename,
    /// Sync) have their own tokens and never take its place.
    @MainActor
    private static var targetWindow: NSWindow? {
        if let key = NSApp.keyWindow, key.windowController is MainWindowController { return key }
        return NSApp.windows.first { $0.isVisible && $0.windowController is MainWindowController }
            ?? NSApp.keyWindow
            ?? NSApp.windows.first { $0.isVisible && $0.contentViewController != nil }
    }

    @MainActor
    private static func press(_ token: String) {
        if token == "wait" { return }
        if LocalOpsDebug.handle(token) { return }
        if AreaDebug.handle(token, window: targetWindow?.windowController as? MainWindowController) { return }
        if token.hasPrefix("touch:") {
            FileManager.default.createFile(atPath: String(token.dropFirst(6)), contents: nil)
            return
        }
        if token.hasPrefix("rm:") {
            try? FileManager.default.removeItem(atPath: String(token.dropFirst(3)))
            return
        }
        if token.hasPrefix("mrrules:") {
            // The rules are JSON (full of commas), so they come from a file.
            let json = (try? String(contentsOfFile: String(token.dropFirst(8)), encoding: .utf8)) ?? "[]"
            MultiRenameWindowController.current?.debugSetRules(json)
            return
        }
        if token.hasPrefix("mrmove:") {
            let ends = token.dropFirst(7).split(separator: ">").compactMap { Int($0) }
            if ends.count == 2 { MultiRenameWindowController.current?.moveItem(from: ends[0], to: ends[1]) }
            return
        }
        if token == "mrrename" {
            MultiRenameWindowController.current?.debugRename()
            return
        }
        if token.hasPrefix("archivepw:") {
            ArchiveCenter.shared.scriptedPasswords.append(String(token.dropFirst(10)))
            return
        }
        if token.hasPrefix("compress:") {
            CompressSheet.current?.debugSet(String(token.dropFirst(9)))
            return
        }
        if token.hasPrefix("tool:") {
            ToolsWindowController.current?.debugSet(String(token.dropFirst(5)))
            return
        }
        if token.hasPrefix("checksum:") {
            ChecksumWindowController.current?.debugSet(String(token.dropFirst(9)))
            return
        }
        if token.hasPrefix("commit:") {
            CommitSheet.current?.debugType(String(token.dropFirst(7)))
            return
        }
        if token.hasPrefix("settings:") {
            SettingsWindowController.current?.debugSet(String(token.dropFirst(9)))
            return
        }
        if token.hasPrefix("workspace:") {
            // `save=<name>`, `open=<name>` (replaces this window), `new=<name>`.
            let parts = token.dropFirst(10).split(separator: "=", maxSplits: 1).map(String.init)
            guard parts.count == 2, let window = targetWindow?.windowController as? MainWindowController,
                  let app = NSApp.delegate as? AppDelegate
            else { return }
            if parts[0] == "save" {
                window.saveWorkspace(named: parts[1])
            } else if let workspace = AppEnvironment.shared.core.workspaces().first(where: { $0.name == parts[1] }) {
                app.openWorkspace(workspace, replacing: parts[0] == "open" ? window : nil)
            }
            return
        }
        if token.hasPrefix("sync:") {
            SyncWindowController.current?.debugSet(String(token.dropFirst(5)))
            return
        }
        if let window = targetWindow?.windowController as? MainWindowController, window.chromePress(token) {
            return
        }
        if let window = targetWindow?.windowController as? MainWindowController {
            if token.hasPrefix("connect:") {
                window.debugConnect(String(token.dropFirst(8)))
                return
            }
            if token.hasPrefix("palette:") {
                window.debugPalette?.debugType(String(token.dropFirst(8)))
                return
            }
            if token.hasPrefix("palettekey:") {
                window.debugPalette?.debugKey(String(token.dropFirst(11)))
                return
            }
            if token.hasPrefix("qs:") {
                window.debugQuickSelect?.debugType(String(token.dropFirst(3)))
                return
            }
            if token.hasPrefix("qsmode:") {
                window.debugQuickSelect?.debugMode(Int(token.dropFirst(7)) ?? 0)
                return
            }
            if token.hasPrefix("qskey:") {
                window.debugQuickSelect?.debugKey(String(token.dropFirst(6)))
                return
            }
            if token.hasPrefix("column:"), let list = window.debugActiveList,
               let column = ListColumn(rawValue: String(token.dropFirst(7))) {
                list.setColumn(column, visible: list.tableView.tableColumn(withIdentifier: column.id)?.isHidden != false)
                return
            }
            if token.hasPrefix("springhover:"), let list = window.debugActiveList {
                let name = String(token.dropFirst(12))
                list.springHover(over: name.isEmpty ? nil : (list.content.path.map { ($0 as NSString).appendingPathComponent(name) }), force: true)
                return
            }
            if token.hasPrefix("slowclick:"), let list = window.debugActiveList,
               let row = list.content.snapshot?.indexOf(name: String(token.dropFirst(10))) {
                list.slowClick(tableRow: list.content.tableRow(ofItem: Int(row)))
                return
            }
            if token.hasPrefix("tag:") {
                window.debugActiveList?.toggleTag(String(token.dropFirst(4)))
                return
            }
            if token.hasPrefix("tagsedit:") {
                let edited = token.dropFirst(9).split(separator: "+").map(String.init)
                window.debugActiveList?.debugEditTags(edited)
                return
            }
            if token.hasPrefix("showtag:") {
                window.showTagged(String(token.dropFirst(8)))
                return
            }
            if token.hasPrefix("search:") {
                window.debugSearch.debugType(String(token.dropFirst(7)))
                return
            }
            if token.hasPrefix("searchopt:") {
                window.debugSearch.set(option: String(token.dropFirst(10)))
                return
            }
            if token == "searchkey:return" {
                window.debugSearch.submit()
                return
            }
            if token == "searchkey:escape" {
                window.debugSearch.cancel()
                return
            }
            if token == "disconnect" {
                window.disconnect(nil)
                return
            }
            if token.hasPrefix("connectfill:") {
                window.debugConnectSheet?.debugFill(String(token.dropFirst(12)))
                return
            }
            if token.hasPrefix("connectset:") {
                window.debugConnectSheet?.debugSet(String(token.dropFirst(11)))
                return
            }
            if token.hasPrefix("remoteedit:") {
                // remoteedit:<name>=<text>: save new text into an open remote file.
                let pair = token.dropFirst(11).split(separator: "=", maxSplits: 1).map(String.init)
                if pair.count == 2 { RemoteFiles.shared.debugEdit(name: pair[0], text: pair[1]) }
                return
            }
            if token.hasPrefix("remote:") {
                window.debugActiveList?.debugRemote(String(token.dropFirst(7)))
                return
            }
            let pane = window.debugPreview
            if token.hasPrefix("previewtext:") {
                pane.debugTextPreview.debugReplaceText(String(token.dropFirst(12)))
                return
            }
            if token.hasPrefix("previewmode:") {
                let modes: [String: PreviewPaneViewController.Mode] = ["preview": .preview, "activities": .activities, "log": .log]
                if let mode = modes[String(token.dropFirst(12))] { pane.show(mode: mode) }
                return
            }
            if token == "previewsave" {
                pane.debugTextPreview.save()
                return
            }
            if token.hasPrefix("setoctal:") {
                pane.debugInfo.debugSetOctal(String(token.dropFirst(9)))
                return
            }
            if token.hasPrefix("infoclick:") {
                let wanted = String(token.dropFirst(10)).lowercased()
                allButtons(in: pane.debugInfo.view).first { $0.title.lowercased() == wanted }?.performClick(nil)
                return
            }
        }
        if token.hasPrefix("viewoptions:") {
            ViewOptionsPanel.shared.debugSet(String(token.dropFirst(12)))
            return
        }
        if token.hasPrefix("menu:") {
            if let window = targetWindow { performMenuItem(titled: String(token.dropFirst(5)), in: window) }
            return
        }
        if token.hasPrefix("answer:") {
            ActivityCenter.shared.debugAnswer(String(token.dropFirst(7)))
            return
        }
        if token.hasPrefix("confirm:") {
            // Any other sheet on the window: press the button with this title.
            let wanted = String(token.dropFirst(8))
            // The browser window's sheet first, then a helper window's.
            let sheets = ([targetWindow] + NSApp.windows).compactMap { $0?.attachedSheet }
            guard let sheet = sheets.first else { return }
            let buttons = sheet.contentView.map { allButtons(in: $0) } ?? []
            if let index = buttons.firstIndex(where: { $0.title.lowercased() == wanted }) {
                buttons[index].performClick(nil)
            }
            return
        }
        if token.hasPrefix("type:") {
            for ch in token.dropFirst(5) { send(String(ch), keyCode: keyCode(for: String(ch)), flags: []) }
            return
        }
        var parts = token.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        // "cmd++" style tokens: the key itself is "+".
        if token.hasSuffix("++") { parts = Array(parts.dropLast(2)) + ["+"] }
        guard let key = parts.last else { return }
        var flags: NSEvent.ModifierFlags = []
        for mod in parts.dropLast() {
            switch mod {
            case "cmd": flags.insert(.command)
            case "shift": flags.insert(.shift)
            case "alt": flags.insert(.option)
            case "ctrl": flags.insert(.control)
            default: break
            }
        }
        let special: [String: (Int, UInt16)] = [
            "f5": (NSF5FunctionKey, 96), "f6": (NSF6FunctionKey, 97),
            "home": (NSHomeFunctionKey, 115), "end": (NSEndFunctionKey, 119),
            "up": (NSUpArrowFunctionKey, 126), "down": (NSDownArrowFunctionKey, 125),
            "left": (NSLeftArrowFunctionKey, 123), "right": (NSRightArrowFunctionKey, 124),
            "return": (13, 36), "enter": (3, 76), "delete": (127, 51), "tab": (9, 48),
            "space": (32, 49), "escape": (27, 53),
        ]
        if let (scalar, code) = special[key] {
            if code >= 123 { flags.insert([.function, .numericPad]) }
            if [96, 97, 115, 119].contains(code) { flags.insert(.function) }
            send(String(UnicodeScalar(scalar)!), keyCode: code, flags: flags)
        } else {
            send(key, keyCode: keyCode(for: key), flags: flags)
        }
    }

    /// US-ANSI virtual key codes. Menu shortcuts are matched by key code, so
    /// a synthetic event must carry the right one.
    private static func keyCode(for key: String) -> UInt16 {
        let codes: [String: UInt16] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
            "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
            "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28,
            "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
            "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
            "`": 50,
        ]
        return codes[key.lowercased()] ?? 0
    }

    @MainActor
    private static func send(_ chars: String, keyCode: UInt16, flags: NSEvent.ModifierFlags) {
        guard let window = targetWindow,
              let event = NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: flags,
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                context: nil, characters: chars, charactersIgnoringModifiers: chars.lowercased(),
                isARepeat: false, keyCode: keyCode)
        else { return }
        // Same order as for a real key: menu shortcuts first, then the window.
        let isFunctionKey = chars.unicodeScalars.first.map { (0xF704...0xF70F).contains($0.value) } ?? false
        // Cmd-Delete arrives as DEL (127); menus bind it as Backspace (8).
        let menuKey = chars == "\u{7f}" ? "\u{8}" : chars.lowercased()
        // A Command or Control combination (or a function key) is a
        // shortcut even when its command is disabled: it is swallowed.
        // Other keys (a bare key or Option combination the user bound)
        // only count if their command can run now; otherwise they are
        // ordinary typing, for example Delete during a rename.
        let isShortcut = !flags.intersection([.command, .control]).isEmpty || isFunctionKey
        if performMenuShortcut(menuKey, flags: flags, in: window, swallowDisabled: isShortcut) {
            return
        }
        // Plain keys go to whatever has focus: a sheet, if one is up.
        if let sheet = window.attachedSheet {
            sheet.sendEvent(event)
            return
        }
        window.sendEvent(event)
    }

    /// Find the menu item for a shortcut and run its action down the window's
    /// responder chain. AppKit's own dispatch needs a key window, which a
    /// script-launched app (not frontmost, display possibly asleep) lacks.
    @MainActor
    private static func performMenuShortcut(_ key: String, flags: NSEvent.ModifierFlags, in window: NSWindow, swallowDisabled: Bool) -> Bool {
        let wanted = flags.intersection([.command, .shift, .option, .control])
        func find(_ menu: NSMenu?) -> NSMenuItem? {
            for item in menu?.items ?? [] {
                if item.keyEquivalent == key, item.keyEquivalentModifierMask == wanted { return item }
                if let hit = find(item.submenu) { return hit }
            }
            return nil
        }
        guard let item = find(NSApp.mainMenu) else { return false }
        return perform(item, in: window, swallowDisabled: swallowDisabled)
    }

    /// Run the menu item with this title (`menu:<title>`), for commands
    /// that have no shortcut.
    @MainActor
    private static func performMenuItem(titled title: String, in window: NSWindow) {
        func find(_ menu: NSMenu?) -> NSMenuItem? {
            for item in menu?.items ?? [] {
                if item.title == title, item.action != nil, !item.hasSubmenu { return item }
                if let hit = find(item.submenu) { return hit }
            }
            return nil
        }
        if let item = find(NSApp.mainMenu) { _ = perform(item, in: window, swallowDisabled: true) }
    }

    @MainActor
    private static func perform(_ item: NSMenuItem, in window: NSWindow, swallowDisabled: Bool) -> Bool {
        guard let action = item.action else { return false }

        // As AppKit does: the key window's chain first (a sheet, if one is
        // up), then the main window's. Commands for the panes are found in
        // the main window's chain and must refuse to run behind a sheet.
        var chain: [AnyObject] = []
        for start in [window.attachedSheet?.firstResponder, window.firstResponder] {
            var responder = start
            while let r = responder {
                chain.append(r)
                responder = r.nextResponder
            }
        }
        chain += [window.delegate, NSApp, NSApp.delegate].compactMap { $0 as AnyObject? }
        for target in chain where target.responds(to: action) {
            if let validator = target as? NSUserInterfaceValidations,
               !validator.validateUserInterfaceItem(item)
            {
                return swallowDisabled  // a disabled shortcut is swallowed
            }
            _ = target.perform(action, with: item)
            return true
        }
        return swallowDisabled
    }

    @MainActor
    private static func allButtons(in view: NSView) -> [NSButton] {
        view.subviews.flatMap { sub in
            ((sub as? NSButton).map { [$0] } ?? []) + allButtons(in: sub)
        }
    }

    @MainActor
    private static func capture(to path: String) -> Bool {
        // RUSTYFINDER_SNAPSHOT_WINDOW picks another window by its title (the
        // Settings window, a sheet…); otherwise the browser window.
        let wanted = ProcessInfo.processInfo.environment["RUSTYFINDER_SNAPSHOT_WINDOW"]
        let other = wanted.flatMap { title in
            NSApp.windows.first { $0.isVisible && $0.title.localizedCaseInsensitiveContains(title) }
                ?? (title == "sheet" ? targetWindow?.attachedSheet : nil)
        }
        // The frame view (content view's superview) includes the toolbar.
        guard let window = other ?? targetWindow,
              let view = window.contentView?.superview ?? window.contentView,
              let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else { return false }
        view.cacheDisplay(in: view.bounds, to: rep)
        guard let data = rep.representation(using: .png, properties: [:]) else { return false }
        return (try? data.write(to: URL(fileURLWithPath: path))) != nil
    }
}
