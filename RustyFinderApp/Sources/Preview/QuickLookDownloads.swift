import AppKit
import Quartz

/// Quick Look for items on a server or in an archive: while an item's
/// local copy is on its way, the panel shows a small page with the
/// progress. Also the slideshow's timer.
@MainActor
enum QuickLookDownloads {
    private static let folder = URL(fileURLWithPath: NSTemporaryDirectory())
        .appendingPathComponent("RustyFinder-ql-\(ProcessInfo.processInfo.processIdentifier)")
    private static var pages: [PreviewCache.Key: URL] = [:]
    private static var observing = false

    /// The progress page for an item still downloading.
    static func placeholder(for item: InspectedItem) -> URL {
        let key = PreviewCache.key(item)
        observe()
        let url = pages[key] ?? {
            let url = folder.appendingPathComponent(UUID().uuidString).appendingPathComponent("Downloading.txt")
            pages[key] = url
            return url
        }()
        write(item: item, to: url)
        return url
    }

    private static func write(item: InspectedItem, to url: URL) {
        let done = PreviewCache.shared.progress[PreviewCache.key(item)] ?? 0
        let name = (item.path as NSString).lastPathComponent
        let text = "Downloading “\(name)”…\n\n\(Int(done * 100)) % of \(RowPresenter.shared.bytes(item.row.size))\n"
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? text.write(to: url, atomically: true, encoding: .utf8)
    }

    /// Progress moves the page on; an arrived copy replaces it.
    private static func observe() {
        guard !observing else { return }
        observing = true
        NotificationCenter.default.addObserver(forName: .rustyfinderPreviewFetchProgress, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated {
                guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.isVisible,
                      let shown = panel.currentPreviewItem as? NSURL, shown.lastPathComponent == "Downloading.txt"
                else { return }
                panel.refreshCurrentPreviewItem()
            }
        }
    }

    static func cleanUp() {
        try? FileManager.default.removeItem(at: folder)
    }

    // MARK: Slideshow

    /// Seconds per slide.
    static let slideSeconds: TimeInterval = 3
    private static var timer: Timer?
    private(set) static var slideshowRunning = false

    /// The list whose selection the slideshow walks through.
    private static weak var source: FileListViewController?

    static func startSlideshow(from list: FileListViewController) {
        source = list
        timer?.invalidate()
        slideshowRunning = true
        timer = Timer.scheduledTimer(withTimeInterval: slideSeconds, repeats: true) { _ in
            MainActor.assumeIsolated { advance() }
        }
    }

    static func stopSlideshow() {
        timer?.invalidate()
        timer = nil
        slideshowRunning = false
    }

    /// Next slide, back to the first after the last. The show ends when
    /// the panel is closed.
    static func advance() {
        guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.isVisible else {
            stopSlideshow()
            return
        }
        let count = source?.previewURLs.count ?? 0
        guard count > 1 else { return }
        panel.currentPreviewItemIndex = (panel.currentPreviewItemIndex + 1) % count
    }

    /// "index/count" of the panel (scripted checks).
    static var debugPosition: String {
        guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.isVisible else {
            return "-"
        }
        let count = source?.previewURLs.count ?? panel.dataSource?.numberOfPreviewItems(in: panel) ?? 0
        let name = (panel.currentPreviewItem as? NSURL)?.lastPathComponent ?? "-"
        return "\(panel.currentPreviewItemIndex)/\(count) \(name)"
    }
}
