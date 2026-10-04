import SwiftUI
import UIKit

import NiumaTermCore

/// Cell geometry for one font and size.
struct TerminalMetrics: Equatable {
    let regular: UIFont
    let bold: UIFont
    let italic: UIFont
    let boldItalic: UIFont

    /// The font's advance, unrounded: runs of narrow cells are drawn as one
    /// string, which only lines up with the grid at the exact advance.
    let cellWidth: CGFloat

    let cellHeight: CGFloat

    /// From the top of a cell to the top of the text drawn in it.
    let textOffset: CGFloat

    /// Horizontal padding on each side of the grid.
    static let inset: CGFloat = 6

    init(size: CGFloat, name: String, scale: CGFloat) {
        let fallback = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
        let base = name == "SF Mono" ? fallback : (UIFont(name: Theme.jetBrainsName, size: size) ?? fallback)

        func styled(_ traits: UIFontDescriptor.SymbolicTraits) -> UIFont {
            base.fontDescriptor.withSymbolicTraits(traits).map { UIFont(descriptor: $0, size: size) } ?? base
        }

        regular = base
        bold = styled(.traitBold)
        italic = styled(.traitItalic)
        boldItalic = styled([.traitBold, .traitItalic])
        cellWidth = ("M" as NSString).size(withAttributes: [.font: base]).width
        cellHeight = ceil(base.lineHeight * 1.1 * scale) / scale
        textOffset = ((cellHeight - base.lineHeight) / 2 * scale).rounded() / scale
    }

    func font(bold isBold: Bool, italic isItalic: Bool) -> UIFont {
        switch (isBold, isItalic) {
        case (false, false): regular
        case (true, false): bold
        case (false, true): italic
        case (true, true): boldItalic
        }
    }

    func grid(for size: CGSize) -> (cols: Int, rows: Int) {
        (max(1, Int((size.width - 2 * Self.inset) / cellWidth)), max(1, Int(size.height / cellHeight)))
    }

    /// A grid for the window before the screen lays out, so a terminal
    /// opened from here starts close to the size it will claim.
    @MainActor
    static func estimatedGrid() -> (cols: Int, rows: Int) {
        let size = UserDefaults.standard.object(forKey: "terminalFontSize") as? Double ?? 11
        let name = UserDefaults.standard.string(forKey: "terminalFontName") ?? "JetBrains Mono"

        let screen = UIApplication.shared.connectedScenes
            .compactMap { ($0 as? UIWindowScene)?.screen }
            .first

        let bounds = screen?.bounds.size ?? CGSize(width: 390, height: 844)
        let metrics = TerminalMetrics(size: size, name: name, scale: screen?.scale ?? 3)

        // Leave room for the navigation bar and the accessory bar.
        return metrics.grid(for: CGSize(width: bounds.width, height: bounds.height - 200))
    }
}

/// Draws a terminal grid with Core Text, redrawing only the rows each frame
/// changed (design doc §8.2), and takes touch and keyboard input for it.
final class TerminalSurface: UIView {
    let model: TerminalSessionModel
    var onFontSize: ((Double) -> Void)?

    var metrics: TerminalMetrics {
        didSet {
            guard metrics != oldValue else { return }

            setNeedsLayout()
            setNeedsDisplay()
        }
    }

    private(set) var lines: [[TerminalRun]] = []
    private(set) var cursor: TerminalCursor?
    private var gridCols = 0
    private var selection: TerminalSelection?
    private var foreground = UIColor.black
    private var background = UIColor.white
    private var colors: [UInt32: UIColor] = [:]

    /// The grid this view last claimed for the PTY.
    private var claimed: (cols: Int, rows: Int)?

    private var displayLink: CADisplayLink?
    private var needsFrame = true

    // Gesture state.
    private var scrollRemainder: CGFloat = 0

    private var pinchBase: Double?
    private lazy var editMenu = UIEditMenuInteraction(delegate: self)

    // Input method state, read by the `UITextInput` conformance.
    var markedText = ""

    var markedSelection = NSRange(location: 0, length: 0)
    weak var inputDelegate: UITextInputDelegate?
    lazy var tokenizer: UITextInputTokenizer = UITextInputStringTokenizer(textInput: self)
    var markedTextStyle: [NSAttributedString.Key: Any]?
    var repeatTimer: Timer?

    init(model: TerminalSessionModel, metrics: TerminalMetrics) {
        self.model = model
        self.metrics = metrics

        super.init(frame: .zero)

        isOpaque = true
        contentMode = .topLeft
        isMultipleTouchEnabled = true

        let tap = UITapGestureRecognizer(target: self, action: #selector(tapped(_:)))
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))

        pan.maximumNumberOfTouches = 1

        let press = UILongPressGestureRecognizer(target: self, action: #selector(pressed(_:)))
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(pinched(_:)))

