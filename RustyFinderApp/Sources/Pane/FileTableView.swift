import AppKit

@MainActor
protocol FileTableKeyHandler: AnyObject {
    // `view` is the list or the icon grid: both report to the same handler.
    /// Return true if the key was consumed.
    func fileTable(_ view: NSView, handleKeyDown event: NSEvent) -> Bool
    /// The view took keyboard focus (click or Tab).
    func fileTableDidBecomeFirstResponder(_ view: NSView)
    /// Undo or redo the window's last file operation.
    func fileTable(_ view: NSView, undo redo: Bool)
    /// Menu title for Undo/Redo ("Undo Move to Trash"), nil if unavailable.
    func fileTable(_ view: NSView, undoTitle redo: Bool) -> String?
}

/// Table view that offers key presses to its pane before AppKit handles
/// them, and reports when it gains focus so the pane can become active.
final class FileTableView: NSTableView {
    /// A drag left the view or ended (cancels spring-loading).
    var onDragExit: (() -> Void)?

    override func draggingExited(_ sender: NSDraggingInfo?) {
        super.draggingExited(sender)
        onDragExit?()
    }

    override func draggingEnded(_ sender: NSDraggingInfo) {
        // NSTableView has no draggingEnded: of its own (it is an optional
        // protocol method), yet Swift compiles the super call. Made anyway,
        // it throws "unrecognized selector" inside AppKit's drag completion,
        // which then never finishes: macOS keeps the drag alive and blocks
        // Mission Control, Cmd-Tab and Spaces until the app quits.
        if Self.superHasDraggingEnded { super.draggingEnded(sender) }
        onDragExit?()
    }

    private static let superHasDraggingEnded =
        NSTableView.instancesRespond(to: #selector(NSDraggingDestination.draggingEnded(_:)))

    weak var keyHandler: FileTableKeyHandler?

    override func keyDown(with event: NSEvent) {
        if keyHandler?.fileTable(self, handleKeyDown: event) == true { return }
        super.keyDown(with: event)
    }

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok { keyHandler?.fileTableDidBecomeFirstResponder(self) }
        return ok
    }

    /// A single click on the name of the only selected row, while the list
    /// already had focus: the start of a "slow second click" rename.
    var onSlowClick: ((Int) -> Void)?
    /// Any other click, which cancels a pending slow-click rename.
    var onOtherClick: (() -> Void)?

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let row = self.row(at: point)
        let column = self.column(at: point)
        let wasOnlySelected = row >= 0 && selectedRowIndexes == [row]
        let hadFocus = window?.firstResponder === self
        DragTrace.note("table mouseDown enter clicks=\(event.clickCount)")
        super.mouseDown(with: event)
        DragTrace.note("table mouseDown exit \(DragTrace.buttons)")
        let onName = column >= 0 && tableColumns[column].identifier.rawValue == "name"
        if event.clickCount == 1, wasOnlySelected, hadFocus, onName, event.modifierFlags.intersection([.command, .shift, .option, .control]).isEmpty {
            onSlowClick?(row)
        } else {
            onOtherClick?()
        }
    }

    // Undo and Redo use the standard selectors, answered here. That way
    // Cmd-Z undoes a file operation while the list has focus, and ordinary
    // text undo while a text field does (the field's editor answers first).
    static let undoAction = Selector(("undo:"))
    static let redoAction = Selector(("redo:"))

    /// True while a text field inside the list (inline rename) is being
    /// edited. Undo then belongs to the typing, not to file operations:
    /// the action still passes through here on its way up from the editor.
    private var isEditingText: Bool {
        window?.firstResponder !== self
    }

    @objc func undo(_ sender: Any?) {
        if isEditingText {
            window?.undoManager?.undo()
        } else {
            keyHandler?.fileTable(self, undo: false)
        }
    }

    @objc func redo(_ sender: Any?) {
        if isEditingText {
            window?.undoManager?.redo()
        } else {
            keyHandler?.fileTable(self, undo: true)
        }
    }

    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        let redo: Bool
        switch item.action {
        case Self.undoAction: redo = false
        case Self.redoAction: redo = true
        default: return super.validateUserInterfaceItem(item)
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
