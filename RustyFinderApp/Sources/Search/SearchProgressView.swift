import AppKit

/// Shown in a pane's status line while a search runs: a spinner and a Stop
/// button. Stop ends the search early and keeps what it found.
final class SearchProgressView: NSStackView {
    private let spinner = NSProgressIndicator()
    private let stopButton = NSButton()
    var onStop: (() -> Void)?

    init() {
        super.init(frame: .zero)
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        stopButton.image = NSImage(systemSymbolName: "stop.circle.fill", accessibilityDescription: "Stop Search")
        stopButton.isBordered = false
        stopButton.toolTip = "Stop Search"
        stopButton.setAccessibilityLabel("Stop Search")
        stopButton.target = self
        stopButton.action = #selector(stop(_:))
        orientation = .horizontal
        spacing = 4
        addArrangedSubview(spinner)
        addArrangedSubview(stopButton)
        translatesAutoresizingMaskIntoConstraints = false
        isHidden = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    var isRunning: Bool { !isHidden }

    func update(running: Bool) {
        guard running != isRunning else { return }
        isHidden = !running
        if running { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
    }

    @objc private func stop(_ sender: Any?) {
        onStop?()
    }

    func debugStop() { stop(nil) }
}

extension FileListViewController {
    /// End the running search early; the results found so far stay.
    func stopSearch() {
        guard search?.running == true else { return }
        session.stopSearch(tab: tabId)
    }
}