        for recognizer in [tap, pan, press, pinch] as [UIGestureRecognizer] {
            addGestureRecognizer(recognizer)
        }

        addInteraction(editMenu)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    // MARK: Frames

    /// Pull a frame on the next display refresh. Several calls before then
    /// cost one frame.
    func setNeedsFrame() {
        needsFrame = true
        displayLink?.isPaused = false
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        displayLink?.invalidate()

        displayLink = nil

        guard window != nil else { return }

        let link = CADisplayLink(target: DisplayLinkTarget(self), selector: #selector(DisplayLinkTarget.tick))

        link.add(to: .main, forMode: .common)

        displayLink = link
        link.isPaused = !needsFrame

        setNeedsLayout()
    }

    func tick() {
        guard needsFrame else {
            displayLink?.isPaused = true

            return
        }

        needsFrame = false
        displayLink?.isPaused = true

        if let frame = model.nextFrame() {
            apply(frame)
        }
    }

    private func apply(_ frame: TerminalFrame) {
        let rowCount = Int(frame.rows)
        let resized = rowCount != lines.count || Int(frame.cols) != gridCols

        var redrawAll = frame.full || resized

        if redrawAll {
            lines = Array(repeating: [], count: rowCount)
            gridCols = Int(frame.cols)
        }

        for line in frame.lines where Int(line.row) < rowCount {
            lines[Int(line.row)] = line.runs

            if !redrawAll { setNeedsDisplay(rowRect(Int(line.row))) }
        }

        let fg = color(frame.foreground)
        let bg = color(frame.background)

        if fg != foreground || bg != background {
            foreground = fg
            background = bg
            backgroundColor = bg
            redrawAll = true
        }

        if frame.selection != selection {
            selection = frame.selection
            redrawAll = true
        }

        if frame.cursor != cursor {
            if let old = cursor { setNeedsDisplay(rowRect(Int(old.row))) }

            cursor = frame.cursor

            if let new = cursor { setNeedsDisplay(rowRect(Int(new.row))) }
        }

        if redrawAll { setNeedsDisplay() }
    }

    func color(_ rgb: UInt32) -> UIColor {
        if let known = colors[rgb] { return known }

        let made = UIColor(red: CGFloat((rgb >> 16) & 0xFF) / 255,
                           green: CGFloat((rgb >> 8) & 0xFF) / 255,
                           blue: CGFloat(rgb & 0xFF) / 255, alpha: 1)

        colors[rgb] = made

        return made
    }

    // MARK: Layout

    override func layoutSubviews() {
        super.layoutSubviews()

        guard window != nil, bounds.width > 0, bounds.height > 0 else { return }

        let grid = metrics.grid(for: bounds.size)

        guard claimed.map({ $0 != grid }) ?? true else { return }

        claimed = grid

        let scale = traitCollection.displayScale

        model.resize(cols: grid.cols, rows: grid.rows,
                     width: Int(bounds.width * scale), height: Int(bounds.height * scale))

        setNeedsDisplay()
    }

    func rowRect(_ row: Int) -> CGRect {
        CGRect(x: 0, y: CGFloat(row) * metrics.cellHeight, width: bounds.width, height: metrics.cellHeight)
    }

    /// Cell edges snapped to device pixels, so adjacent backgrounds meet
    /// with no hairline gap between them.
    func cellRect(col: Int, cells: Int, row: Int) -> CGRect {
        let scale = traitCollection.displayScale

        func snap(_ value: CGFloat) -> CGFloat { (value * scale).rounded() / scale }

        let x0 = snap(TerminalMetrics.inset + CGFloat(col) * metrics.cellWidth)
        let x1 = snap(TerminalMetrics.inset + CGFloat(col + cells) * metrics.cellWidth)

        return CGRect(x: x0, y: CGFloat(row) * metrics.cellHeight, width: x1 - x0, height: metrics.cellHeight)
    }

    func cell(at point: CGPoint) -> (col: UInt16, row: UInt16) {
        let col = Int((point.x - TerminalMetrics.inset) / metrics.cellWidth)
        let row = Int(point.y / metrics.cellHeight)

        return (UInt16(clamping: min(max(col, 0), max(gridCols - 1, 0))),
                UInt16(clamping: min(max(row, 0), max(lines.count - 1, 0))))
    }

    // MARK: Drawing

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext() else { return }

        context.setFillColor(background.cgColor)
        context.fill(rect)

        let first = max(0, Int(rect.minY / metrics.cellHeight))
        let last = min(lines.count, Int((rect.maxY / metrics.cellHeight).rounded(.up)))

        guard first < last else { return }

        for row in first..<last {
            for run in lines[row] {
                guard let bg = run.bg else { continue }

                context.setFillColor(color(bg).cgColor)
                context.fill(cellRect(col: Int(run.col), cells: Int(run.cells), row: row))
            }

            drawSelection(row: row, in: context)

            for run in lines[row] {
                drawText(run, row: row, color: nil)
            }

            if let cursor, Int(cursor.row) == row {
                drawCursor(cursor, in: context)
            }
        }
    }

