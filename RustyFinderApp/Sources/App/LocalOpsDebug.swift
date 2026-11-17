import AppKit
import Quartz

/// Script tokens and dump lines for the local file features (App Deleter,
/// aliases, editors, iCloud, unlock and authorization prompts). Kept apart
/// from DebugSnapshot so that file only needs one hook.
///
///   `ad:<what>`              App Deleter window (see its `debugSet`)
///   `actcopy:<src>><dir>`    start a copy (for queue checks)
///   `actmove:<from>><to>`    move a queued activity, as a drag does
///   `opt:on|off`             treat Option as held (toolbar Delete)
///   `toolbar:<label>`        click a toolbar button by its label
///   `banner:extra`           press the extra button of the inline error
///   `cloud:download|evict`   Download Now / Remove Download on the selection
@MainActor
enum LocalOpsDebug {
    /// Option held, for the toolbar's Delete button.
    private(set) static var optionHeld = false

    private static var window: MainWindowController? {
        let key = NSApp.keyWindow?.windowController as? MainWindowController
        return key ?? NSApp.windows.lazy.compactMap { $0.windowController as? MainWindowController }.first
    }

    /// Handle `token` if it is one of ours.
    static func handle(_ token: String) -> Bool {
        let parts = token.split(separator: ":", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { return false }
        let value = parts[1]
        switch parts[0] {
        case "ad":
            AppDeleterWindowController.current?.debugSet(value)
        case "actcopy":
            let ends = value.components(separatedBy: ">")
            guard ends.count == 2, let window else { return true }
            ActivityCenter.shared.submit(
                .transfer(mode: .copy, sources: [ends[0]], destDir: ends[1], renameTo: nil), window: window.session.id())
        case "actmove":
            let ends = value.split(separator: ">").compactMap { Int($0) }
            if ends.count == 2 { ActivitiesViewController.debugMoveQueued(from: ends[0], to: ends[1]) }
        case "opt":
            optionHeld = value == "on"
        case "toolbar":
            guard let item = window?.window?.toolbar?.items.first(where: { $0.label.lowercased() == value.lowercased() }),
                  let action = item.action else { return true }
            NSApp.sendAction(action, to: item.target, from: item)
        case "banner":
            window?.debugActiveList?.banner.debugPressExtra()
        case "cloud":
            let list = window?.debugActiveList
            if value == "download" { list?.downloadNow(nil) } else { list?.removeDownload(nil) }
        default:
            return false
        }
        return true
    }

    /// What Quick Look would be handed for the first selected item.
    private static var quickLookFirst: String {
        guard let list = window?.debugActiveList, !list.selection.isEmpty,
              let item = list.previewPanel(nil, previewItemAt: 0) else { return "-" }
        return item.previewItemURL.map { "file \($0.lastPathComponent)" } ?? "placeholder \((item.previewItemTitle ?? nil) ?? "")"
    }

    static func dumpLines() -> [String] {
        [
            "appdeleter: \(AppDeleterWindowController.current?.debugDump() ?? "-")",
            "appdeleter.confirm: \(AppDeleterWindowController.current?.confirmation?.messageText ?? "-")",
            "sound.last: \(Sounds.last)",
            "comment.scripted: \(FinderComments.scriptedSummary)",
            "preflight: \(LocalPreflight.lastAlert?.messageText ?? "-")",
            "cloud.last: \(CloudItems.lastEvent) problem=\(CloudItems.lastProblem)",
            "eject.last: \(VolumeEject.lastEvent)",
            "notify.last: \(CompletionNotifier.last)",
            "help: \(HelpWindowController.current?.debugSummary ?? "-")",
            "toolbar: \(window?.window?.toolbar?.items.map(\.label).filter { !$0.isEmpty }.joined(separator: "|") ?? "-")",
            "cloud.badges: \(window?.debugActiveList?.debugCloudBadges.joined(separator: "|") ?? "-")",
            "ql.first: \(quickLookFirst)",
        ]
    }
}
