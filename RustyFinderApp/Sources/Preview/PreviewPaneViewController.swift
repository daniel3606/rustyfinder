import AppKit
import PDFKit
import Quartz

/// The pane at the right edge of the window. Three modes: a preview of the
/// selection with its Info below, the activity list, and the operation log.
final class PreviewPaneViewController: NSViewController {
    enum Mode: Int {
        case preview, activities, log
    }

    var onNavigate: ((String) -> Void)? {
        didSet { info.onNavigate = onNavigate }
    }
    var windowId: UInt64 = 0 {
        didSet {
            info.windowId = windowId
            activities.windowId = windowId
        }
    }

    private let modeControl = NSSegmentedControl(
        labels: ["Preview", "Activities", "Log"], trackingMode: .selectOne, target: nil, action: nil)
    private let holder = NSView()
    private let split = NSSplitView()
    private let previewHolder = NSView()
    private let info = InfoViewController()
    private let activities = ActivitiesViewController()
    private let log = LogViewController()

    /// Made on first use. A Quick Look view that was never shown must not
    /// be closed (it aborts), so teardown has to know if one exists.
    private var quickLookView: QLPreviewView?
    private var quickLook: QLPreviewView? {
        if quickLookView == nil { quickLookView = QLPreviewView(frame: .zero, style: .normal) }
        return quickLookView
    }
    private let textPreview = TextPreviewView()
    /// PDFs get a view of their own, with display options that are remembered.
    private lazy var pdfView = PDFDisplayOptions.makeView()
    private let iconView = NSImageView()
    private let summary = NSTextField(labelWithString: "")
    private var shown: [InspectedItem] = []
    private var pending: DispatchWorkItem?
    private(set) var mode: Mode = .preview
    /// What the preview area currently displays (for scripted checks).
    private(set) var previewKind = "none"

    override func loadView() {
        let root = NSView()
        // A download finished or a local copy was removed: show it.
        NotificationCenter.default.addObserver(forName: .rustyfinderCloudStateChanged, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                if self?.mode == .preview { self?.rebuildPreview() }
            }
        }
        modeControl.target = self
        modeControl.action = #selector(modeChanged(_:))
        modeControl.selectedSegment = 0
        modeControl.controlSize = .small
        modeControl.translatesAutoresizingMaskIntoConstraints = false
        holder.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(modeControl)
        root.addSubview(holder)
        NSLayoutConstraint.activate([
            modeControl.topAnchor.constraint(equalTo: root.safeAreaLayoutGuide.topAnchor, constant: 8),
            modeControl.centerXAnchor.constraint(equalTo: root.centerXAnchor),
            holder.topAnchor.constraint(equalTo: modeControl.bottomAnchor, constant: 8),
            holder.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            holder.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            holder.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            root.widthAnchor.constraint(greaterThanOrEqualToConstant: 240),
        ])

        // Preview above, Info below, with a divider whose position is kept.
        split.isVertical = false
        split.dividerStyle = .thin
        split.autosaveName = "RustyFinderPreviewInfoSplit"
        addChild(info)
        split.addArrangedSubview(previewHolder)
        split.addArrangedSubview(info.view)

        iconView.imageScaling = .scaleProportionallyUpOrDown
        summary.alignment = .center
        summary.textColor = .secondaryLabelColor
        summary.font = .systemFont(ofSize: NSFont.smallSystemFontSize)

        addChild(activities)
        activities.fixedWidth = false
        addChild(log)
        view = root
        show(mode: .preview)
        NotificationCenter.default.addObserver(
            self, selector: #selector(fetchProgressed(_:)), name: .rustyfinderPreviewFetchProgress, object: nil)
        PDFDisplayOptions.observe()
    }

    /// The split starts with no sizes of its own, which would leave the
    /// preview with no height at all. Give it the upper 40 % the first
    /// time there is room, and never let it be dragged shut.
    private var dividerPlaced = false

    override func viewDidLayout() {
        super.viewDidLayout()
        guard mode == .preview, split.superview != nil, split.bounds.height > 200 else { return }
        let tooSmall = previewHolder.frame.height < 120
        if !dividerPlaced || tooSmall {
            dividerPlaced = true
            if tooSmall { split.setPosition(max(160, split.bounds.height * 0.4), ofDividerAt: 0) }
        }
    }

