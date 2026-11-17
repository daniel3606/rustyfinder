import AppKit
import PDFKit

/// PDFs are previewed in a PDF view whose context menu offers the display
/// options (single page or continuous, two pages, zoom). What the user
/// picks is remembered across items and launches.
@MainActor
enum PDFDisplayOptions {
    private static let key = "preview.pdf"
    /// Set while options are being applied, so that is not taken for a choice.
    private static var applying = false
    private static var observing = false

    static func makeView() -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displaysPageBreaks = true
        view.setAccessibilityLabel("PDF preview")
        return view
    }

    /// Show `url` with the remembered options.
    static func show(_ url: URL, in view: PDFView) -> Bool {
        guard let document = PDFDocument(url: url) else { return false }
        applying = true
        view.document = document
        let saved = Prefs.strings(key) ?? []
        if let raw = saved.first.flatMap(Int.init), let mode = PDFDisplayMode(rawValue: raw) {
            view.displayMode = mode
        }
        if saved.count > 1, let scale = Double(saved[1]) {
            view.autoScales = false
            view.scaleFactor = CGFloat(scale)
        } else {
            view.autoScales = true
        }
        applying = false
        return true
    }

    /// Remember display mode and zoom whenever the user changes them.
    static func observe() {
        guard !observing else { return }
        observing = true
        for name in [Notification.Name.PDFViewDisplayModeChanged, .PDFViewScaleChanged] {
            NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { note in
                MainActor.assumeIsolated {
                    guard !applying, let view = note.object as? PDFView, view.document != nil else { return }
                    remember(view)
                }
            }
        }
    }

    static func remember(_ view: PDFView) {
        let scale = view.autoScales ? [] : [String(Double(view.scaleFactor))]
        Prefs.setStrings(key, [String(view.displayMode.rawValue)] + scale)
    }

    /// "mode=<n> scale=<auto|x>" (scripted checks).
    static func describe(_ view: PDFView) -> String {
        "mode=\(view.displayMode.rawValue) scale=\(view.autoScales ? "auto" : String(format: "%.2f", view.scaleFactor))"
    }
}
