import AppKit
import Quartz

/// Quick Look for the file list: Space or Cmd-Y opens the system panel on
/// the selection, and the arrow keys keep working in the list underneath.
extension FileListViewController: QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    /// URLs the panel shows: the selection, with symlinks resolved so a
    /// link previews its target. Items on a server or in an archive show
    /// their local copy, or a progress page until it has arrived.
    var previewURLs: [URL] {
        selection.compactMap { index -> URL? in
            guard let path = content.path(at: index), let row = content.row(at: index) else { return nil }
            if isRemote {
                let item = InspectedItem(path: path, row: row, conn: conn)
                if let local = PreviewCache.shared.local(for: item) { return URL(fileURLWithPath: local) }
                return row.isDir ? nil : QuickLookDownloads.placeholder(for: item)
            }
            let url = URL(fileURLWithPath: path)
            return row.kind == .symlink && !row.isBrokenLink ? url.resolvingSymlinksInPath() : url
        }
    }

    var isQuickLookVisible: Bool {
        QLPreviewPanel.sharedPreviewPanelExists() && QLPreviewPanel.shared().isVisible
    }

    @objc func toggleQuickLook(_ sender: Any?) {
        guard let panel = QLPreviewPanel.shared() else { return }
        if isQuickLookVisible {
            panel.orderOut(nil)
            QuickLookDownloads.stopSlideshow()
        } else if selection.count > 0 {
            fetchForQuickLook()
            panel.makeKeyAndOrderFront(nil)
        }
    }

    /// Items elsewhere are copied here first (whatever their size: the user
    /// asked to see them); the panel follows as each one arrives.
    func fetchForQuickLook() {
        guard isRemote else { return }
        for index in selection {
            guard let path = content.path(at: index), let row = content.row(at: index), !row.isDir else { continue }
            let item = InspectedItem(path: path, row: row, conn: conn)
            guard PreviewCache.shared.local(for: item) == nil else { continue }
            PreviewCache.shared.fetch(item) { _ in
                guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.isVisible
                else { return }
                panel.reloadData()
            }
        }
    }

    /// Quick Look full screen, stepping through the selection by itself.
    @objc func quickLookSlideshow(_ sender: Any?) {
        guard let panel = QLPreviewPanel.shared(), selection.count > 0 else { return }
        fetchForQuickLook()
        if !panel.isVisible { panel.makeKeyAndOrderFront(nil) }
        if !panel.isInFullScreenMode { panel.enterFullScreenMode(nil, withOptions: nil) }
        // The slides move on by themselves.
        QuickLookDownloads.startSlideshow(from: self)
    }

    /// Keep an open panel in step with the selection.
    func quickLookSelectionChanged() {
        guard isQuickLookVisible, let panel = QLPreviewPanel.shared(),
              panel.dataSource === self
        else { return }
        fetchForQuickLook()
        panel.reloadData()
    }

    // MARK: Panel control (found through the responder chain)

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool {
        true
    }

    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        panel.dataSource = self
        panel.delegate = self
    }

    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {
        panel.dataSource = nil
        panel.delegate = nil
    }

    // MARK: Data source

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int {
        previewURLs.count
    }

    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! {
        // A cloud-only item is never downloaded just to be looked at (cloud.md §2).
        if let row = Array(selection)[safe: index].flatMap({ content.row(at: $0) }), !isRemote, CloudItems.isCloudOnly(row) {
            return CloudPlaceholderItem(title: row.name)
        }
        return previewURLs[safe: index] as NSURL?
    }

    // MARK: Delegate

    /// Arrow keys (and friends) pressed in the panel move the selection in
    /// the list, so the user can walk through a folder with the panel open.
    func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard event.type == .keyDown else { return false }
        let navigation: Set<UInt16> = [123, 124, 125, 126, 115, 119, 116, 121]  // arrows, home/end, page up/down
        guard navigation.contains(event.keyCode) else { return false }
        primaryView.keyDown(with: event)
        return true
    }

    /// Where the panel zooms from and back to: the selected row's icon.
    func previewPanel(_ panel: QLPreviewPanel!, sourceFrameOnScreenFor item: QLPreviewItem!) -> NSRect {
        // Only the list has a simple row to zoom from.
        guard layout != .icons, let row = selection.first, let window = view.window else { return .zero }
        var rect = tableView.rect(ofRow: content.tableRow(ofItem: row))
        rect.size.width = min(rect.width, 28)
        let visible = tableView.visibleRect
        guard visible.intersects(rect) else { return .zero }
        return window.convertToScreen(tableView.convert(rect, to: nil))
    }
}

/// What Quick Look shows for an item whose content is only in iCloud: its
/// name, without a file to read.
final class CloudPlaceholderItem: NSObject, QLPreviewItem {
    private let title: String

    init(title: String) {
        self.title = title
    }

    var previewItemURL: URL? { nil }
    var previewItemTitle: String? { "\(title) (in iCloud, not downloaded)" }
}
