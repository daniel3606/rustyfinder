import AppKit

/// Spring-loaded folders: pause while dragging over a folder and it opens
/// in this pane, so the drag can carry on inside it. Dropping without the
/// pause just transfers into the folder. Follows the system's own setting
/// and delay (Accessibility › Pointer Control › Spring-loading).
extension FileListViewController {
    static var springLoadingEnabled: Bool {
        UserDefaults.standard.object(forKey: "com.apple.springing.enabled") as? Bool ?? true
    }

    static var springDelay: TimeInterval {
        let delay = UserDefaults.standard.double(forKey: "com.apple.springing.delay")
        return delay > 0 ? delay : 0.5
    }

    /// The drag is over `folder` (nil: over no folder). Starts or restarts
    /// the countdown when the folder changes.
    func springHover(over folder: String?, force: Bool = false) {
        guard folder != springTarget else { return }
        springTimer?.invalidate()
        springTimer = nil
        springTarget = folder
        guard let folder, force || Self.springLoadingEnabled else { return }
        springTimer = Timer.scheduledTimer(withTimeInterval: Self.springDelay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.springTarget == folder else { return }
                self.springTarget = nil
                self.springTimer = nil
                NSSound(named: "Pop")?.play()
                self.session.navigate(tab: self.tabId, path: folder)
            }
        }
    }

    /// The drag left, ended or dropped.
    func springCancel() {
        springTimer?.invalidate()
        springTimer = nil
        springTarget = nil
    }
}