    // MARK: Modes

    @objc private func modeChanged(_ sender: NSSegmentedControl) {
        show(mode: Mode(rawValue: sender.selectedSegment) ?? .preview)
    }

    func show(mode: Mode) {
        loadViewIfNeeded()
        self.mode = mode
        modeControl.selectedSegment = mode.rawValue
        holder.subviews.forEach { $0.removeFromSuperview() }
        let content: NSView
        switch mode {
        case .preview: content = split
        case .activities: content = activities.view
        case .log:
            content = log.view
            log.reload()
        }
        embed(content, in: holder)
        // Coming back to Preview must show the selection again, not a blank.
        if mode == .preview { rebuildPreview() }
    }

    private func embed(_ child: NSView, in parent: NSView) {
        child.translatesAutoresizingMaskIntoConstraints = false
        parent.addSubview(child)
        NSLayoutConstraint.activate([
            child.topAnchor.constraint(equalTo: parent.topAnchor),
            child.bottomAnchor.constraint(equalTo: parent.bottomAnchor),
            child.leadingAnchor.constraint(equalTo: parent.leadingAnchor),
            child.trailingAnchor.constraint(equalTo: parent.trailingAnchor),
        ])
    }

    // MARK: Selection

    /// Follow the active pane's selection. While the selection is moving
    /// fast (arrow keys held), wait briefly so expensive previews are not
    /// built for items that were only passed over.
    func setSelection(_ items: [InspectedItem], immediately: Bool = false) {
        pending?.cancel()
        guard items != shown else {
            info.refresh()
            return
        }
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.shown = items
            self.info.show(items)
            if self.mode == .preview { self.rebuildPreview() }
        }
        pending = work
        if immediately {
            work.perform()
        } else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
        }
    }

    private func rebuildPreview() {
        previewHolder.subviews.forEach { $0.removeFromSuperview() }
        quickLook?.previewItem = nil
        previewSource = "file"
        downloadLabel = nil
        guard let item = shown.first else {
            previewKind = "none"
            return
        }
        if shown.count > 1 {
            // Several items: a stacked icon and a count.
            showIcon(NSImage(systemSymbolName: "square.stack.3d.up", accessibilityDescription: "Several items"),
                     text: "\(shown.count.formatted()) items")
            previewKind = "several"
            return
        }
        if !item.isLocal {
            rebuildElsewhere(item)
            return
        }
        textPreview.isEditable = true
        textPreview.onSaved = nil
        // Links preview what they point at.
        let url = item.row.kind == .symlink && !item.row.isBrokenLink ? item.url.resolvingSymlinksInPath() : item.url
        showLocal(url, item: item)
    }

    /// Preview a file on this Mac (or a local copy of one elsewhere).
    private func showLocal(_ url: URL, item: InspectedItem) {
        if item.row.isDir, !NSWorkspace.shared.isFilePackage(atPath: url.path) {
            showIcon(RowPresenter.shared.icon(item.row, path: item.path), text: nil)
            previewKind = "folder"
        } else if !item.row.isDir, CloudItems.isCloudOnly(item.row) {
            // Never download just to preview (cloud.md CL-07).
            embed(CloudItems.placeholder(for: item) { [weak self] in self?.view.window }, in: previewHolder)
            previewKind = "cloud"
        } else if !item.row.isDir, url.pathExtension.lowercased() == "pdf", PDFDisplayOptions.show(url, in: pdfView) {
            embed(pdfView, in: previewHolder)
            previewKind = "pdf"
        } else if !item.row.isDir, TextPreviewView.canShow(url, size: item.row.size), textPreview.load(url) {
            embed(textPreview, in: previewHolder)
            previewKind = "text"
        } else if let quickLook {
            quickLook.previewItem = url as NSURL
            embed(quickLook, in: previewHolder)
            previewKind = "quicklook"
        }
    }

    // MARK: Items on a server or in an archive

    /// Download button or progress for the item on show, if it is elsewhere.
    private var downloadLabel: NSTextField?

    /// Items elsewhere are previewed from a local copy. Small ones are
    /// fetched at once; big ones on a server wait for the Download button.
    private func rebuildElsewhere(_ item: InspectedItem) {
        let cache = PreviewCache.shared
        if item.row.isDir {
            showIcon(RowPresenter.shared.icon(item.row, path: nil), text: nil)
            previewKind = "folder"
            return
        }
        if let local = cache.local(for: item), FileManager.default.fileExists(atPath: local) {
            let archived = ArchiveCenter.shared.isArchive(item.conn)
            // Quick edits of a server's file go back by upload; an archive
            // cannot be changed.
            textPreview.isEditable = !archived
            textPreview.onSaved = archived ? nil : { [weak self] url in
                guard let self else { return }
                _ = AppEnvironment.shared.core.submitBetween(
                    window: self.windowId, op: .put(source: url.path, target: item.path), src: 0, dst: item.conn)
            }
            showLocal(URL(fileURLWithPath: local), item: item)
            previewSource = "copy"
            return
        }
        let icon = RowPresenter.shared.icon(item.row, path: nil)
        if PreviewCache.needsAsking(item), !cache.isFetching(item) {
            let size = RowPresenter.shared.bytes(item.row.size)
            showIcon(icon, text: "\(size), not downloaded")
            let download = NSButton(
                image: NSImage(systemSymbolName: "icloud.and.arrow.down", accessibilityDescription: "Download")!,
                target: self, action: #selector(downloadClicked(_:)))
            download.title = "Download"
            download.imagePosition = .imageLeading
            download.bezelStyle = .push
            download.setAccessibilityLabel("Download to preview")
            download.translatesAutoresizingMaskIntoConstraints = false
            previewHolder.addSubview(download)
            NSLayoutConstraint.activate([
                download.centerXAnchor.constraint(equalTo: previewHolder.centerXAnchor),
                download.topAnchor.constraint(equalTo: summary.bottomAnchor, constant: 10),
            ])
            previewKind = "download"
            return
        }
        showIcon(icon, text: downloadingText(item))
        downloadLabel = summary
        previewKind = "downloading"
        cache.fetch(item) { [weak self] _ in
            // Once, however many asked for this copy.
            guard let self, self.shown == [item], self.mode == .preview, self.previewKind == "downloading" else { return }
            self.rebuildPreview()
            // The Info's More Info comes from the copy too.
            self.info.refresh()
        }
    }

    private func downloadingText(_ item: InspectedItem) -> String {
        let done = PreviewCache.shared.progress[PreviewCache.key(item)] ?? 0
        return "Downloading… \(Int(done * 100)) %"
    }

    @objc private func fetchProgressed(_ notification: Notification) {
        guard previewKind == "downloading", let item = shown.first, shown.count == 1 else { return }
        downloadLabel?.stringValue = downloadingText(item)
    }

    @objc private func downloadClicked(_ sender: Any?) {
        guard let item = shown.first, shown.count == 1, !item.isLocal else { return }
        // Starts the download; the preview shows its progress and follows
        // when it lands.
        PreviewCache.shared.fetch(item) { _ in }
        rebuildPreview()
    }

    /// Refresh (Cmd-R) on a server: fetch the previewed item again.
    func refreshRemote() {
        guard shown.count == 1, let item = shown.first, !item.isLocal, !ArchiveCenter.shared.isArchive(item.conn)
        else { return }
        PreviewCache.shared.forget(item)
        if mode == .preview { rebuildPreview() }
    }

    /// Where the preview came from (scripted checks): the file, or a copy.
    private var previewSource = "file"

    private func showIcon(_ image: NSImage?, text: String?) {
        iconView.image = image
        summary.stringValue = text ?? ""
        summary.isHidden = text == nil
        let column = NSStackView(views: [iconView, summary])
        column.orientation = .vertical
        column.spacing = 8
        column.translatesAutoresizingMaskIntoConstraints = false
        iconView.widthAnchor.constraint(lessThanOrEqualToConstant: 128).isActive = true
        iconView.heightAnchor.constraint(lessThanOrEqualToConstant: 128).isActive = true
        previewHolder.addSubview(column)
        NSLayoutConstraint.activate([
            column.centerXAnchor.constraint(equalTo: previewHolder.centerXAnchor),
            column.centerYAnchor.constraint(equalTo: previewHolder.centerYAnchor),
        ])
    }

    /// The Quick Look view must be closed before it goes away.
    func tearDown() {
        guard let view = quickLookView else { return }
        quickLookView = nil
        // Only a view that has been on screen has anything to close.
        if view.superview != nil {
            view.previewItem = nil
            view.close()
        }
    }

    // MARK: Scripted checks

    func debugDump() -> [String] {
        var lines = [
            "preview.mode: \(mode)",
            "preview.kind: \(previewKind)",
            "preview.items: \(shown.map { ($0.path as NSString).lastPathComponent }.joined(separator: "|"))",
            "preview.source: \(previewSource) \(PreviewCache.shared.lastEvent)",
        ]
        if previewKind == "pdf" { lines.append("preview.pdf: \(PDFDisplayOptions.describe(pdfView))") }
        if previewKind == "download" || previewKind == "downloading" { lines.append("preview.note: \(summary.stringValue)") }
        if previewKind == "text" {
            lines.append("preview.text: \(textPreview.debugText.replacingOccurrences(of: "\n", with: "\\n").prefix(120))")
            lines.append("preview.encoding: \(TextEncodings.displayName(textPreview.encoding))")
        }
        if mode == .log { lines.append("preview.log: \(log.debugText.replacingOccurrences(of: "\n", with: " / ").suffix(160))") }
        lines += info.debugDump()
        return lines
    }

    var debugTextPreview: TextPreviewView { textPreview }
    func debugDownload() { downloadClicked(nil) }
    /// Pick a PDF display mode and zoom as the context menu would.
    func debugPDF(mode: Int, scale: Double?) {
        if let display = PDFDisplayMode(rawValue: mode) { pdfView.displayMode = display }
        if let scale {
            pdfView.autoScales = false
            pdfView.scaleFactor = CGFloat(scale)
        }
        PDFDisplayOptions.remember(pdfView)
    }
    var debugInfo: InfoViewController { info }
}

