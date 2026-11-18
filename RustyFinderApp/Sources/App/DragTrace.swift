import AppKit
import os

/// Diagnostics for drags out of the app. Off unless the app is started
/// with RF_DRAG_TRACE=1 in its environment; then it logs to the unified
/// log (subsystem dev.rustyfinder.dragtrace): drag begin and end, what the
/// dragged items put on the pasteboard, app and Space changes, the mouse
/// button state the window server reports, and main-thread stalls.
///
///   log stream --level info --predicate 'subsystem == "dev.rustyfinder.dragtrace"'
enum DragTrace {
    nonisolated static let enabled = ProcessInfo.processInfo.environment["RF_DRAG_TRACE"] == "1"
    nonisolated private static let log = Logger(subsystem: "dev.rustyfinder.dragtrace", category: "drag")

    nonisolated static func note(_ message: @autoclosure () -> String) {
        guard enabled else { return }
        let text = message()
        log.notice("\(text, privacy: .public)")
    }

    /// The main run loop's current mode (default, event tracking, modal…).
    nonisolated static var mainMode: String {
        (CFRunLoopCopyCurrentMode(CFRunLoopGetMain()).map { $0.rawValue as String }) ?? "none"
    }

    nonisolated static var buttons: String {
        let session = CGEventSource.buttonState(.combinedSessionState, button: .left)
        let hid = CGEventSource.buttonState(.hidSystemState, button: .left)
        return "leftDown session=\(session) hid=\(hid) pressedMouseButtons=\(NSEvent.pressedMouseButtons)"
    }

    /// A pasteboard writer about to be dragged: its types and whether any
    /// of them is lazy (promised), which would call back into the app later.
    nonisolated static func writer(_ writer: NSPasteboardWriting?, row: Int) {
        guard enabled, let writer else { return }
        let pasteboard = NSPasteboard(name: .drag)
        let types = writer.writableTypes(for: pasteboard)
        let lazy = types.filter { writer.writingOptions?(forType: $0, pasteboard: pasteboard).contains(.promised) == true }
        note("writer row=\(row) class=\(type(of: writer)) types=\(types.map(\.rawValue)) promised=\(lazy.map(\.rawValue))")
    }

    @MainActor static func start() {
        guard enabled else { return }
        note("trace on, pid \(getpid())")
        let workspace = NSWorkspace.shared.notificationCenter
        workspace.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: nil) { _ in
            note("activeSpaceDidChange mode=\(mainMode) \(buttons)")
        }
        workspace.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: nil) { n in
            let app = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            note("didActivateApplication \(app?.localizedName ?? "?") mode=\(mainMode)")
        }
        let center = NotificationCenter.default
        for name in [NSApplication.didBecomeActiveNotification, NSApplication.didResignActiveNotification] {
            center.addObserver(forName: name, object: nil, queue: nil) { n in note("\(n.name.rawValue) mode=\(mainMode)") }
        }
        // Main-thread watchdog: a background thread pings the main queue
        // every 100 ms and reports when an answer is late, with the mode the
        // main run loop is in. Also reports mouse-button state changes.
        let thread = Thread {
            var lastAnswer = Date()
            var stalled = false
            var lastButtons = ""
            let lock = NSLock()
            while true {
                Thread.sleep(forTimeInterval: 0.1)
                DispatchQueue.main.async { lock.lock(); lastAnswer = Date(); lock.unlock() }
                lock.lock()
                let late = Date().timeIntervalSince(lastAnswer)
                lock.unlock()
                if late > 0.5, !stalled {
                    stalled = true
                    note("MAIN STALLED >500ms mode=\(mainMode) \(buttons)")
                } else if late < 0.2, stalled {
                    stalled = false
                    note("main answering again mode=\(mainMode)")
                }
                let now = buttons
                if now != lastButtons {
                    note("buttons: \(now) mode=\(mainMode)")
                    lastButtons = now
                }
            }
        }
        thread.name = "rustyfinder-dragtrace"
        thread.start()
    }
}