    private func drawText(_ run: TerminalRun, row: Int, color override: UIColor?) {
        var attributes: [NSAttributedString.Key: Any] = [
            .font: metrics.font(bold: run.bold, italic: run.italic),
            .foregroundColor: override ?? color(run.fg),
        ]

        switch run.underline {
        case .none: break
        case .double: attributes[.underlineStyle] = NSUnderlineStyle.double.rawValue
        case .dotted: attributes[.underlineStyle] = NSUnderlineStyle([.single, .patternDot]).rawValue
        case .dashed: attributes[.underlineStyle] = NSUnderlineStyle([.single, .patternDash]).rawValue
        case .single, .curly: attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue
        }

        if run.strikeout {
            attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
        }

        let text = run.text as NSString

        var x = TerminalMetrics.inset + CGFloat(run.col) * metrics.cellWidth

        // A double-width glyph from a fallback font rarely matches two cells
        // exactly; centering keeps it inside them.
        if run.cells == 2 && run.text.count == 1 {
            let width = text.size(withAttributes: attributes).width

            x += (2 * metrics.cellWidth - width) / 2
        }

        text.draw(at: CGPoint(x: x, y: CGFloat(row) * metrics.cellHeight + metrics.textOffset),
                  withAttributes: attributes)
    }

    private func drawSelection(row: Int, in context: CGContext) {
        guard let selection, selection.startRow <= row, row <= selection.endRow else { return }

        let start: Int
        let end: Int

        if selection.block {
            start = Int(selection.startCol)
            end = Int(selection.endCol)
        } else {
            start = row == selection.startRow ? Int(selection.startCol) : 0
            end = row == selection.endRow ? Int(selection.endCol) : gridCols - 1
        }

        guard start <= end else { return }

        context.setFillColor(UIColor(Theme.accent).withAlphaComponent(0.3).cgColor)
        context.fill(cellRect(col: start, cells: end - start + 1, row: row))
    }

    private func drawCursor(_ cursor: TerminalCursor, in context: CGContext) {
        let row = Int(cursor.row)
        let under = run(at: Int(cursor.col), row: row)
        // The cell under the cursor: two cells for a double-width glyph,
        // which starts at its run.
        let wide = under.map { $0.run.cells == 2 && $0.run.text.count == 1 } ?? false
        let rect = cellRect(col: Int(under?.col ?? cursor.col), cells: wide ? 2 : 1, row: row)

        // Text an input method is composing shows at the cursor until it is
        // committed; only then does it reach the host.
        if !markedText.isEmpty {
            let attributes: [NSAttributedString.Key: Any] = [
                .font: metrics.regular,
                .foregroundColor: foreground,
                .backgroundColor: background,
                .underlineStyle: NSUnderlineStyle.single.rawValue,
            ]

            (markedText as NSString).draw(at: CGPoint(x: rect.minX, y: rect.minY + metrics.textOffset),
                                          withAttributes: attributes)

            return
        }

        context.setFillColor(foreground.cgColor)

        switch cursor.shape {
        case .block where isFirstResponder:
            context.fill(rect)

            if let under {
                let glyph = TerminalRun(col: under.col, cells: wide ? 2 : 1,
                                        text: under.text, fg: under.run.fg, bg: nil,
                                        bold: under.run.bold, italic: under.run.italic,
                                        underline: .none, strikeout: false)

                drawText(glyph, row: row, color: background)
            }
        case .block:
            context.setStrokeColor(foreground.cgColor)
            context.stroke(rect.insetBy(dx: 0.5, dy: 0.5), width: 1)
        case .underline:
            context.fill(CGRect(x: rect.minX, y: rect.maxY - 2, width: rect.width, height: 2))
        case .beam:
            context.fill(CGRect(x: rect.minX, y: rect.minY, width: 2, height: rect.height))
        }
    }

    /// The run covering a cell, with that cell's text. Every character of
    /// a narrow run takes one cell, so the text is found by position.
    private func run(at col: Int, row: Int) -> (run: TerminalRun, col: UInt16, text: String)? {
        guard row < lines.count else { return nil }

        for run in lines[row] where Int(run.col) <= col && col < Int(run.col + run.cells) {
            if run.cells == 2 && run.text.count == 1 {
                return (run, run.col, run.text)
            }

            let characters = Array(run.text)
            let index = col - Int(run.col)

            guard index < characters.count else { return nil }

            return (run, UInt16(col), String(characters[index]))
        }

        return nil
    }

    // MARK: Gestures

