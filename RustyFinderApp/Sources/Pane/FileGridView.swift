import AppKit
import QuickLookThumbnailing

/// Icon view: a grid of icons (or thumbnails) with names below. Offers key
/// presses to its pane first, like the list does.
final class FileGridView: NSCollectionView, NSUserInterfaceValidations {
    /// A drag left the view or ended (cancels spring-loading).
    var onDragExit: (() -> Void)?

    override func draggingExited(_ sender: NSDraggingInfo?) {
        super.draggingExited(sender)
        onDragExit?()
    }

    override func draggingEnded(_ sender: NSDraggingInfo) {
        super.draggingEnded(sender)
        onDragExit?()
    }

    weak var keyHandler: FileTableKeyHandler?
    /// Item under the pointer at the last right-click, or -1 for empty space.
    private(set) var clickedIndex = -1
    /// Item number of a position (the grid has one section per group).
    var indexOf: (IndexPath) -> Int = { $0.item }
    var onDoubleClick: (() -> Void)?
    /// Focus moved in or out: selection colour must follow.
    var onFocusChange: (() -> Void)?

    override func keyDown(with event: NSEvent) {
        if keyHandler?.fileTable(self, handleKeyDown: event) == true { return }
        super.keyDown(with: event)
    }

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok {
            keyHandler?.fileTableDidBecomeFirstResponder(self)
            onFocusChange?()
        }
        return ok
    }

    override func resignFirstResponder() -> Bool {
        let ok = super.resignFirstResponder()
        if ok { onFocusChange?() }
        return ok
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let onItem = indexPathForItem(at: point) != nil
        DragTrace.note("grid mouseDown enter clicks=\(event.clickCount)")
        super.mouseDown(with: event)
        DragTrace.note("grid mouseDown exit \(DragTrace.buttons)")
        // A double-click on empty space must do nothing.
        if event.clickCount == 2, onItem { onDoubleClick?() }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        clickedIndex = indexPathForItem(at: convert(event.locationInWindow, from: nil)).map { indexOf($0) } ?? -1
        return super.menu(for: event)
    }

    // Undo and Redo, as in FileTableView: file operations while the grid
    // has focus, ordinary text undo while a name is being edited.
    private var isEditingText: Bool { window?.firstResponder !== self }

    @objc func undo(_ sender: Any?) {
        isEditingText ? window?.undoManager?.undo() : keyHandler?.fileTable(self, undo: false)
    }

    @objc func redo(_ sender: Any?) {
        isEditingText ? window?.undoManager?.redo() : keyHandler?.fileTable(self, undo: true)
    }

    func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        let redo: Bool
        switch item.action {
        case FileTableView.undoAction: redo = false
        case FileTableView.redoAction: redo = true
        // Select All works whenever there is something to select.
        default: return true
        }
        let menuItem = item as? NSMenuItem
        if isEditingText {
            let manager = window?.undoManager
            menuItem?.title = (redo ? manager?.redoMenuItemTitle : manager?.undoMenuItemTitle) ?? (redo ? "Redo" : "Undo")
            return (redo ? manager?.canRedo : manager?.canUndo) ?? false
        }
        let title = keyHandler?.fileTable(self, undoTitle: redo)
        menuItem?.title = title ?? (redo ? "Redo" : "Undo")
        return title != nil
    }
}

/// One cell of the icon view.
/// The header above a group in a grouped Icon view.
final class GridGroupHeader: NSView, NSCollectionViewElement {
    static let id = NSUserInterfaceItemIdentifier("GridGroupHeader")
    private let label = NSTextField(labelWithString: "")

    var title: String {
        get { label.stringValue }
        set { label.stringValue = newValue }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        label.font = .boldSystemFont(ofSize: NSFont.smallSystemFontSize)
        label.textColor = .secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }
}

final class FileGridItem: NSCollectionViewItem {
    static let id = NSUserInterfaceItemIdentifier("FileGridItem")

