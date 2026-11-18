// Draws the DMG window background: black, the RustyFinder wordmark, an
// arrow from the app to Applications and one line of instructions.
//
//   swift packaging/dmg/make-background.swift <out-dir>
//
// Writes background.png (660x400) and background@2x.png; dmgbuild joins
// them into one HiDPI TIFF. Item positions in settings.py must match the
// icon centres here (app at x=165, Applications at x=495, both at y=195).
//
// Finder draws icon labels in black on any picture background, even in
// Dark Mode, so on black they vanish. Visible labels are painted here,
// just below the band where Finder draws its own (small, unseen) ones, so
// the two never overlap. Labels on the right would avoid that too, but
// make Finder add a horizontal scroll bar.
import AppKit

let width: CGFloat = 660
let height: CGFloat = 400

func color(_ hex: UInt32, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: a)
}

func centered(_ text: String, font: NSFont, color: NSColor, y: CGFloat, x: CGFloat = width / 2,
              kern: CGFloat = 0) {
    let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .kern: kern]
    let s = NSAttributedString(string: text, attributes: attrs)
    let size = s.size()
    s.draw(at: NSPoint(x: x - size.width / 2, y: y - size.height / 2))
}

func draw() {
    color(0x000000).setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()

    // AppKit's origin is bottom-left; positions below are from the top.
    func top(_ y: CGFloat) -> CGFloat { height - y }

    centered("RustyFinder", font: .systemFont(ofSize: 30, weight: .semibold),
             color: color(0xf5f5f7), y: top(62), kern: -0.3)
    centered("A fast, native file manager for macOS", font: .systemFont(ofSize: 13, weight: .regular),
             color: color(0x86868b), y: top(94))

    // Arrow centred between the two 128 pt icons (x=165 and x=495).
    let y = top(195)
    let arrow = NSBezierPath()
    arrow.move(to: NSPoint(x: 272, y: y))
    arrow.line(to: NSPoint(x: 388, y: y))
    arrow.move(to: NSPoint(x: 377, y: y + 10))
    arrow.line(to: NSPoint(x: 388, y: y))
    arrow.line(to: NSPoint(x: 377, y: y - 10))
    arrow.lineWidth = 2
    arrow.lineCapStyle = .round
    arrow.lineJoinStyle = .round
    color(0x48484a).setStroke()
    arrow.stroke()

    let label = NSFont.systemFont(ofSize: 13, weight: .medium)
    centered("RustyFinder", font: label, color: color(0xe5e5ea), y: top(294), x: 165)
    centered("Applications", font: label, color: color(0xe5e5ea), y: top(294), x: 495)

    centered("Drag RustyFinder to Applications to install", font: .systemFont(ofSize: 13, weight: .medium),
             color: color(0x86868b), y: top(352))
}

func write(scale: CGFloat, to url: URL) throws {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(width * scale),
                               pixelsHigh: Int(height * scale), bitsPerSample: 8, samplesPerPixel: 4,
                               hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    // Points, not pixels: the @2x file says it is 660x400 pt at 144 dpi.
    rep.size = NSSize(width: width, height: height)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    draw()
    NSGraphicsContext.restoreGraphicsState()
    try rep.representation(using: .png, properties: [:])!.write(to: url)
}

let args = CommandLine.arguments
guard args.count == 2 else {
    FileHandle.standardError.write("usage: make-background.swift <out-dir>\n".data(using: .utf8)!)
    exit(2)
}
let dir = URL(fileURLWithPath: args[1], isDirectory: true)
try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
try write(scale: 1, to: dir.appendingPathComponent("background.png"))
try write(scale: 2, to: dir.appendingPathComponent("background@2x.png"))