/// The operation log: one entry per finished operation, newest last.
/// Plain selectable text, so a failed path can be copied.
final class LogViewController: NSViewController {
    private let textView = NSTextView()
    private let formatter: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .short
        f.timeStyle = .medium
        return f
    }()

    override func loadView() {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        textView.isEditable = false
        textView.isSelectable = true
        textView.drawsBackground = false
        textView.textContainerInset = NSSize(width: 8, height: 8)
        textView.isVerticallyResizable = true
        textView.autoresizingMask = [.width]
        textView.setAccessibilityLabel("Operation log")
        scroll.documentView = textView
        view = scroll
        NotificationCenter.default.addObserver(self, selector: #selector(reload), name: .rustyfinderActivitiesChanged, object: nil)
    }

    @objc func reload() {
        guard isViewLoaded, view.superview != nil else { return }
        let text = NSMutableAttributedString()
        let small = NSFont.systemFont(ofSize: NSFont.smallSystemFontSize)
        for entry in AppEnvironment.shared.core.operationLog() {
            let when = formatter.string(from: Date(timeIntervalSince1970: Double(entry.timeMs) / 1000))
            let outcome: String
            let color: NSColor
            switch entry.result {
            case .success: (outcome, color) = ("", .labelColor)
            case .partial: (outcome, color) = (" (some items failed)", .systemOrange)
            case .failed: (outcome, color) = (" (failed)", .systemRed)
            case .stopped: (outcome, color) = (" (stopped)", .secondaryLabelColor)
            }
            text.append(NSAttributedString(string: "\(when)  ", attributes: [.font: small, .foregroundColor: NSColor.secondaryLabelColor]))
            text.append(NSAttributedString(string: "\(entry.title)\(outcome)\n", attributes: [.font: small, .foregroundColor: color]))
            for line in entry.lines {
                text.append(NSAttributedString(string: "    \(line)\n", attributes: [.font: small, .foregroundColor: NSColor.secondaryLabelColor]))
            }
        }
        if text.length == 0 {
            text.append(NSAttributedString(string: "Nothing has been logged yet.", attributes: [.font: small, .foregroundColor: NSColor.secondaryLabelColor]))
        }
        textView.textStorage?.setAttributedString(text)
        textView.scrollToEndOfDocument(nil)
    }

    var debugText: String { textView.string }
}