    private let icon = NSImageView()
    private let label = NSTextField(labelWithString: "")
    /// "Show item info": size, item count or pixel size under the name.
    private let info = NSTextField(labelWithString: "")
    private let iconBacking = NSView()
    private let scmBadge = ScmBadge.makeView()
    private var iconSide: NSLayoutConstraint!
    /// Path this cell shows, so a thumbnail that arrives late is only used
    /// if the cell has not been recycled for another file.
    private(set) var path = ""
    var paneIsActive = true { didSet { refreshHighlight() } }

    override func loadView() {
        let root = NSView()
        iconBacking.wantsLayer = true
        iconBacking.layer?.cornerRadius = 6
        icon.imageScaling = .scaleProportionallyUpOrDown
        label.alignment = .center
        label.lineBreakMode = .byTruncatingMiddle
        label.maximumNumberOfLines = 2
        label.cell?.wraps = true
        label.cell?.truncatesLastVisibleLine = true
        label.wantsLayer = true
        label.layer?.cornerRadius = 3
        info.alignment = .center
        info.font = .systemFont(ofSize: NSFont.smallSystemFontSize - 1)
        info.textColor = .systemBlue
        info.lineBreakMode = .byTruncatingTail
        info.isHidden = true
        for v in [iconBacking, icon, label, info, scmBadge] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        iconSide = icon.widthAnchor.constraint(equalToConstant: 64)
        NSLayoutConstraint.activate([
            icon.topAnchor.constraint(equalTo: root.topAnchor, constant: 6),
            icon.centerXAnchor.constraint(equalTo: root.centerXAnchor),
            iconSide,
            icon.heightAnchor.constraint(equalTo: icon.widthAnchor),
            iconBacking.topAnchor.constraint(equalTo: icon.topAnchor, constant: -4),
            iconBacking.bottomAnchor.constraint(equalTo: icon.bottomAnchor, constant: 4),
            iconBacking.leadingAnchor.constraint(equalTo: icon.leadingAnchor, constant: -4),
            iconBacking.trailingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 4),
            label.topAnchor.constraint(equalTo: icon.bottomAnchor, constant: 6),
            label.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 2),
            label.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -2),
            info.topAnchor.constraint(equalTo: label.bottomAnchor, constant: 1),
            info.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 2),
            info.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -2),
            // Git status sits on the icon's lower right corner.
            scmBadge.widthAnchor.constraint(equalToConstant: 16),
            scmBadge.heightAnchor.constraint(equalToConstant: 16),
            scmBadge.trailingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 2),
            scmBadge.bottomAnchor.constraint(equalTo: icon.bottomAnchor, constant: 2),
        ])
        view = root
        imageView = icon
        textField = label
    }

    func configure(name: String, path: String, image: NSImage, hidden: Bool, iconSize: CGFloat, fontSize: CGFloat) {
        self.path = path
        hasThumbnail = false
        label.stringValue = name
        label.font = ViewDisplay.font(size: fontSize)
        icon.image = image
        icon.alphaValue = hidden ? 0.5 : 1
        iconSide.constant = iconSize
        view.toolTip = name
        view.setAccessibilityLabel(name)
        refreshHighlight(hidden: hidden)
    }

    var infoText: String { info.isHidden ? "-" : info.stringValue }

    func setInfo(_ text: String?) {
        info.stringValue = text ?? ""
        info.isHidden = text == nil
    }

    /// Up to three colour-tag dots before the name.
    func setTagDots(_ dots: NSAttributedString) {
        guard dots.length > 0 else { return }
        let text = NSMutableAttributedString(attributedString: dots)
        text.append(NSAttributedString(string: " " + label.stringValue, attributes: [.font: label.font as Any]))
        label.attributedStringValue = text
    }

    /// Git status badge; nil for a clean item or no repository.
    func setScmBadge(_ status: ScmStatus?) {
        loadViewIfNeeded()
        scmStatus = status
        ScmBadge.show(status, in: scmBadge)
    }

    private(set) var scmStatus: ScmStatus?

    var scmBadgeLetter: String? { scmStatus.map(ScmBadge.letter) }

    func setThumbnail(_ image: NSImage, for path: String) {
        if path == self.path {
            icon.image = image
            hasThumbnail = true
        }
    }

    /// The icon shows a thumbnail rather than a generic icon.
    private(set) var hasThumbnail = false

    override var isSelected: Bool {
        didSet { refreshHighlight() }
    }

    private func refreshHighlight(hidden: Bool? = nil) {
        guard isViewLoaded else { return }
        let dimmed = hidden ?? (icon.alphaValue < 1)
        if isSelected {
            // The inactive pane's selection is grey, the active one's accent.
            let fill: NSColor = paneIsActive ? .selectedContentBackgroundColor : .unemphasizedSelectedContentBackgroundColor
            iconBacking.layer?.backgroundColor = NSColor.labelColor.withAlphaComponent(0.1).cgColor
            label.layer?.backgroundColor = fill.cgColor
            label.textColor = paneIsActive ? .alternateSelectedControlTextColor : .labelColor
        } else {
            iconBacking.layer?.backgroundColor = NSColor.clear.cgColor
            label.layer?.backgroundColor = NSColor.clear.cgColor
            label.textColor = dimmed ? .tertiaryLabelColor : .labelColor
        }
    }
}