    @objc private func tapped(_ recognizer: UITapGestureRecognizer) {
        let cell = cell(at: recognizer.location(in: self))

        guard let handle = model.handle else { return }

        handle.clearSelection()

        if handle.click(col: cell.col, row: cell.row) { return }

        if !isFirstResponder { _ = becomeFirstResponder() }
    }

    /// Vertical drags scroll history, or send wheel events to a program
    /// tracking the mouse. Dragging down reveals older lines.
    @objc private func panned(_ recognizer: UIPanGestureRecognizer) {
        switch recognizer.state {
        case .began:
            scrollRemainder = 0
        case .changed:
            scrollRemainder += recognizer.translation(in: self).y

            recognizer.setTranslation(.zero, in: self)

            let lines = Int(scrollRemainder / metrics.cellHeight)

            guard lines != 0, let handle = model.handle else { return }

            scrollRemainder -= CGFloat(lines) * metrics.cellHeight

            let cell = cell(at: recognizer.location(in: self))

            _ = handle.scroll(col: cell.col, row: cell.row, lines: Int32(lines))
        default:
            break
        }
    }

    /// Long press selects the word under the finger; dragging on extends
    /// it, and lifting offers Copy.
    @objc private func pressed(_ recognizer: UILongPressGestureRecognizer) {
        guard let handle = model.handle else { return }

        let point = recognizer.location(in: self)
        let cell = cell(at: point)

        switch recognizer.state {
        case .began:
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()

            _ = handle.selectStart(col: cell.col, row: cell.row, word: true)
        case .changed:
            _ = handle.selectExtend(col: cell.col, row: cell.row)
        case .ended:
            editMenu.presentEditMenu(with: UIEditMenuConfiguration(identifier: nil, sourcePoint: point))
        default:
            break
        }
    }

    @objc private func pinched(_ recognizer: UIPinchGestureRecognizer) {
        switch recognizer.state {
        case .began:
            pinchBase = Double(metrics.regular.pointSize)
        case .changed:
            guard let base = pinchBase else { return }

            onFontSize?(min(18, max(8, (base * recognizer.scale).rounded())))
        default:
            pinchBase = nil
        }
    }

    // MARK: Edit actions

    override var canBecomeFirstResponder: Bool { true }

    override func becomeFirstResponder() -> Bool {
        let became = super.becomeFirstResponder()

        if became {
            model.keyboardShown = true

            if let cursor { setNeedsDisplay(rowRect(Int(cursor.row))) }
        }

        return became
    }

    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()

        if resigned {
            model.keyboardShown = false

            if let cursor { setNeedsDisplay(rowRect(Int(cursor.row))) }
        }

        return resigned
    }

    override func copy(_ sender: Any?) {
        model.copySelection()
    }

    override func paste(_ sender: Any?) {
        model.paste()
    }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        switch action {
        case #selector(copy(_:)): selection != nil
        case #selector(paste(_:)): UIPasteboard.general.hasStrings
        default: false
        }
    }
}

extension TerminalSurface: UIEditMenuInteractionDelegate {
    func editMenuInteraction(_ interaction: UIEditMenuInteraction,
                             menuFor configuration: UIEditMenuConfiguration,
                             suggestedActions: [UIMenuElement]) -> UIMenu? {
        var actions: [UIMenuElement] = []

        if selection != nil {
            actions.append(UIAction(title: tr("Copy"), image: UIImage(systemName: "doc.on.doc")) { [weak self] _ in
                self?.model.copySelection()
            })
        }

        if UIPasteboard.general.hasStrings {
            actions.append(UIAction(title: tr("Paste"), image: UIImage(systemName: "doc.on.clipboard")) { [weak self] _ in
                self?.model.paste()
            })
        }

        return UIMenu(children: actions)
    }
}

/// A display link retains its target; this keeps it from retaining the
/// surface, which would never leave memory otherwise.
private final class DisplayLinkTarget {
    weak var surface: TerminalSurface?

    init(_ surface: TerminalSurface) {
        self.surface = surface
    }

    @MainActor @objc func tick() {
        surface?.tick()
    }
}

/// The surface inside SwiftUI.
struct TerminalSurfaceView: UIViewRepresentable {
    let model: TerminalSessionModel
    let fontSize: Double
    let fontName: String
    var onFontSize: (Double) -> Void

    func makeUIView(context: Context) -> TerminalSurface {
        let surface = TerminalSurface(model: model, metrics: metrics(context))

        surface.onFontSize = onFontSize
        model.surface = surface

        return surface
    }

    func updateUIView(_ surface: TerminalSurface, context: Context) {
        surface.onFontSize = onFontSize
        surface.metrics = metrics(context)

        if model.surface !== surface { model.surface = surface }
    }

    private func metrics(_ context: Context) -> TerminalMetrics {
        TerminalMetrics(size: fontSize, name: fontName, scale: context.environment.displayScale)
    }
}
