import AppKit
import UniformTypeIdentifiers

/// An item shown in the preview pane or a Get Info window.
struct InspectedItem: Equatable {
    let path: String
    let row: Row
    /// Connection the item is on (0: this Mac). Items elsewhere are shown
    /// from what the listing says, and previewed from a local copy.
    var conn: UInt64 = 0

    var isLocal: Bool { conn == 0 }

    var url: URL { URL(fileURLWithPath: path) }
}

/// The Info section: what an item is, where it lives, its dates, tags,
/// comment, flags, default application and permissions. Used at the bottom
/// of the preview pane and as the whole of a Get Info window.
final class InfoViewController: NSViewController, NSTextFieldDelegate, NSTokenFieldDelegate {
    /// Navigate the active pane to a folder (the clickable "Where").
    var onNavigate: ((String) -> Void)?
    /// Window the item belongs to, for operations started from here.
    var windowId: UInt64 = 0

    private(set) var items: [InspectedItem] = []
    private let stack = NSStackView()
    private var sizeLabel: NSTextField?
    private var octalField: NSTextField?
    private var permissionBoxes: [NSButton] = []
    private var commentField: NSTextField?
    private var tagField: NSTokenField?
    /// The checkboxes of the current form (to tell them from plain buttons).
    private var checkboxes: Set<ObjectIdentifier> = []
    /// Values currently on disk, to tell a real edit from a mere focus change.
    private var loadedComment = ""
    private var loadedTags: [String] = []
    /// The single item's permissions as last read (from disk when local).
    private var currentMode: UInt32 = 0
    /// Several items: every item's tags as read, for editing them together.
    private var severalTags: [[String]] = []

