import AppKit

/// The list shown in the Activities pop-up: one row per running or queued
/// operation, with Pause/Resume and Stop.
final class ActivitiesViewController: NSViewController {
    /// Only this window's "Stop All" target; the list shows every activity.
    var windowId: UInt64 = 0
    /// The pop-up has a fixed width; inside the preview pane it fills it.
    var fixedWidth = true {
        didSet { widthConstraint?.isActive = fixedWidth }
    }
    private var widthConstraint: NSLayoutConstraint?

    private let stack = NSStackView()
    private let emptyLabel = NSTextField(labelWithString: "No activities")
    private let stopAll = NSButton(title: "Stop All", target: nil, action: nil)
    private var rows: [UInt64: ActivityRowView] = [:]

    override func loadView() {
        // Queued activities can be dragged to a new place in the queue.
        let root = ActivityDropView()
        root.owner = self
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.translatesAutoresizingMaskIntoConstraints = false

        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.translatesAutoresizingMaskIntoConstraints = false

        stopAll.target = self
        stopAll.action = #selector(stopAllActivities(_:))
        stopAll.bezelStyle = .accessoryBarAction
        stopAll.controlSize = .small
        stopAll.translatesAutoresizingMaskIntoConstraints = false

        root.addSubview(stack)
        root.addSubview(emptyLabel)
        root.addSubview(stopAll)
        widthConstraint = root.widthAnchor.constraint(equalToConstant: 360)
        widthConstraint?.isActive = fixedWidth
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor, constant: 14),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 14),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -14),
            stopAll.topAnchor.constraint(equalTo: stack.bottomAnchor, constant: 12),
            stopAll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -14),
            stopAll.bottomAnchor.constraint(lessThanOrEqualTo: root.bottomAnchor, constant: -12),
            {
                // Hug the content in the pop-up; in the pane, stay at the top.
                let hug = stopAll.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -12)
                hug.priority = .defaultLow
                return hug
            }(),
            emptyLabel.centerXAnchor.constraint(equalTo: root.centerXAnchor),
            emptyLabel.topAnchor.constraint(equalTo: root.topAnchor, constant: 18),
        ])
        view = root
        NotificationCenter.default.addObserver(
            self, selector: #selector(refresh), name: .rustyfinderActivitiesChanged, object: nil)
        refresh()
    }

    @objc private func refresh() {
        let activities = ActivityCenter.shared.activities
        let alive = Set(activities.map(\.id))
        for (id, row) in rows where !alive.contains(id) {
            stack.removeArrangedSubview(row)
            row.removeFromSuperview()
            rows[id] = nil
        }
        for (index, activity) in activities.enumerated() {
            let row = rows[activity.id] ?? {
                let row = ActivityRowView()
                rows[activity.id] = row
                stack.addArrangedSubview(row)
                row.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
                return row
            }()
            // Keep rows in queue order (reordering changes it).
            if stack.arrangedSubviews[safe: index] !== row {
                stack.removeArrangedSubview(row)
                stack.insertArrangedSubview(row, at: min(index, stack.arrangedSubviews.count))
            }
            row.update(activity)
        }
        emptyLabel.isHidden = !activities.isEmpty
        stopAll.isHidden = activities.isEmpty
    }

    @objc private func stopAllActivities(_ sender: Any?) {
        AppEnvironment.shared.core.stopAllActivities(window: windowId)
    }

    // MARK: Reordering the queue (file-ops §5.2)

    /// The queued activities, in queue order.
    private var queued: [ActivityRecord] {
        ActivityCenter.shared.activities.filter { $0.phase == .queued }
    }

    /// Where among the queued activities a drop at `y` (in `stack`'s
    /// coordinates) lands.
    fileprivate func queueIndex(at point: NSPoint, in view: NSView) -> Int {
        let local = stack.convert(point, from: view)
        let queuedRows = queued.compactMap { rows[$0.id] }
        // Rows are stacked top-down in a flipped-or-not view: compare midpoints.
        for (index, row) in queuedRows.enumerated() {
            let frame = row.frame
            let above = stack.isFlipped ? local.y < frame.midY : local.y > frame.midY
            if above { return index }
        }
        return queuedRows.count
    }

    /// Move a queued activity to `index` among the queued ones.
    func moveQueued(_ id: UInt64, to index: Int) {
        guard let from = queued.firstIndex(where: { $0.id == id }) else { return }
        // Dropping below itself means one slot less once it is taken out.
        let target = index > from ? index - 1 : index
        guard target != from else { return }
        AppEnvironment.shared.core.reorderActivity(id: id, index: UInt32(max(0, target)))
    }

    /// Scripted checks: move the queued activity at `from` to `to`.
    static func debugMoveQueued(from: Int, to: Int) {
        let queued = ActivityCenter.shared.activities.filter { $0.phase == .queued }
        guard let activity = queued[safe: from] else { return }
        AppEnvironment.shared.core.reorderActivity(id: activity.id, index: UInt32(to))
    }
}

/// The pasteboard type of a dragged activity (its id as text).
private extension NSPasteboard.PasteboardType {
    static let rustyfinderActivity = NSPasteboard.PasteboardType("dev.rustyfinder.activity")
}

/// The activity list's background: accepts a dragged queued activity and
/// tells its controller where it goes.
private final class ActivityDropView: NSView {
    weak var owner: ActivitiesViewController?

    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.rustyfinderActivity])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { .move }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { .move }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard let text = sender.draggingPasteboard.string(forType: .rustyfinderActivity), let id = UInt64(text),
              let owner else { return false }
        let point = convert(sender.draggingLocation, from: nil)
        MainActor.assumeIsolated { owner.moveQueued(id, to: owner.queueIndex(at: point, in: self)) }
        return true
    }
}

