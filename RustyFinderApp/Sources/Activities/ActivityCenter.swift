import AppKit

extension Notification.Name {
    /// The list of running and queued activities changed.
    static let rustyfinderActivitiesChanged = Notification.Name("RustyFinderActivitiesChanged")
}

/// Moves items to the Trash for the Rust core.
private final class TrashService: PlatformServices, @unchecked Sendable {
    func trash(path: String) throws -> String {
        // Scripted checks redirect the Trash to a scratch folder so they
        // never fill the user's real one.
        if let scratch = ProcessInfo.processInfo.environment["RUSTYFINDER_TRASH_DIR"] {
            return try moveAside(path, into: scratch)
        }
        var result: NSURL?
        do {
            try FileManager.default.trashItem(at: URL(fileURLWithPath: path), resultingItemURL: &result)
        } catch {
            throw RustyFinderError.Io(message: error.localizedDescription)
        }
        guard let trashed = result?.path else {
            throw RustyFinderError.Io(message: "The item was moved to the Trash, but its new location is unknown.")
        }
        return trashed
    }

    /// A real Finder alias: bookmark data written as an alias file, so
    /// Finder and every other app resolve it, even after the original moves.
    func makeAlias(target: String, alias: String) throws {
        do {
            let data = try URL(fileURLWithPath: target).bookmarkData(
                options: .suitableForBookmarkFile, includingResourceValuesForKeys: nil, relativeTo: nil)
            try URL.writeBookmarkData(data, to: URL(fileURLWithPath: alias))
        } catch {
            throw RustyFinderError.Io(message: error.localizedDescription)
        }
    }

    func setTags(path: String, tags: [String]) throws {
        do {
            try (URL(fileURLWithPath: path) as NSURL).setResourceValue(tags, forKey: .tagNamesKey)
        } catch {
            throw RustyFinderError.Io(message: error.localizedDescription)
        }
    }

    func setComment(path: String, comment: String) throws {
        if let message = FinderComments.set(comment, at: path) {
            throw RustyFinderError.Io(message: message)
        }
    }

    /// A step retried as an administrator. A Trash step goes to the
    /// user's Trash under a name not yet taken there.
    func elevate(step: PrivilegedStep) throws -> String {
        var argv = step.argv
        var result = ""
        if step.kind == "trash" {
            let trash = ProcessInfo.processInfo.environment["RUSTYFINDER_TRASH_DIR"]
                ?? FileManager.default.urls(for: .trashDirectory, in: .userDomainMask).first?.path
                ?? (NSHomeDirectory() as NSString).appendingPathComponent(".Trash")
            result = freeName(for: step.path, in: trash)
            argv = privilegedTrashArgv(path: step.path, trashTo: result)
        }
        if let message = AdminAuthorization.run(argv, prompt: AdminAuthorization.prompt(for: step)) {
            throw RustyFinderError.PermissionDenied(message: message)
        }
        return result
    }

    private func freeName(for path: String, in dir: String) -> String {
        let name = (path as NSString).lastPathComponent
        var target = (dir as NSString).appendingPathComponent(name)
        var n = 1
        while FileManager.default.fileExists(atPath: target) {
            n += 1
            target = (dir as NSString).appendingPathComponent("\(name) \(n)")
        }
        return target
    }

    private func moveAside(_ path: String, into dir: String) throws -> String {
        let fm = FileManager.default
        let name = (path as NSString).lastPathComponent
        var target = (dir as NSString).appendingPathComponent(name)
        var n = 1
        while fm.fileExists(atPath: target) {
            n += 1
            target = (dir as NSString).appendingPathComponent("\(name) \(n)")
        }
        do {
            try fm.createDirectory(atPath: dir, withIntermediateDirectories: true)
            try fm.moveItem(atPath: path, toPath: target)
        } catch {
            throw RustyFinderError.Io(message: error.localizedDescription)
        }
        return target
    }
}

/// A question an activity is waiting on.
enum PendingPrompt {
    case conflict(ConflictRecord)
    /// `canElevate`: "Retry as Administrator" is offered.
    case error(path: String, message: String, canElevate: Bool)
    /// Replace was picked for a folder holding items the new one lacks:
    /// those would be deleted, so ask once more.
    case confirmFolderReplace(ConflictRecord, applyToAll: Bool)
}

/// The app's view of file operations: the activity list, the prompts they
/// raise, and what happens when one finishes.
@MainActor
final class ActivityCenter {
    static let shared = ActivityCenter()

    /// Receives callbacks on Rust worker threads and hops to the main thread.
    private final class Bridge: ActivityListener, @unchecked Sendable {
        func activitiesChanged(activities: [ActivityRecord]) {
            DispatchQueue.main.async { ActivityCenter.shared.update(activities) }
        }

