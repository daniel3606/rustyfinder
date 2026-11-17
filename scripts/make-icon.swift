// Draws the app icon: a black, two-tone smiling face in the macOS icon shape.
//
//   swift scripts/make-icon.swift            writes the AppIcon asset set
//   swift scripts/make-icon.swift out.png    writes only a 1024 px preview
import AppKit

let size: CGFloat = 1024

func render(_ px: Int) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px,
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let cg = NSGraphicsContext.current!.cgContext
    // Work in a 1024 canvas with y pointing down, whatever the output size.
    cg.scaleBy(x: CGFloat(px) / size, y: CGFloat(px) / size)
    cg.translateBy(x: 0, y: size)
    cg.scaleBy(x: 1, y: -1)
    draw(cg)
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

func color(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: a)
}

func gradient(_ cg: CGContext, _ top: UInt32, _ bottom: UInt32, _ rect: CGRect) {
    let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB),
                       colors: [color(top), color(bottom)] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(g, start: CGPoint(x: 0, y: rect.minY), end: CGPoint(x: 0, y: rect.maxY), options: [])
}

func draw(_ cg: CGContext) {
    // Apple's icon grid: an 824 pt rounded square centred on the 1024 canvas.
    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)

    cg.saveGState()
    cg.setShadow(offset: CGSize(width: 0, height: 12), blur: 28, color: color(0x000000, 0.35))
    cg.addPath(shape)
    cg.setFillColor(color(0x000000))
    cg.fillPath()
    cg.restoreGState()

    cg.saveGState()
    cg.addPath(shape)
    cg.clip()

    // The profile line that splits the face: down from the top, out into a
    // nose, back in, then straight to the bottom.
    let profile = CGMutablePath()
    profile.move(to: CGPoint(x: 560, y: 90))
    profile.addLine(to: CGPoint(x: 545, y: 420))
    profile.addCurve(to: CGPoint(x: 445, y: 585), control1: CGPoint(x: 520, y: 500), control2: CGPoint(x: 440, y: 540))
    profile.addCurve(to: CGPoint(x: 520, y: 620), control1: CGPoint(x: 450, y: 615), control2: CGPoint(x: 495, y: 618))
    profile.addLine(to: CGPoint(x: 505, y: 934))

    // Left half: charcoal. Right half: near black.
    let left = CGMutablePath()
    left.addPath(profile)
    left.addLine(to: CGPoint(x: 90, y: 934))
    left.addLine(to: CGPoint(x: 90, y: 90))
    left.closeSubpath()

    gradient(cg, 0x141416, 0x000000, tile)
    cg.saveGState()
    cg.addPath(left)
    cg.clip()
    gradient(cg, 0x4a4a4e, 0x2a2a2d, tile)
    cg.restoreGState()

    let ink = color(0xf2f2f4)
    cg.setStrokeColor(ink)
    cg.setLineCap(.round)
    cg.setLineJoin(.round)

    cg.setLineWidth(16)
    cg.addPath(profile)
    cg.strokePath()

    // Eyes.
    cg.setLineWidth(46)
    for x in [330.0, 700.0] {
        cg.move(to: CGPoint(x: x, y: 330))
        cg.addLine(to: CGPoint(x: x, y: 430))
    }
    cg.strokePath()

    // Smile, across both halves.
    cg.setLineWidth(22)
    cg.move(to: CGPoint(x: 270, y: 680))
    cg.addCurve(to: CGPoint(x: 760, y: 680), control1: CGPoint(x: 400, y: 790), control2: CGPoint(x: 630, y: 790))
    cg.strokePath()

    // Soft sheen on the top half, and a thin rim so it reads on a dark Dock.
    cg.addRect(CGRect(x: 100, y: 100, width: 824, height: 412))
    cg.clip()
    let sheen = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB),
                           colors: [color(0xffffff, 0.10), color(0xffffff, 0)] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(sheen, start: CGPoint(x: 0, y: 100), end: CGPoint(x: 0, y: 512), options: [])
    cg.restoreGState()

    cg.addPath(shape)
    cg.setStrokeColor(color(0xffffff, 0.12))
    cg.setLineWidth(3)
    cg.strokePath()
}

let args = CommandLine.arguments
if args.count > 1 {
    try! render(1024).write(to: URL(fileURLWithPath: args[1]))
    exit(0)
}

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let set = root.appendingPathComponent("RustyFinderApp/Resources/Assets.xcassets/AppIcon.appiconset")
try! FileManager.default.createDirectory(at: set, withIntermediateDirectories: true)

var images: [String] = []
for pt in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let name = "icon_\(pt)x\(pt)\(scale == 2 ? "@2x" : "").png"
        try! render(pt * scale).write(to: set.appendingPathComponent(name))
        images.append(#"    { "idiom" : "mac", "size" : "\#(pt)x\#(pt)", "scale" : "\#(scale)x", "filename" : "\#(name)" }"#)
    }
}
let contents = "{\n  \"images\" : [\n" + images.joined(separator: ",\n")
    + "\n  ],\n  \"info\" : { \"author\" : \"xcode\", \"version\" : 1 }\n}\n"
try! contents.write(to: set.appendingPathComponent("Contents.json"), atomically: true, encoding: .utf8)
try! "{\n  \"info\" : { \"author\" : \"xcode\", \"version\" : 1 }\n}\n"
    .write(to: set.deletingLastPathComponent().appendingPathComponent("Contents.json"), atomically: true, encoding: .utf8)
print("wrote \(set.path)")