/// One activity: title, a detail line, a progress bar and its controls.
final class ActivityRowView: NSView, NSDraggingSource {
    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")
    private let bar = NSProgressIndicator()
    private let pauseButton = NSButton()
    private let stopButton = NSButton()
    private var activity: ActivityRecord?

    override init(frame: NSRect) {
        super.init(frame: frame)
        title.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .medium)
        title.lineBreakMode = .byTruncatingMiddle
        detail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingMiddle
        bar.style = .bar
        bar.controlSize = .small
        bar.minValue = 0
        bar.maxValue = 1

        for (button, symbol, label, action) in [
            (pauseButton, "pause.fill", "Pause", #selector(togglePause(_:))),
            (stopButton, "xmark.circle.fill", "Stop", #selector(stop(_:))),
        ] {
            button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
            button.isBordered = false
            button.toolTip = label
            button.target = self
            button.action = action
            button.setContentHuggingPriority(.required, for: .horizontal)
        }
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        detail.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let top = NSStackView(views: [title, pauseButton, stopButton])
        top.orientation = .horizontal
        top.spacing = 6
        let column = NSStackView(views: [top, bar, detail])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 3
        column.translatesAutoresizingMaskIntoConstraints = false
        addSubview(column)
        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: topAnchor),
            column.bottomAnchor.constraint(equalTo: bottomAnchor),
            column.leadingAnchor.constraint(equalTo: leadingAnchor),
            column.trailingAnchor.constraint(equalTo: trailingAnchor),
            top.widthAnchor.constraint(equalTo: column.widthAnchor),
            bar.widthAnchor.constraint(equalTo: column.widthAnchor),
            detail.widthAnchor.constraint(lessThanOrEqualTo: column.widthAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func update(_ activity: ActivityRecord) {
        self.activity = activity
        title.stringValue = activity.title
        let text = Self.detailText(activity)
        detail.stringValue = text
        // The line is often cut short; the tooltip carries all of it.
        detail.toolTip = text

        let paused = activity.phase == .paused
        pauseButton.isHidden = activity.phase == .queued
        pauseButton.image = NSImage(
            systemSymbolName: paused ? "play.fill" : "pause.fill",
            accessibilityDescription: paused ? "Resume" : "Pause")
        pauseButton.toolTip = paused ? "Resume" : "Pause"

        let known = activity.bytesTotal > 0 || activity.itemsTotal > 0
        let waiting = activity.phase == .queued || activity.phase == .preparing || !known
        bar.isIndeterminate = waiting
        if waiting {
            activity.phase == .queued ? bar.stopAnimation(nil) : bar.startAnimation(nil)
        } else {
            bar.stopAnimation(nil)
            bar.doubleValue = activity.bytesTotal > 0
                ? Double(activity.bytesDone) / Double(activity.bytesTotal)
                : Double(activity.itemsDone) / Double(activity.itemsTotal)
        }
    }

    static func detailText(_ a: ActivityRecord) -> String {
        if a.waitingForUser { return "Waiting for your answer" }
        switch a.phase {
        case .queued:
            return "Queued"
        case .preparing:
            return a.itemsTotal > 0 ? "Preparing… \(a.itemsTotal.formatted()) items found" : "Preparing…"
        case .paused, .running:
            var parts: [String] = []
            if a.phase == .paused { parts.append("Paused") }
            parts.append("\(a.itemsDone.formatted()) of \(a.itemsTotal.formatted())")
            if a.bytesTotal > 0 {
                let p = RowPresenter.shared
                parts.append("\(p.bytes(a.bytesDone)) of \(p.bytes(a.bytesTotal))")
            }
            if a.phase == .running, a.speed > 0 {
                parts.append("\(RowPresenter.shared.bytes(a.speed))/s")
                if let eta = a.etaSecs { parts.append(remaining(eta)) }
            }
            if !a.current.isEmpty { parts.append(a.current) }
            return parts.joined(separator: " · ")
        }
    }

    private static func remaining(_ seconds: UInt64) -> String {
        switch seconds {
        case 0..<60: return "about \(max(seconds, 1)) s left"
        case 60..<3600: return "about \(seconds / 60) min left"
        default: return "about \(seconds / 3600) h left"
        }
    }

    // MARK: Dragging a queued activity

    override func mouseDragged(with event: NSEvent) {
        guard let activity, activity.phase == .queued else { return super.mouseDragged(with: event) }
        let item = NSPasteboardItem()
        item.setString(String(activity.id), forType: .rustyfinderActivity)
        let dragging = NSDraggingItem(pasteboardWriter: item)
        let image = NSImage(size: bounds.size)
        if let rep = bitmapImageRepForCachingDisplay(in: bounds) {
            cacheDisplay(in: bounds, to: rep)
            image.addRepresentation(rep)
        }
        dragging.setDraggingFrame(bounds, contents: image)
        beginDraggingSession(with: [dragging], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .withinApplication ? .move : []
    }

    @objc private func togglePause(_ sender: Any?) {
        guard let activity else { return }
        let core = AppEnvironment.shared.core
        activity.phase == .paused ? core.resumeActivity(id: activity.id) : core.pauseActivity(id: activity.id)
    }

    @objc private func stop(_ sender: Any?) {
        guard let activity else { return }
        AppEnvironment.shared.core.stopActivity(id: activity.id)
    }
}