/// Thumbnails for the icon view, made off the main thread and cached.
@MainActor
final class ThumbnailCache {
    static let shared = ThumbnailCache()
    private let cache = NSCache<NSString, NSImage>()

    init() {
        cache.countLimit = 2000
    }

    /// Cached thumbnail, if there is one. The key includes the date and
    /// size so an edited file gets a fresh thumbnail.
    func cached(_ path: String, row: Row, side: CGFloat) -> NSImage? {
        cache.object(forKey: key(path, row, side))
    }

    private func key(_ path: String, _ row: Row, _ side: CGFloat) -> NSString {
        "\(path)|\(row.modifiedMs ?? 0)|\(row.size)|\(Int(side))" as NSString
    }

    /// Files inside an archive larger than this get no thumbnail: making
    /// one would mean extracting all of it.
    static let archiveThumbnailLimit: UInt64 = 32 * 1_000_000

    /// A thumbnail for an item inside an archive, made from an extracted
    /// copy, cached under the item's own path.
    func requestInArchive(_ item: InspectedItem, side: CGFloat, scale: CGFloat, completion: @escaping (NSImage) -> Void) {
        guard item.row.size <= Self.archiveThumbnailLimit else { return }
        let key = key(item.path, item.row, side)
        PreviewCache.shared.fetch(item) { [weak self] local in
            guard let local else { return }
            let request = QLThumbnailGenerator.Request(
                fileAt: URL(fileURLWithPath: local), size: CGSize(width: side, height: side), scale: scale,
                representationTypes: .thumbnail)
            QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { representation, _ in
                guard let image = representation?.nsImage else { return }
                DispatchQueue.main.async {
                    self?.cache.setObject(image, forKey: key)
                    completion(image)
                }
            }
        }
    }

    /// Ask for a thumbnail. Only real thumbnails are delivered (a generic
    /// icon is what the cell already shows).
    func request(_ path: String, row: Row, side: CGFloat, scale: CGFloat, completion: @escaping (NSImage) -> Void) {
        let key = key(path, row, side)
        let request = QLThumbnailGenerator.Request(
            fileAt: URL(fileURLWithPath: path), size: CGSize(width: side, height: side), scale: scale,
            representationTypes: .thumbnail)
        QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { [weak self] representation, _ in
            guard let image = representation?.nsImage else { return }
            DispatchQueue.main.async {
                self?.cache.setObject(image, forKey: key)
                completion(image)
            }
        }
    }
}