        func conflictPrompt(activity: UInt64, conflict: ConflictRecord) {
            DispatchQueue.main.async { ActivityCenter.shared.enqueue(activity, .conflict(conflict)) }
        }

        func errorPrompt(activity: UInt64, path: String, message: String, canElevate: Bool) {
            DispatchQueue.main.async {
                ActivityCenter.shared.enqueue(activity, .error(path: path, message: message, canElevate: canElevate))
            }
        }

        func finished(report: FinishRecord) {
            DispatchQueue.main.async { ActivityCenter.shared.finished(report) }
        }
    }

    private(set) var activities: [ActivityRecord] = []
    /// Prompts waiting to be shown, oldest first. Only the first is on screen.
    private var prompts: [(activity: UInt64, prompt: PendingPrompt)] = []
    private var showing: (activity: UInt64, alert: NSAlert)?
    /// The most recent finish report (for scripted checks).
    private(set) var lastFinish: FinishRecord?

    private var core: RustyFinderApp { AppEnvironment.shared.core }

    /// Connect to the Rust core. Call once at launch.
    func start() {
        core.attach(listener: Bridge(), platform: TrashService())
    }

    /// Start a file operation on behalf of a window. `src` is the
    /// connection the source paths are on and `dst` the target's (0 is the
    /// local disk); operations on one file system pass it as both.
    func submit(_ op: FileOp, window: UInt64, src: UInt64 = 0, dst: UInt64 = 0) {
        _ = core.submitBetween(window: window, op: op, src: src, dst: dst)
    }

    private func update(_ list: [ActivityRecord]) {
        activities = list
        CompletionNotifier.seen(list)
        NotificationCenter.default.post(name: .rustyfinderActivitiesChanged, object: nil)
    }

    private func windowController(for windowId: UInt64) -> MainWindowController? {
        (NSApp.delegate as? AppDelegate)?.windowControllers.first { $0.session.id() == windowId }
    }

    /// The window an activity belongs to. The list may not have caught up
    /// with a brand-new activity yet, so ask the core as well.
    private func window(of activity: UInt64) -> MainWindowController? {
        let record = activities.first { $0.id == activity } ?? core.activities().first { $0.id == activity }
        return record.flatMap { windowController(for: $0.window) }
            ?? (NSApp.delegate as? AppDelegate)?.windowControllers.first
    }

    // MARK: Prompts

    var currentPrompt: PendingPrompt? { prompts.first?.prompt }

    private func enqueue(_ activity: UInt64, _ prompt: PendingPrompt) {
        prompts.append((activity, prompt))
        showNextPrompt()
    }

    private func showNextPrompt() {
        guard showing == nil, let next = prompts.first else { return }
        guard let window = window(of: next.activity)?.window else {
            // No window to ask in: stop rather than guess.
            answer(next.activity, with: nil)
            return
        }
        let alert: NSAlert
        switch next.prompt {
        case .conflict(let conflict): alert = PromptAlerts.conflict(conflict)
        case .error(let path, let message, let canElevate):
            alert = PromptAlerts.error(path: path, message: message, canElevate: canElevate)
        case .confirmFolderReplace(let conflict, _): alert = PromptAlerts.confirmFolderReplace(conflict)
        }
        showing = (next.activity, alert)
        alert.beginSheetModal(for: window) { [weak self] response in
            self?.answer(next.activity, with: response, applyToAll: alert.suppressionButton?.state == .on)
        }
    }

    /// Apply the response to the prompt on screen. A nil response (or one
    /// that maps to nothing) means Stop.
    private func answer(_ activity: UInt64, with response: NSApplication.ModalResponse?, applyToAll: Bool = false) {
        guard let index = prompts.firstIndex(where: { $0.activity == activity }) else { return }
        let prompt = prompts.remove(at: index).prompt
        showing = nil
        switch prompt {
        case .conflict(let conflict):
            let choice = response.flatMap { PromptAlerts.conflictChoice(for: $0, conflict: conflict) } ?? .stop
            if choice == .replace, !conflict.targetExtras.isEmpty {
                prompts.insert((activity, .confirmFolderReplace(conflict, applyToAll: applyToAll)), at: 0)
            } else {
                core.answerConflict(activity: activity, choice: choice, applyToAll: applyToAll)
            }
        case .confirmFolderReplace(let conflict, let all):
            if response == .alertSecondButtonReturn {
                core.answerConflict(activity: activity, choice: .replace, applyToAll: all)
            } else if response == nil {
                core.answerConflict(activity: activity, choice: .stop, applyToAll: false)
            } else {
                // Cancel: back to the choices.
                prompts.insert((activity, .conflict(conflict)), at: 0)
            }
        case .error(_, _, let canElevate):
            let choice = response.flatMap { PromptAlerts.errorChoice(for: $0, canElevate: canElevate) } ?? .stop
            core.answerError(activity: activity, choice: choice)
        }
        showNextPrompt()
    }