    override func loadView() {
        // Folder sizes are shared with the list (Calculate Folder Size).
        NotificationCenter.default.addObserver(
            forName: .rustyfinderFolderSizeChanged, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, let current = self.items.first else { return }
                self.sizeLabel?.stringValue = self.items.count > 1 ? self.severalSizeText() : self.sizeText(for: current)
            }
        }
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
        stack.translatesAutoresizingMaskIntoConstraints = false

        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        let clip = FlippedClipView()
        clip.drawsBackground = false
        scroll.contentView = clip
        scroll.documentView = stack
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: clip.topAnchor),
            stack.leadingAnchor.constraint(equalTo: clip.leadingAnchor),
            stack.widthAnchor.constraint(equalTo: clip.widthAnchor),
        ])
        view = scroll
    }

    /// Show these items. One item gets the full form; several get a summary.
    func show(_ items: [InspectedItem]) {
        loadViewIfNeeded()
        self.items = items
        rebuild()
    }

    /// Re-read the current items from disk (after an edit or a change).
    func refresh() {
        // Do not rebuild under the user's cursor.
        if let editor = view.window?.firstResponder as? NSTextView, editor.isDescendant(of: stack) { return }
        rebuild()
    }

    private func rebuild() {
        stack.arrangedSubviews.forEach {
            stack.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        sizeLabel = nil
        octalField = nil
        checkboxes = []
        permissionBoxes = []
        commentField = nil
        tagField = nil
        switch items.count {
        case 0: add(note("No Selection"))
        case 1: items[0].isLocal ? buildSingle(items[0]) : buildElsewhere(items[0])
        default: buildSeveral(items)
        }
    }

    // MARK: Single item

    private func buildSingle(_ item: InspectedItem) {
        let presenter = RowPresenter.shared
        let keys: Set<URLResourceKey> = [
            .creationDateKey, .contentModificationDateKey, .contentAccessDateKey, .addedToDirectoryDateKey,
            .tagNamesKey, .isHiddenKey, .isUserImmutableKey, .hasHiddenExtensionKey,
        ]
        let values = try? item.url.resourceValues(forKeys: keys.union([.isAliasFileKey]))
        let attributes = try? FileManager.default.attributesOfItem(atPath: item.path)
        // Read afresh, so a change made elsewhere shows when the form is rebuilt.
        currentMode = (attributes?[.posixPermissions] as? NSNumber)?.uint32Value ?? item.row.mode ?? 0
        let diskSize = (attributes?[.size] as? NSNumber)?.uint64Value

        add(title(presenter.displayName(item.row)))
        add(row("Kind", presenter.kind(item.row)))

        // Size: readable and exact. Folders are counted on request.
        if item.row.isDir, item.row.kind != .symlink {
            let label = value(sizeText(for: item))
            sizeLabel = label
            let calculate = NSButton(title: "Calculate", target: self, action: #selector(calculateSize(_:)))
            calculate.bezelStyle = .accessoryBarAction
            calculate.controlSize = .small
            add(labelled("Size", NSStackView(views: [label, calculate])))
        } else if let diskSize, item.row.kind != .symlink {
            add(row("Size", "\(presenter.bytes(diskSize)) (\(diskSize.formatted()) bytes)"))
        } else {
            add(row("Size", sizeText(for: item)))
        }

        // Where: the containing folder, clickable.
        let parent = (item.path as NSString).deletingLastPathComponent
        let whereButton = NSButton(title: parent.abbreviatingHome, target: self, action: #selector(goToParent(_:)))
        whereButton.isBordered = false
        whereButton.contentTintColor = .linkColor
        whereButton.lineBreakMode = .byTruncatingMiddle
        whereButton.toolTip = parent
        whereButton.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        add(labelled("Where", whereButton))

        add(row("Created", date(values?.creationDate)))
        add(row("Modified", date(values?.contentModificationDate)))
        add(row("Last opened", date(values?.contentAccessDate)))
        add(row("Added", date(values?.addedToDirectoryDate)))

        if item.row.kind == .symlink || values?.isAliasFile == true {
            addOriginal(item, isAlias: item.row.kind != .symlink)
        }

        // Tags and comment are editable in place.
        add(separator())
        let tags = NSTokenField()
        tags.objectValue = values?.tagNames ?? []
        tags.delegate = self
        tags.placeholderString = "Add tags"
        tags.controlSize = .small
        tags.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        tagField = tags
        loadedTags = values?.tagNames ?? []
        add(labelled("Tags", tags, fill: true))

        let comment = NSTextField(string: Self.finderComment(at: item.url))
        comment.delegate = self
        comment.placeholderString = "Add a comment"
        comment.controlSize = .small
        comment.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        commentField = comment
        loadedComment = comment.stringValue
        add(labelled("Comments", comment, fill: true))

        let flags = NSStackView(views: [
            checkbox("Hidden", values?.isHidden ?? false, #selector(toggleHidden(_:))),
            checkbox("Locked", values?.isUserImmutable ?? false, #selector(toggleLocked(_:))),
            checkbox("Hide extension", values?.hasHiddenExtension ?? false, #selector(toggleHideExtension(_:))),
        ])
        flags.spacing = 10
        add(labelled("Flags", flags))

        if !item.row.isDir {
            add(labelled("Open with", openWithControls(for: item)))
        }

        // Permissions: checkboxes and the octal field mirror each other.
        add(separator())
        let owner = (attributes?[.ownerAccountName] as? String) ?? "-"
        let group = (attributes?[.groupOwnerAccountName] as? String) ?? "-"
        addPermissions(item, owner: owner, group: group, editable: true)
        if let acl = ACLInfo.summary(atPath: item.path) { add(row("ACL", acl)) }

        let more = MoreInfo.rows(for: item.url)
        if !more.isEmpty {
            add(separator())
            add(header("More Info"))
            more.forEach { add(row($0.0, $0.1)) }
        }
    }

    private func sizeText(for item: InspectedItem) -> String {
        let presenter = RowPresenter.shared
        if item.row.isDir, item.row.kind != .symlink {
            if FolderSizes.shared.isCalculating(item.path) { return "Calculating…" }
            guard let m = FolderSizes.shared.size(of: item.path) else { return "--" }
            let inside = m.items - 1
            return "\(presenter.bytes(m.bytes)) (\(m.bytes.formatted()) bytes), \(inside.formatted()) \(inside == 1 ? "item" : "items")"
        }
        return "\(presenter.bytes(item.row.size)) (\(item.row.size.formatted()) bytes)"
    }

    // MARK: Several items

    private func buildSeveral(_ items: [InspectedItem]) {
        add(title("\(items.count.formatted()) items"))
        let label = value(severalSizeText())
        sizeLabel = label
        let folders = items.filter { $0.row.isDir && $0.row.kind != .symlink && $0.isLocal }
        if folders.contains(where: { FolderSizes.shared.size(of: $0.path) == nil }) {
            let calculate = NSButton(title: "Calculate", target: self, action: #selector(calculateSeveral(_:)))
            calculate.bezelStyle = .accessoryBarAction
            calculate.controlSize = .small
            add(labelled("Size", NSStackView(views: [label, calculate])))
        } else {
            add(labelled("Size", label))
        }
        // One date when all agree, else the range from earliest to latest.
        let local = items.allSatisfy(\.isLocal)
        let values = local ? items.map { try? $0.url.resourceValues(forKeys: [.addedToDirectoryDateKey, .tagNamesKey]) } : []
        func ms(_ date: Date?) -> Int64? { date.map { Int64($0.timeIntervalSince1970 * 1000) } }
        add(row("Created", range(items.map(\.row.createdMs))))
        add(row("Modified", range(items.map(\.row.modifiedMs))))
        if local { add(row("Added", range(values.map { ms($0?.addedToDirectoryDate) }))) }

        // Tags and comments are edited for all of them at once.
        guard local else { return }
        add(separator())
        severalTags = values.map { $0?.tagNames ?? [] }
        let tags = NSTokenField()
        tags.objectValue = tagsCommon(items: severalTags)
        tags.delegate = self
        tags.placeholderString = "Add tags to all"
        tags.controlSize = .small
        tags.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        tagField = tags
        loadedTags = tagsCommon(items: severalTags)
        add(labelled("Tags", tags, fill: true))

        let comments = items.map { Self.finderComment(at: $0.url) }
        let same = Set(comments).count == 1
        let comment = NSTextField(string: same ? comments[0] : "")
        comment.delegate = self
        comment.placeholderString = same ? "Add a comment" : "Multiple values"
        comment.controlSize = .small
        comment.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        commentField = comment
        loadedComment = comment.stringValue
        add(labelled("Comments", comment, fill: true))
    }

    /// Total of the files, plus folders whose size is known.
    private func severalSizeText() -> String {
        var bytes: UInt64 = 0
        var uncounted = 0
        for item in items {
            if item.row.isDir, item.row.kind != .symlink {
                if let measured = item.isLocal ? FolderSizes.shared.size(of: item.path) : nil {
                    bytes += measured.bytes
                } else {
                    uncounted += 1
                }
            } else {
                bytes += item.row.size
            }
        }
        var text = "\(RowPresenter.shared.bytes(bytes)) (\(bytes.formatted()) bytes)"
        let calculating = items.contains { FolderSizes.shared.isCalculating($0.path) }
        if calculating {
            text += ", calculating…"
        } else if uncounted > 0 {
            text += ", plus \(uncounted.formatted()) \(uncounted == 1 ? "folder" : "folders") not counted"
        }
        return text
    }

    /// "date" when all agree, "earliest – latest" when not, "--" when unknown.
    private func range(_ dates: [Int64?]) -> String {
        let known = dates.compactMap { $0 }
        guard let first = known.min(), let last = known.max() else { return "--" }
        let presenter = RowPresenter.shared
        return first == last ? presenter.date(first) : "\(presenter.date(first)) – \(presenter.date(last))"
    }

    @objc private func calculateSeveral(_ sender: Any?) {
        FolderSizes.shared.calculate(items.filter { $0.row.isDir && $0.isLocal }.map(\.path))
        sizeLabel?.stringValue = severalSizeText()
    }

    /// Tags or comment edited with several items selected: applied to all.
    private func severalFieldEdited(_ field: NSTextField) {
        if field === tagField {
            let edited = (tagField?.objectValue as? [String]) ?? []
            guard edited != loadedTags else { return }
            loadedTags = edited
            let updated = tagsApplyCommon(items: severalTags, edited: edited)
            for (item, tags) in zip(items, updated) {
                do {
                    try (item.url as NSURL).setResourceValue(tags, forKey: .tagNamesKey)
                } catch {
                    report(error)
                    break
                }
            }
            severalTags = updated
        } else if field === commentField {
            guard field.stringValue != loadedComment else { return }
            loadedComment = field.stringValue
            for item in items {
                Self.setFinderComment(field.stringValue, at: item.url) { [weak self] error in
                    if let error { self?.reportMessage("The comment could not be saved.", error) }
                }
            }
        }
    }

    // MARK: Items on a server or in an archive

    /// What the listing says about an item elsewhere: no tags, comments or
    /// flags there. Permissions are edited through the connection where it
    /// has them (SFTP, FTP); archives are read-only.
    private func buildElsewhere(_ item: InspectedItem) {
        let presenter = RowPresenter.shared
        add(title(presenter.displayName(item.row)))
        add(row("Kind", presenter.kind(item.row)))
        add(row("Size", item.row.isDir ? "--" : "\(presenter.bytes(item.row.size)) (\(item.row.size.formatted()) bytes)"))
        let parent = (item.path as NSString).deletingLastPathComponent
        let whereButton = NSButton(title: parent, target: self, action: #selector(goToParent(_:)))
        whereButton.isBordered = false
        whereButton.contentTintColor = .linkColor
        whereButton.lineBreakMode = .byTruncatingMiddle
        whereButton.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        add(labelled("Where", whereButton))
        add(row("Created", item.row.createdMs.map { presenter.date($0) } ?? "--"))
        add(row("Modified", item.row.modifiedMs.map { presenter.date($0) } ?? "--"))
        if item.row.kind == .symlink { add(row("Original", item.row.linkTarget ?? "")) }
        let archive = ArchiveCenter.shared.isArchive(item.conn)
        if archive { add(note("Inside an archive (read-only)")) }
        if item.row.mode != nil || !archive {
            add(separator())
            currentMode = item.row.mode ?? 0
            let editable = AppEnvironment.shared.core.canSetMode(connection: item.conn)
            addPermissions(item, owner: item.row.owner ?? "-", group: item.row.group ?? "-", editable: editable)
        }
        // Type details from the preview's local copy, once there is one.
        if let local = PreviewCache.shared.local(for: item), !item.row.isDir {
            let more = MoreInfo.rows(for: URL(fileURLWithPath: local))
            if !more.isEmpty {
                add(separator())
                add(header("More Info"))
                more.forEach { add(row($0.0, $0.1)) }
            }
        }
    }

    /// Owner, group and others as checkboxes, mirrored by an octal field.
    private func addPermissions(_ item: InspectedItem, owner: String, group: String, editable: Bool) {
        let mode = currentMode
        add(header("Permissions"))
        for (index, who) in ["Owner (\(owner))", "Group (\(group))", "Others"].enumerated() {
            let shift = UInt32(6 - index * 3)
            let boxes = ["Read", "Write", "Execute"].enumerated().map { bit, name -> NSButton in
                let box = checkbox(name, mode & (UInt32(4 >> bit) << shift) != 0, #selector(permissionToggled(_:)))
                box.tag = Int(UInt32(4 >> bit) << shift)
                box.isEnabled = editable
                permissionBoxes.append(box)
                return box
            }
            let line = NSStackView(views: boxes)
            line.spacing = 8
            add(labelled(who, line))
        }
        let octal = NSTextField(string: permissionOctal(mode: mode))
        octal.delegate = self
        octal.controlSize = .small
        octal.font = .monospacedDigitSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        octal.widthAnchor.constraint(equalToConstant: 56).isActive = true
        octal.isEditable = editable
        octalField = octal
        var controls: [NSView] = [octal]
        if editable, item.row.isDir, item.row.kind != .symlink {
            let apply = NSButton(title: "Apply to Enclosed Items", target: self, action: #selector(applyToEnclosed(_:)))
            apply.bezelStyle = .accessoryBarAction
            apply.controlSize = .small
            controls.append(apply)
        }
        add(labelled("Octal", NSStackView(views: controls)))
    }

    // MARK: Links and aliases

    /// Where a link or alias points, what that is, and a way to point it
    /// somewhere else.
    private func addOriginal(_ item: InspectedItem, isAlias: Bool) {
        let target = Self.originalPath(of: item, isAlias: isAlias)
        let shown = target ?? item.row.linkTarget ?? "?"
        let exists = target.map { FileManager.default.fileExists(atPath: $0) } ?? false
        let label = value(exists ? shown : "\(shown) (missing)")
        let change = NSButton(title: "Change…", target: self, action: #selector(changeOriginal(_:)))
        change.bezelStyle = .accessoryBarAction
        change.controlSize = .small
        add(labelled("Original", NSStackView(views: [label, change])))
        guard let target, exists else { return }
        let url = URL(fileURLWithPath: target)
        let keys: Set<URLResourceKey> = [.localizedTypeDescriptionKey, .fileSizeKey, .isDirectoryKey, .creationDateKey, .contentModificationDateKey]
        let values = try? url.resourceValues(forKeys: keys)
        add(row("Original kind", values?.localizedTypeDescription ?? "--"))
        if values?.isDirectory != true, let size = values?.fileSize {
            add(row("Original size", "\(RowPresenter.shared.bytes(UInt64(size))) (\(size.formatted()) bytes)"))
        }
        add(row("Original created", date(values?.creationDate)))
        add(row("Original modified", date(values?.contentModificationDate)))
    }

    /// Full path a symlink (relative targets resolved) or alias points to.
    static func originalPath(of item: InspectedItem, isAlias: Bool) -> String? {
        if isAlias {
            return (try? URL(resolvingAliasFileAt: item.url, options: [.withoutUI, .withoutMounting]))?.path
        }
        guard let target = item.row.linkTarget ?? (try? FileManager.default.destinationOfSymbolicLink(atPath: item.path))
        else { return nil }
        if target.hasPrefix("/") { return target }
        let base = (item.path as NSString).deletingLastPathComponent
        return ((base as NSString).appendingPathComponent(target) as NSString).standardizingPath
    }

    @objc private func changeOriginal(_ sender: Any?) {
        guard let item = items.first, let window = view.window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.prompt = "Choose"
        panel.message = "Choose the new original for “\(item.row.name)”."
        panel.beginSheetModal(for: window) { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            self?.pointOriginal(of: item, to: url.path)
        }
    }

    /// Point a link or alias at `target`. A symlink is replaced in one
    /// step (a new link renamed over the old one), so it is never missing.
    func pointOriginal(of item: InspectedItem, to target: String) {
        let isAlias = item.row.kind != .symlink
        do {
            if isAlias {
                let data = try URL(fileURLWithPath: target).bookmarkData(options: .suitableForBookmarkFile)
                try URL.writeBookmarkData(data, to: item.url)
            } else {
                let staging = (item.path as NSString).deletingLastPathComponent
                    + "/.rustyfinder-link-\(ProcessInfo.processInfo.processIdentifier)"
                try? FileManager.default.removeItem(atPath: staging)
                try FileManager.default.createSymbolicLink(atPath: staging, withDestinationPath: target)
                guard rename(staging, item.path) == 0 else {
                    try? FileManager.default.removeItem(atPath: staging)
                    throw CocoaError(.fileWriteUnknown)
                }
            }
        } catch {
            report(error)
            return
        }
        // The listing's row still has the old target: read it afresh.
        var fresh = item
        if !isAlias, let row = Self.relisted(item) { fresh = InspectedItem(path: item.path, row: row, conn: 0) }
        items = items.map { $0.path == item.path ? fresh : $0 }
        rebuild()
    }

    /// The item's row with its link target as it is now on disk.
    private static func relisted(_ item: InspectedItem) -> Row? {
        guard let target = try? FileManager.default.destinationOfSymbolicLink(atPath: item.path) else { return nil }
        var row = item.row
        row.linkTarget = target
        row.isBrokenLink = !FileManager.default.fileExists(atPath: item.path)
        return row
    }

    // MARK: Actions

    @objc private func goToParent(_ sender: Any?) {
        guard let item = items.first else { return }
        onNavigate?((item.path as NSString).deletingLastPathComponent)
    }

    @objc private func calculateSize(_ sender: Any?) {
        guard let item = items.first else { return }
        FolderSizes.shared.calculate([item.path])
    }

    private func setResource(_ change: (inout URLResourceValues) -> Void) {
        guard let item = items.first else { return }
        var url = item.url
        var values = URLResourceValues()
        change(&values)
        do {
            try url.setResourceValues(values)
        } catch {
            report(error)
            rebuild()
        }
    }

    @objc private func toggleHidden(_ sender: NSButton) { setResource { $0.isHidden = sender.state == .on } }
    @objc private func toggleLocked(_ sender: NSButton) { setResource { $0.isUserImmutable = sender.state == .on } }
    @objc private func toggleHideExtension(_ sender: NSButton) { setResource { $0.hasHiddenExtension = sender.state == .on } }

    /// The mode the checkboxes currently spell out.
    private var checkedMode: UInt32 {
        permissionBoxes.reduce(UInt32(0)) { $0 | ($1.state == .on ? UInt32($1.tag) : 0) }
    }

    @objc private func permissionToggled(_ sender: NSButton) {
        guard items.first != nil else { return }
        // Keep any special bits (setuid and friends) the item already had.
        let mode = (currentMode & 0o7000) | checkedMode
        octalField?.stringValue = permissionOctal(mode: mode)
        submitMode(mode, recursive: false)
    }

    @objc private func applyToEnclosed(_ sender: Any?) {
        guard let text = octalField?.stringValue, let mode = parsePermissionOctal(text: text) else { return }
        submitMode(mode, recursive: true)
    }

    private func submitMode(_ mode: UInt32, recursive: Bool) {
        guard let item = items.first else { return }
        currentMode = mode
        // On a server this goes through the connection (setstat, SITE CHMOD).
        ActivityCenter.shared.submit(
            .setMode(paths: [item.path], mode: mode, recursive: recursive), window: windowId, src: item.conn, dst: item.conn)
    }

    func controlTextDidEndEditing(_ notification: Notification) {
        guard let field = notification.object as? NSTextField, let item = items.first else { return }
        if items.count > 1 {
            severalFieldEdited(field)
            return
        }
        if field === octalField {
            guard let mode = parsePermissionOctal(text: field.stringValue) else {
                // Not valid octal: put back what the item has.
                field.stringValue = permissionOctal(mode: currentMode)
                NSSound.beep()
                return
            }
            permissionBoxes.forEach { $0.state = mode & UInt32($0.tag) != 0 ? .on : .off }
            if mode != currentMode & 0o7777 { submitMode(mode, recursive: false) }
        } else if field === commentField {
            guard field.stringValue != loadedComment else { return }
            let before = loadedComment, after = field.stringValue
            loadedComment = after
            let windowId = self.windowId
            Self.setFinderComment(after, at: item.url) { [weak self] error in
                if let error {
                    self?.reportMessage("The comment could not be saved.", error)
                } else {
                    // Undoable like a file operation (file-ops §8).
                    AppEnvironment.shared.core.recordCommentChange(window: windowId, path: item.path, before: before, after: after)
                }
            }
        } else if field === tagField {
            let tags = (tagField?.objectValue as? [String]) ?? []
            guard tags != loadedTags else { return }
            let before = loadedTags
            loadedTags = tags
            do {
                try (item.url as NSURL).setResourceValue(tags, forKey: .tagNamesKey)
                AppEnvironment.shared.core.recordTagChange(
                    window: windowId, changes: [TagChange(path: item.path, before: before, after: tags)])
            } catch {
                report(error)
            }
        }
    }

    // MARK: Open with

    private func openWithControls(for item: InspectedItem) -> NSView {
        let workspace = NSWorkspace.shared
        let current = workspace.urlForApplication(toOpen: item.url)
        var apps = workspace.urlsForApplications(toOpen: item.url)
        if let current, !apps.contains(current) { apps.insert(current, at: 0) }

        let popup = NSPopUpButton()
        popup.controlSize = .small
        popup.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        for app in apps.prefix(30) {
            popup.addItem(withTitle: FileManager.default.displayName(atPath: app.path))
            popup.lastItem?.representedObject = app
            let icon = workspace.icon(forFile: app.path)
            icon.size = NSSize(width: 16, height: 16)
            popup.lastItem?.image = icon
        }
        if let current, let index = apps.firstIndex(of: current) { popup.selectItem(at: index) }
        popup.target = self
        popup.action = #selector(openWithChosen(_:))
        popup.isEnabled = !apps.isEmpty

        let changeAll = NSButton(title: "Change All…", target: self, action: #selector(changeAll(_:)))
        changeAll.bezelStyle = .accessoryBarAction
        changeAll.controlSize = .small
        changeAll.isEnabled = !apps.isEmpty
        openWithPopup = popup
        return NSStackView(views: [popup, changeAll])
    }

    private var openWithPopup: NSPopUpButton?

    /// Choosing an application changes it for this file only.
    @objc private func openWithChosen(_ sender: NSPopUpButton) {
        guard let app = sender.selectedItem?.representedObject as? URL, let item = items.first else { return }
        NSWorkspace.shared.setDefaultApplication(at: app, toOpenFileAt: item.url) { [weak self] error in
            if let error { DispatchQueue.main.async { self?.report(error) } }
        }
    }

    /// Make the chosen application the default for every file of this type,
    /// after asking.
    @objc private func changeAll(_ sender: Any?) {
        guard let app = openWithPopup?.selectedItem?.representedObject as? URL, let item = items.first,
              let window = view.window,
              let type = UTType(filenameExtension: item.url.pathExtension)
        else { return }
        let name = FileManager.default.displayName(atPath: app.path)
        let alert = NSAlert()
        alert.messageText = "Open all “.\(item.url.pathExtension)” documents with “\(name)”?"
        alert.informativeText = "This changes the default application for every file of this kind."
        alert.addButton(withTitle: "Change All")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in
            guard response == .alertFirstButtonReturn else { return }
            NSWorkspace.shared.setDefaultApplication(at: app, toOpen: type) { error in
                if let error { DispatchQueue.main.async { self?.report(error) } }
            }
        }
    }

    // MARK: Finder comments

    /// The comment as Spotlight has it.
    static func finderComment(at url: URL) -> String {
        if let scripted = FinderComments.scriptedComment(at: url.path) { return scripted }
        guard let item = NSMetadataItem(url: url) else { return "" }
        return (item.value(forAttribute: kMDItemFinderComment as String) as? String) ?? ""
    }

    /// Finder keeps comments in its own store, so only Finder can set one
    /// that then shows in its Get Info. This asks it to.
    static func setFinderComment(_ comment: String, at url: URL, completion: @escaping (String?) -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            let message = FinderComments.set(comment, at: url.path)
            DispatchQueue.main.async { completion(message) }
        }
    }

    // MARK: Building blocks

    private func add(_ view: NSView) {
        stack.addArrangedSubview(view)
    }

    private func title(_ text: String) -> NSView {
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
        label.isSelectable = true
        return label
    }

    private func header(_ text: String) -> NSView {
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        return label
    }

    private func note(_ text: String) -> NSView {
        let label = NSTextField(labelWithString: text)
        label.textColor = .secondaryLabelColor
        return label
    }

    private func value(_ text: String) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        label.isSelectable = true
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return label
    }

    private func row(_ name: String, _ text: String) -> NSView {
        labelled(name, value(text))
    }

    /// A name on the left, a control or value on the right.
    private func labelled(_ name: String, _ control: NSView, fill: Bool = false) -> NSView {
        let label = NSTextField(labelWithString: name)
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        label.textColor = .secondaryLabelColor
        label.alignment = .right
        label.widthAnchor.constraint(equalToConstant: 86).isActive = true
        label.setContentHuggingPriority(.required, for: .horizontal)
        let line = NSStackView(views: [label, control])
        line.orientation = .horizontal
        line.alignment = .firstBaseline
        line.spacing = 8
        line.setAccessibilityLabel(name)
        if fill {
            control.setContentHuggingPriority(.defaultLow, for: .horizontal)
            DispatchQueue.main.async { [weak self, weak line] in
                guard let self, let line, line.superview === self.stack else { return }
                line.trailingAnchor.constraint(equalTo: self.stack.trailingAnchor, constant: -12).isActive = true
            }
        }
        return line
    }

    private func separator() -> NSView {
        let box = NSBox()
        box.boxType = .separator
        box.widthAnchor.constraint(greaterThanOrEqualToConstant: 120).isActive = true
        return box
    }

    private func checkbox(_ title: String, _ on: Bool, _ action: Selector) -> NSButton {
        let box = NSButton(checkboxWithTitle: title, target: self, action: action)
        box.state = on ? .on : .off
        box.controlSize = .small
        box.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        checkboxes.insert(ObjectIdentifier(box))
        return box
    }

    private func date(_ date: Date?) -> String {
        guard let date else { return "--" }
        return RowPresenter.shared.date(Int64(date.timeIntervalSince1970 * 1000))
    }

    private func report(_ error: Error) {
        reportMessage("The change could not be made.", error.localizedDescription)
    }

    private func reportMessage(_ title: String, _ detail: String) {
        guard let window = view.window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = title
        alert.informativeText = detail
        alert.beginSheetModal(for: window)
    }

    // MARK: Scripted checks

    /// Type into the Octal field and commit, as the user would.
    func debugSetOctal(_ text: String) {
        guard let field = octalField else { return }
        field.stringValue = text
        controlTextDidEndEditing(Notification(name: NSControl.textDidEndEditingNotification, object: field))
    }

    /// Type tags into the Tags field and leave it, as the user would.
    func debugSetTags(_ tags: [String]) {
        guard let field = tagField else { return }
        field.objectValue = tags
        controlTextDidEndEditing(Notification(name: NSControl.textDidEndEditingNotification, object: field))
    }

    /// "Label: value" for every row of the form, for the state dump.
    func debugDump() -> [String] {
        stack.arrangedSubviews.compactMap { view -> String? in
            guard let line = view as? NSStackView, line.arrangedSubviews.count == 2,
                  let label = line.arrangedSubviews[0] as? NSTextField
            else { return (view as? NSTextField).map { "info: \($0.stringValue)" } }
            return "info.\(label.stringValue): \(text(of: line.arrangedSubviews[1]))"
        }
    }

    private func text(of view: NSView) -> String {
        switch view {
        case let popup as NSPopUpButton: return popup.titleOfSelectedItem ?? ""
        case let token as NSTokenField: return ((token.objectValue as? [String]) ?? []).joined(separator: ",")
        case let field as NSTextField: return field.stringValue
        case let button as NSButton where checkboxes.contains(ObjectIdentifier(button)):
            return "\(button.state == .on ? "[x]" : "[ ]") \(button.title)"
        case let button as NSButton: return button.title
        case let stack as NSStackView: return stack.arrangedSubviews.map(text(of:)).joined(separator: " ")
        default: return ""
        }
    }
}

/// Clip view that lays its document out from the top, so a short form
/// starts at the top of the scroll view instead of the bottom.
final class FlippedClipView: NSClipView {
    override var isFlipped: Bool { true }
}

/// Type-specific details from Spotlight metadata.
@MainActor
enum MoreInfo {
    private static let keys: [(String, String)] = [
        ("Dimensions", "_dimensions"),
        ("Color space", kMDItemColorSpace as String),
        ("Color profile", kMDItemProfileName as String),
        ("Camera", kMDItemAcquisitionModel as String),
        ("Lens", kMDItemLensModel as String),
        ("Aperture", kMDItemAperture as String),
        ("Exposure time", kMDItemExposureTimeSeconds as String),
        ("Exposure program", kMDItemExposureProgram as String),
        ("F number", kMDItemFNumber as String),
        ("Focal length", kMDItemFocalLength as String),
        ("ISO speed", kMDItemISOSpeed as String),
        ("Flash", kMDItemFlashOnOff as String),
        ("Metering mode", kMDItemMeteringMode as String),
        ("White balance", kMDItemWhiteBalance as String),
        ("Latitude", kMDItemLatitude as String),
        ("Longitude", kMDItemLongitude as String),
        ("Duration", kMDItemDurationSeconds as String),
        ("Codecs", kMDItemCodecs as String),
        ("Sample rate", kMDItemAudioSampleRate as String),
        ("Bits per sample", kMDItemBitsPerSample as String),
        ("Audio channels", kMDItemAudioChannelCount as String),
        ("Title", kMDItemTitle as String),
        ("Album", kMDItemAlbum as String),
        ("Authors", kMDItemAuthors as String),
        ("Genre", kMDItemMusicalGenre as String),
        ("Track number", kMDItemAudioTrackNumber as String),
        ("Year recorded", kMDItemRecordingYear as String),
        ("Rating", kMDItemStarRating as String),
        ("Pages", kMDItemNumberOfPages as String),
        ("Encoding software", kMDItemEncodingApplications as String),
        ("Content created", kMDItemContentCreationDate as String),
    ]

    static func rows(for url: URL) -> [(String, String)] {
        guard let item = NSMetadataItem(url: url) else { return [] }
        return keys.compactMap { name, key in
            if key == "_dimensions" {
                // Rotated media can report negative sizes: always show positive.
                guard let w = item.value(forAttribute: kMDItemPixelWidth as String) as? Int,
                      let h = item.value(forAttribute: kMDItemPixelHeight as String) as? Int
                else { return nil }
                return (name, "\(abs(w)) × \(abs(h))")
            }
            guard let value = item.value(forAttribute: key) else { return nil }
            let text = format(value, key: key)
            return text.isEmpty ? nil : (name, text)
        }
    }

    private static func format(_ value: Any, key: String) -> String {
        switch value {
        case let list as [Any]: return list.map { "\($0)" }.joined(separator: ", ")
        case let date as Date: return RowPresenter.shared.date(Int64(date.timeIntervalSince1970 * 1000))
        case let number as NSNumber where key == kMDItemDurationSeconds as String:
            let seconds = Int(number.doubleValue.rounded())
            return seconds >= 3600
                ? String(format: "%d:%02d:%02d", seconds / 3600, seconds / 60 % 60, seconds % 60)
                : String(format: "%d:%02d", seconds / 60, seconds % 60)
        case let number as NSNumber where key == kMDItemFlashOnOff as String:
            return number.boolValue ? "On" : "Off"
        case let number as NSNumber:
            return number.doubleValue == number.doubleValue.rounded()
                ? "\(number.intValue)"
                : String(format: "%.4g", number.doubleValue)
        default: return "\(value)"
        }
    }
}