/// Get Info: the Info form in a window of its own, one per item.
final class InfoWindowController: NSWindowController, NSWindowDelegate {
    /// Open windows by path, so asking again for an item brings its window
    /// forward instead of opening a second one.
    private static var open: [String: InfoWindowController] = [:]
    /// More items than this share one combined window.
    static let maxWindows = 10

    private let info = InfoViewController()
    private let key: String

    static func show(_ items: [InspectedItem], windowId: UInt64, onNavigate: @escaping (String) -> Void) {
        guard !items.isEmpty else { return }
        let groups: [[InspectedItem]] = items.count > maxWindows ? [items] : items.map { [$0] }
        for (offset, group) in groups.enumerated() {
            let key = group.map { "\($0.conn):\($0.path)" }.joined(separator: "\n")
            if let existing = open[key] {
                existing.info.refresh()
                existing.window?.makeKeyAndOrderFront(nil)
                continue
            }
            let controller = InfoWindowController(items: group, key: key, windowId: windowId, onNavigate: onNavigate)
            open[key] = controller
            if let window = controller.window {
                window.cascadeTopLeft(from: NSPoint(x: 120 + offset * 24, y: 700 - offset * 24))
            }
            controller.showWindow(nil)
            // Nothing is being edited yet: the form may follow changes.
            controller.window?.makeFirstResponder(nil)
        }
    }