    // MARK: Finish

    private func finished(_ report: FinishRecord) {
        lastFinish = report
        CompletionNotifier.finished(report)
        RemoteFiles.shared.activityFinished(report)
        ArchiveCenter.shared.activityFinished(report)
        ToolCenter.shared.activityFinished(report)
        // A prompt that was still up for it is moot now.
        if let showing, showing.activity == report.id, let sheet = showing.alert.window.sheetParent {
            sheet.endSheet(showing.alert.window, returnCode: .abort)
        }
        // No success sound for anything stopped, skipped or failed. Undo
        // and Redo have their own.
        if report.result == .success, report.notable {
            let isUndo = report.title.hasPrefix("Undo ") || report.title.hasPrefix("Redo ")
            Sounds.play(isUndo ? Sounds.undo : Sounds.completion)
        }
        let controller = windowController(for: report.window)
        (NSApp.delegate as? AppDelegate)?.windowControllers.forEach { $0.reloadRemoteTabs() }
        if !report.results.isEmpty {
            controller?.reveal(report.results, renameAfter: report.renameAfter)
        }
        if let message = report.message, report.result == .failed || report.result == .partial {
            controller?.showOperationProblem(title: report.title, message: message, partial: report.result == .partial)
        }
        NotificationCenter.default.post(name: .rustyfinderActivitiesChanged, object: nil)
    }

    // MARK: Scripted checks

    /// Answer the prompt on screen by name: "replace", "skip", "keepboth",
    /// "older", "merge", "stop", "retry", "skipall". A trailing "!" ticks
    /// "Apply to all".
    func debugAnswer(_ text: String) {
        guard let showing else { return }
        let applyToAll = text.hasSuffix("!")
        let name = text.trimmingCharacters(in: CharacterSet(charactersIn: "!"))
        showing.alert.suppressionButton?.state = applyToAll ? .on : .off
        guard let index = showing.alert.buttons.firstIndex(where: {
            $0.title.lowercased().replacingOccurrences(of: " ", with: "") == name
        }) else { return }
        let response = NSApplication.ModalResponse(rawValue: NSApplication.ModalResponse.alertFirstButtonReturn.rawValue + index)
        showing.alert.window.sheetParent?.endSheet(showing.alert.window, returnCode: response)
    }

    func debugDump() -> [String] {
        var prompt = "-"
        switch currentPrompt {
        case .conflict(let c):
            prompt = "conflict \(c.kind) \((c.targetPath as NSString).lastPathComponent)"
            if c.resumable { prompt += " resumable" }
            if !c.targetExtras.isEmpty { prompt += " extras=\(c.targetExtras.joined(separator: ","))" }
        case .error(let path, _, let canElevate):
            prompt = "error \((path as NSString).lastPathComponent)\(canElevate ? " admin" : "")"
        case .confirmFolderReplace(let c, _): prompt = "confirm-replace \((c.targetPath as NSString).lastPathComponent)"
        case nil: break
        }
        let log = core.operationLog().last.map { "\($0.title) [\($0.result)]" } ?? "-"
        return [
            "activities: \(activities.map { "\($0.title) [\($0.phase)]" }.joined(separator: "|"))",
            "prompt: \(prompt)",
            "prompt.buttons: \(showing?.alert.buttons.map(\.title).joined(separator: "|") ?? "-")",
            "log.last: \(log)",
            "finish.last: \(lastFinish.map { "\($0.title) [\($0.result)] \($0.message ?? "")" } ?? "-")",
        ]
    }
}

/// Builds the conflict and error sheets and maps their buttons to choices.
@MainActor
enum PromptAlerts {
    /// Button order for a conflict of the given kind. Skip is first, so
    /// Return picks the choice that changes nothing; Stop is last and also
    /// answers Esc.
    static func conflictChoices(_ conflict: ConflictRecord) -> [(String, ConflictOption)] {
        let kind = conflict.kind
        var choices: [(String, ConflictOption)] = [("Skip", .skip)]
        // A partial file a stopped transfer left: carrying on is offered.
        if conflict.resumable { choices.append(("Resume", .resume)) }
        choices.append(("Replace", .replace))
        if kind == .folderFolder { choices.append(("Merge", .merge)) }
        if kind != .mixed { choices.append(("Replace Older", .replaceOlder)) }
        choices += [("Keep Both", .keepBoth), ("Stop", .stop)]
        return choices
    }

    static func conflictChoice(for response: NSApplication.ModalResponse, conflict: ConflictRecord) -> ConflictOption? {
        let index = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
        return conflictChoices(conflict)[safe: index]?.1
    }

