// Regression check for the system-wide input lock after a drag out of the
// list view. At the end of every drag, AppKit sends draggingEnded: to each
// view that acted as a destination. If that throws, CoreDragEndDragging is
// abandoned and macOS thinks the drag never ended: Mission Control, Cmd-Tab
// and Spaces stay blocked until the app quits.
//
// Sends draggingEnded: to the app's own drag destination views, the same
// way AppKit does, and fails if anything throws or spring-loading isn't
// cancelled. Run with scripts/dnd-diag/check-dragging-ended.sh.
import AppKit

var failures = 0
func check(_ name: String, _ view: NSView, ended: () -> Bool) {
    if let e = RFTry({ RFSendDraggingEnded(view) }) {
        print("FAIL \(name): draggingEnded: threw \(e.name.rawValue): \(e.reason ?? "")")
        failures += 1
    } else if !ended() {
        print("FAIL \(name): draggingEnded: did not report the drag ending")
        failures += 1
    } else {
        print("PASS \(name)")
    }
}

_ = NSApplication.shared
var tableEnded = false
let table = FileTableView()
table.onDragExit = { tableEnded = true }
check("FileTableView (List view)", table) { tableEnded }

// FileGridView (Icon view) calls super.draggingEnded unconditionally; that
// is only safe while NSCollectionView implements it.
if NSCollectionView.instancesRespond(to: NSSelectorFromString("draggingEnded:")) {
    print("PASS FileGridView (Icon view): NSCollectionView implements draggingEnded:")
} else {
    print("FAIL FileGridView (Icon view): NSCollectionView no longer implements draggingEnded:")
    failures += 1
}

exit(failures == 0 ? 0 : 1)
