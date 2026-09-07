// Draws the disk image window's background: the arrow and the instruction that
// tell a first-time user what the two icons Finder draws on top are for.
//
// Rendered at both scales because a disk image background is a fixed bitmap and
// nothing scales it: a 1x-only image is visibly soft on every display Apple has
// shipped for a decade.

import AppKit

let width: CGFloat = 660
let height: CGFloat = 400

// Finder places icons from the top left; AppKit draws from the bottom left.
func flipped(_ y: CGFloat) -> CGFloat { height - y }

let iconCentreY: CGFloat = 155
let appIconX: CGFloat = 165
let applicationsIconX: CGFloat = 495

func drawBackground() {
    // A quiet vertical wash rather than flat white: it gives the icons a ground
    // to sit on without competing with them.
    let gradient = NSGradient(
        colors: [
            NSColor(calibratedRed: 0.98, green: 0.98, blue: 0.99, alpha: 1),
            NSColor(calibratedRed: 0.91, green: 0.92, blue: 0.94, alpha: 1),
        ]
    )!
    gradient.draw(in: NSRect(x: 0, y: 0, width: width, height: height), angle: -90)
}

func drawArrow() {
    let y = flipped(iconCentreY)
    // Stops short of both icons so it reads as pointing between them rather
    // than touching either.
    let start = appIconX + 85
    let end = applicationsIconX - 85
    let headLength: CGFloat = 26
    let headHalfHeight: CGFloat = 15

    let ink = NSColor(calibratedRed: 0.45, green: 0.47, blue: 0.52, alpha: 0.85)
    ink.setStroke()
    ink.setFill()

    let shaft = NSBezierPath()
    shaft.move(to: NSPoint(x: start, y: y))
    shaft.line(to: NSPoint(x: end - headLength + 4, y: y))
    shaft.lineWidth = 7
    shaft.lineCapStyle = .round
    shaft.stroke()

    let head = NSBezierPath()
    head.move(to: NSPoint(x: end, y: y))
    head.line(to: NSPoint(x: end - headLength, y: y + headHalfHeight))
    head.line(to: NSPoint(x: end - headLength, y: y - headHalfHeight))
    head.close()
    head.fill()
}

func drawInstruction() {
    let text = "Drag NiumaTerm into Applications to install"
    let style = NSMutableParagraphStyle()
    style.alignment = .center

    let attributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: 15, weight: .medium),
        .foregroundColor: NSColor(calibratedRed: 0.33, green: 0.35, blue: 0.40, alpha: 1),
        .paragraphStyle: style,
    ]

    // Below the icon labels Finder draws, which sit just under the icons.
    let box = NSRect(x: 0, y: flipped(285), width: width, height: 24)
    (text as NSString).draw(in: box, withAttributes: attributes)
}

func render(scale: CGFloat, to path: String) {
    guard
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: Int(width * scale),
            pixelsHigh: Int(height * scale),
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        )
    else {
        FileHandle.standardError.write(Data("could not create the bitmap\n".utf8))
        exit(1)
    }
    // Declaring the point size is what makes every coordinate below scale-free.
    rep.size = NSSize(width: width, height: height)

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    drawBackground()
    drawArrow()
    drawInstruction()
    NSGraphicsContext.restoreGraphicsState()

    guard let png = rep.representation(using: .png, properties: [:]) else {
        FileHandle.standardError.write(Data("could not encode the png\n".utf8))
        exit(1)
    }
    try! png.write(to: URL(fileURLWithPath: path))
    print("wrote \(path) at \(Int(width * scale))x\(Int(height * scale))")
}

let outputDirectory = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "."
render(scale: 1, to: "\(outputDirectory)/dmg-background.png")
render(scale: 2, to: "\(outputDirectory)/dmg-background@2x.png")