    /// "Replace" on a folder that holds items the new folder lacks.
    static func confirmFolderReplace(_ conflict: ConflictRecord) -> NSAlert {
        let alert = NSAlert()
        alert.alertStyle = .critical
        let name = RowPresenter.shared.displayName(conflict.target)
        let count = conflict.targetExtras.count
        alert.messageText = "Replace the folder “\(name)”?"
        let shown = conflict.targetExtras.prefix(5).map { "“\($0)”" }.joined(separator: ", ")
        let more = count > 5 ? " and \(count - 5) more" : ""
        alert.informativeText = "It holds \(count == 1 ? "an item" : "\(count) items") the new folder doesn’t have "
            + "(\(shown)\(more)). Replacing deletes \(count == 1 ? "it" : "them")."
        alert.addButton(withTitle: "Cancel")
        let replace = alert.addButton(withTitle: "Replace")
        replace.hasDestructiveAction = true
        return alert
    }

    /// A permission problem on this Mac also offers to retry as an
    /// administrator, through the system's prompt (file-ops §1).
    private static func errorChoices(canElevate: Bool) -> [(String, ErrorOption)] {
        (canElevate ? [("Retry as Administrator…", ErrorOption.retryAsAdmin)] : [])
            + [("Retry", .retry), ("Skip", .skip), ("Skip All", .skipAll), ("Stop", .stop)]
    }

    static func errorChoice(for response: NSApplication.ModalResponse, canElevate: Bool) -> ErrorOption? {
        let index = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
        return errorChoices(canElevate: canElevate)[safe: index]?.1
    }

    static func conflict(_ conflict: ConflictRecord) -> NSAlert {
        let alert = NSAlert()
        let name = RowPresenter.shared.displayName(conflict.target)
        let folder = ((conflict.targetPath as NSString).deletingLastPathComponent).folderDisplayName
        let what = conflict.target.isDir ? "A folder" : "An item"
        alert.messageText = "\(what) named “\(name)” already exists in “\(folder)”."
        if conflict.resumable {
            alert.informativeText = "It is what a stopped transfer of this item left. Resume adds the rest; "
                + "Replace copies it all again."
        } else {
            alert.informativeText = conflict.kind == .folderFolder
                ? "Merge combines both folders. Replace removes the existing folder and everything in it."
                : "Replace overwrites the existing item."
        }
        for (title, choice) in conflictChoices(conflict) {
            let button = alert.addButton(withTitle: title)
            if choice == .stop { button.keyEquivalent = "\u{1b}" }
            if choice == .replace { button.hasDestructiveAction = true }
        }
        alert.showsSuppressionButton = true
        alert.suppressionButton?.title = "Apply to all"
        alert.accessoryView = comparison(conflict)
        return alert
    }

    static func error(path: String, message: String, canElevate: Bool) -> NSAlert {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "“\((path as NSString).lastPathComponent)” couldn’t be processed."
        alert.informativeText = canElevate
            ? message + "\n\nAn administrator can do this: macOS will ask for an administrator’s name and password."
            : message
        for (title, choice) in errorChoices(canElevate: canElevate) {
            let button = alert.addButton(withTitle: title)
            if choice == .stop { button.keyEquivalent = "\u{1b}" }
        }
        return alert
    }

    /// Both items side by side: icon, size, date, and which one is newer.
    private static func comparison(_ conflict: ConflictRecord) -> NSView {
        let sourceNewer: Bool? = {
            guard let s = conflict.source.modifiedMs, let t = conflict.target.modifiedMs, s != t else { return nil }
            return s > t
        }()
        let rows = NSStackView(views: [
            line("Existing", conflict.target, conflict.targetPath, newer: sourceNewer == false),
            line("New", conflict.source, conflict.sourcePath, newer: sourceNewer == true),
        ])
        rows.orientation = .vertical
        rows.alignment = .leading
        rows.spacing = 6
        rows.frame = NSRect(x: 0, y: 0, width: 320, height: 76)
        return rows
    }

    private static func line(_ label: String, _ row: Row, _ path: String, newer: Bool) -> NSView {
        let presenter = RowPresenter.shared
        let icon = NSImageView(image: presenter.icon(row, path: path))
        icon.widthAnchor.constraint(equalToConstant: 32).isActive = true
        icon.heightAnchor.constraint(equalToConstant: 32).isActive = true
        var detail = presenter.date(row.modifiedMs)
        if !row.isDir { detail = "\(presenter.size(row)), \(detail)" }
        let title = NSTextField(labelWithString: newer ? "\(label) (newer)" : label)
        title.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: newer ? .semibold : .regular)
        let info = NSTextField(labelWithString: detail)
        info.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        info.textColor = .secondaryLabelColor
        let text = NSStackView(views: [title, info])
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 1
        let line = NSStackView(views: [icon, text])
        line.orientation = .horizontal
        line.spacing = 8
        return line
    }
}
