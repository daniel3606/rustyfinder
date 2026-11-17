import AppKit

/// Scripted-check tokens and dump lines for preview, archives, tools, sync,
/// rename and search additions. Kept apart from DebugSnapshot so several
/// areas can add tokens without touching the same lines.
///
/// Tokens: `searchstop`, `toolbarclick:<label>`, `comparetool:<key>`,
/// `settingsclick:<identifier>` (a control of the Settings window),
/// `ctxmenu` (build the item context menu for the selection, icon view).
@MainActor
enum AreaDebug {
    /// Titles of the last context menu `ctxmenu` built.
    private static var lastContextMenu = "-"

    /// Handle `token` if it is one of ours. Returns true if handled.
    static func handle(_ token: String, window: MainWindowController?) -> Bool {
        let (name, value) = split(token)
        switch name {
        case "previewdownload":
            window?.debugPreview.debugDownload()
        case "previewpdf":
            // `mode` or `mode/scale`, e.g. 0/1.5 (0: single page).
            let parts = value.split(separator: "/").map(String.init)
            window?.debugPreview.debugPDF(mode: Int(parts.first ?? "") ?? 1, scale: parts.count > 1 ? Double(parts[1]) : nil)
        case "infotarget":
            // Point the Info pane's link or alias at another item.
            if let info = window?.debugPreview.debugInfo, let item = info.items.first { info.pointOriginal(of: item, to: value) }
        case "infotags":
            window?.debugPreview.debugInfo.debugSetTags(value.split(separator: "+").map(String.init))
        case "slideshowtick":
            QuickLookDownloads.advance()
        case "dragout":
            // Drop the first selected item of an archive into this folder.
            guard let list = window?.debugActiveList, let index = list.selection.first,
                  let path = list.content.path(at: index), let row = list.content.row(at: index),
                  let delegate = ArchiveDragOut.provider(conn: list.conn, path: path, row: row).userInfo as? ArchiveDragOut
            else { return true }
            delegate.write(to: URL(fileURLWithPath: value).appendingPathComponent(row.name)) { _ in }
        case "searchstop":
            window?.debugActivePane.debugSearchProgress.debugStop()
        case "toolbarclick":
            guard let item = window?.window?.toolbar?.items.first(where: { $0.label == value }),
                  let action = item.action
            else { return true }
            NSApp.sendAction(action, to: item.target, from: item)
        case "comparetool":
            AppEnvironment.shared.update { $0.compareTool = value }
        case "settingsclick":
            guard let content = SettingsWindowController.current?.window?.contentView else { return true }
            controls(in: content).first { $0.identifier?.rawValue == value }?.performClick(nil)
        case "ctxmenu":
            lastContextMenu = window?.debugActiveList.map(contextMenuTitles) ?? "-"
        // Multi-Rename: add a rule, edit the selected one, switch one off, move one.
        case "mradd":
            MultiRenameWindowController.current?.debugAddRule(value)
        case "mredit":
            let parts = value.split(separator: "=", maxSplits: 1).map(String.init)
            if parts.count == 2 { MultiRenameWindowController.current?.debugEdit(parts[0], parts[1]) }
        case "mrtoggle":
            let parts = value.split(separator: "=").map(String.init)
            if parts.count == 2, let row = Int(parts[0]) {
                MultiRenameWindowController.current?.setEnabled(rule: row, parts[1] == "on")
            }
        case "mrrulemove":
            let ends = value.split(separator: ">").compactMap { Int($0) }
            if ends.count == 2 { MultiRenameWindowController.current?.moveRule(from: ends[0], to: ends[1]) }
        default:
            return false
        }
        return true
    }

    /// Extra lines for the window's state dump.
    static func dump(_ window: MainWindowController) -> [String] {
        var lines: [String] = []
        lines.append("search.progress: \(window.debugActivePane.debugSearchProgress.isRunning ? "running" : "idle")")
        let items = window.window?.toolbar?.items.map(\.label).filter { !$0.isEmpty } ?? []
        lines.append("toolbar: \(items.joined(separator: "|"))")
        let settings = AppEnvironment.shared.settings
        lines.append("tools.inheritpath: \(settings.toolsInheritPath ? "on" : "off") compare=\(settings.compareTool)")
        lines.append("grid.scm: \(window.debugActiveList?.debugGridScmBadges ?? "")")
        lines.append("ctxmenu: \(lastContextMenu)")
        lines.append("grid.thumbs: \(window.debugActiveList?.debugGridThumbnails ?? "")")
        lines.append("ql.position: \(QuickLookDownloads.debugPosition) slideshow=\(QuickLookDownloads.slideshowRunning)")
        lines.append("ql.items: \(window.debugActiveList?.previewURLs.map(\.lastPathComponent).joined(separator: "|") ?? "")")
        lines.append("dragout: \(ArchiveDragOut.lastResult)")
        if let info = InfoWindowController.debugControllers.first {
            lines.append("infowindow.changes: \(info.outsideChanges)")
        }
        return lines
    }

    /// The item context menu for the first selected icon, as titles, with
    /// submenus in brackets.
    private static func contextMenuTitles(_ list: FileListViewController) -> String {
        let grid = list.gridView
        guard let index = grid.selectionIndexPaths.first, let window = grid.window,
              let frame = grid.item(at: index)?.view.frame
        else { return "-" }
        let point = grid.convert(NSPoint(x: frame.midX, y: frame.midY), to: nil)
        guard let event = NSEvent.mouseEvent(
            with: .rightMouseDown, location: point, modifierFlags: [], timestamp: 0,
            windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1),
            let menu = grid.menu(for: event)
        else { return "-" }
        menu.delegate?.menuNeedsUpdate?(menu)
        func titles(_ menu: NSMenu) -> String {
            menu.items.filter { !$0.isSeparatorItem && !$0.isHidden && !$0.isAlternate }.map { item in
                item.submenu.map { "\(item.title)[\(titles($0))]" } ?? item.title
            }.joined(separator: "|")
        }
        return titles(menu)
    }

    private static func controls(in view: NSView) -> [NSButton] {
        view.subviews.flatMap { sub in ((sub as? NSButton).map { [$0] } ?? []) + controls(in: sub) }
    }

    private static func split(_ token: String) -> (String, String) {
        guard let colon = token.firstIndex(of: ":") else { return (token, "") }
        return (String(token[..<colon]), String(token[token.index(after: colon)...]))
    }
}