    static var openCount: Int { open.count }
    static var debugControllers: [InfoWindowController] { Array(open.values) }
    var debugInfo: InfoViewController { info }

    private init(items: [InspectedItem], key: String, windowId: UInt64, onNavigate: @escaping (String) -> Void) {
        self.key = key
        let window = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 340, height: 560),
            styleMask: [.titled, .closable, .resizable, .utilityWindow], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.title = items.count == 1
            ? "\(RowPresenter.shared.displayName(items[0].row)) Info"
            : "\(items.count.formatted()) Items Info"
        window.minSize = NSSize(width: 300, height: 240)
        super.init(window: window)
        info.windowId = windowId
        info.onNavigate = onNavigate
        window.contentViewController = info
        window.setContentSize(NSSize(width: 340, height: 560))
        window.delegate = self
        info.show(items)
        // Stay live: re-read when file operations finish, and when the item
        // changes on disk for any other reason (another app, Terminal).
        NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: .rustyfinderActivitiesChanged, object: nil)
        for item in items where item.isLocal { watch(item.path) }
    }

    /// Watchers on the items' files, by path.
    private var watchers: [String: DispatchSourceFileSystemObject] = [:]
    /// Changes noticed so far (scripted checks).
    private(set) var outsideChanges = 0

    private func watch(_ path: String) {
        watchers[path]?.cancel()
        let fd = Darwin.open(path, O_EVTONLY | O_SYMLINK)
        guard fd >= 0 else { return }
        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd, eventMask: [.write, .attrib, .extend, .delete, .rename, .link], queue: .main)
        source.setEventHandler { [weak self, weak source] in
            guard let self, let source else { return }
            let replaced = !source.data.intersection([.delete, .rename]).isEmpty
            self.outsideChanges += 1
            // A burst of changes (a save) settles before the form is rebuilt.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
                self.info.refresh()
                // Saved by replacing the file: watch the new one.
                if replaced, FileManager.default.fileExists(atPath: path) { self.watch(path) }
            }
        }
        source.setCancelHandler { Darwin.close(fd) }
        source.resume()
        watchers[path] = source
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    @objc private func refresh() {
        info.refresh()
    }

    func windowWillClose(_ notification: Notification) {
        Self.open[key] = nil
        watchers.values.forEach { $0.cancel() }
        watchers.removeAll()
    }
}
