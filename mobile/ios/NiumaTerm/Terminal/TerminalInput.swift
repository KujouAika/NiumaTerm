import UIKit

import NiumaTermCore

/// A position in the text an input method is composing. The terminal keeps
/// no document of its own: committed text goes to the host at once, so the
/// only text input methods see is what they are still composing.
final class ComposingPosition: UITextPosition {
    let offset: Int

    init(_ offset: Int) { self.offset = offset }
}

final class ComposingRange: UITextRange {
    let lower: Int
    let upper: Int

    init(_ lower: Int, _ upper: Int) {
        self.lower = min(lower, upper)
        self.upper = max(lower, upper)
    }

    override var start: UITextPosition { ComposingPosition(lower) }
    override var end: UITextPosition { ComposingPosition(upper) }
    override var isEmpty: Bool { lower == upper }
}

// MARK: Keyboard text and input methods

extension TerminalSurface: UITextInput {
    // Terminals take text literally: no corrections, capitals, or smart
    // punctuation, which would rewrite commands as they are typed.
    var autocorrectionType: UITextAutocorrectionType { get { .no } set {} }

    var autocapitalizationType: UITextAutocapitalizationType { get { .none } set {} }
    var spellCheckingType: UITextSpellCheckingType { get { .no } set {} }
    var smartQuotesType: UITextSmartQuotesType { get { .no } set {} }
    var smartDashesType: UITextSmartDashesType { get { .no } set {} }
    var smartInsertDeleteType: UITextSmartInsertDeleteType { get { .no } set {} }
    var inlinePredictionType: UITextInlinePredictionType { get { .no } set {} }
    var keyboardType: UIKeyboardType { get { .default } set {} }

    var hasText: Bool { true }

    func insertText(_ text: String) {
        if !markedText.isEmpty { clearMarkedText() }

        model.sendText(text)
    }

    func deleteBackward() {
        model.sendKey("backspace")
    }

    func text(in range: UITextRange) -> String? {
        guard let range = range as? ComposingRange else { return nil }

        let characters = Array(markedText)
        let lower = min(range.lower, characters.count)
        let upper = min(range.upper, characters.count)

        return String(characters[lower..<upper])
    }

    func replace(_ range: UITextRange, withText text: String) {
        insertText(text)
    }

    var selectedTextRange: UITextRange? {
        get {
            let lower = markedSelection.location

            return ComposingRange(lower, lower + markedSelection.length)
        }
        set {
            guard let range = newValue as? ComposingRange else { return }

            markedSelection = NSRange(location: range.lower, length: range.upper - range.lower)
        }
    }

    var markedTextRange: UITextRange? {
        markedText.isEmpty ? nil : ComposingRange(0, markedText.count)
    }

    func setMarkedText(_ markedText: String?, selectedRange: NSRange) {
        self.markedText = markedText ?? ""
        markedSelection = selectedRange

        redrawCursorRow()
    }

    /// The input method committed what it was composing.
    func unmarkText() {
        let text = markedText

        clearMarkedText()

        if !text.isEmpty { model.sendText(text) }
    }

    private func clearMarkedText() {
        markedText = ""
        markedSelection = NSRange(location: 0, length: 0)

        redrawCursorRow()
    }

    private func redrawCursorRow() {
        setNeedsDisplay(cursor.map { rowRect(Int($0.row)) } ?? bounds)
    }

    var beginningOfDocument: UITextPosition { ComposingPosition(0) }
    var endOfDocument: UITextPosition { ComposingPosition(markedText.count) }

    func textRange(from fromPosition: UITextPosition, to toPosition: UITextPosition) -> UITextRange? {
        guard let from = fromPosition as? ComposingPosition, let to = toPosition as? ComposingPosition else {
            return nil
        }

        return ComposingRange(from.offset, to.offset)
    }

    func position(from position: UITextPosition, offset: Int) -> UITextPosition? {
        guard let position = position as? ComposingPosition else { return nil }

        let moved = position.offset + offset

        return (0...markedText.count).contains(moved) ? ComposingPosition(moved) : nil
    }

    func position(from position: UITextPosition, in direction: UITextLayoutDirection,
                  offset: Int) -> UITextPosition? {
        switch direction {
        case .left, .up: self.position(from: position, offset: -offset)
        default: self.position(from: position, offset: offset)
        }
    }

    func compare(_ position: UITextPosition, to other: UITextPosition) -> ComparisonResult {
        let a = (position as? ComposingPosition)?.offset ?? 0
        let b = (other as? ComposingPosition)?.offset ?? 0

        return a < b ? .orderedAscending : a > b ? .orderedDescending : .orderedSame
    }

    func offset(from: UITextPosition, to toPosition: UITextPosition) -> Int {
        ((toPosition as? ComposingPosition)?.offset ?? 0) - ((from as? ComposingPosition)?.offset ?? 0)
    }

    func position(within range: UITextRange, farthestIn direction: UITextLayoutDirection) -> UITextPosition? {
        switch direction {
        case .left, .up: range.start
        default: range.end
        }
    }

    func characterRange(byExtending position: UITextPosition,
                        in direction: UITextLayoutDirection) -> UITextRange? {
        nil
    }

    func baseWritingDirection(for position: UITextPosition,
                              in direction: UITextStorageDirection) -> NSWritingDirection {
        .leftToRight
    }

    func setBaseWritingDirection(_ writingDirection: NSWritingDirection, for range: UITextRange) {}

    /// Candidate windows open next to the terminal cursor.
    func firstRect(for range: UITextRange) -> CGRect {
        caretRect(for: range.start)
    }

    func caretRect(for position: UITextPosition) -> CGRect {
        guard let cursor else { return .zero }

        return cellRect(col: Int(cursor.col), cells: 1, row: Int(cursor.row))
    }

    func selectionRects(for range: UITextRange) -> [UITextSelectionRect] { [] }

    func closestPosition(to point: CGPoint) -> UITextPosition? { endOfDocument }

    func closestPosition(to point: CGPoint, within range: UITextRange) -> UITextPosition? { range.end }

    func characterRange(at point: CGPoint) -> UITextRange? { nil }
}

// MARK: Hardware keyboards

extension TerminalSurface {
    /// Keys the terminal encodes itself: named keys, and characters with
    /// Control or Option. Plain characters and Command chords go the usual
    /// way, so input methods, key repeat for text, and Command-C and
    /// Command-V keep working.
    private func terminalKey(_ key: UIKey) -> TerminalKeyInput? {
        // While an input method composes, its own keys (arrows through the
        // candidates, Return to commit) belong to it.
        guard markedText.isEmpty else { return nil }

        let flags = key.modifierFlags

        guard !flags.contains(.command) else { return nil }

        let shift = flags.contains(.shift)
        let control = flags.contains(.control)
        let alt = flags.contains(.alternate)

        if let name = Self.namedKeys[key.keyCode] {
            // Unmodified Return, Tab, and Backspace arrive as text and
            // `deleteBackward`, which repeat while held.
            let plain = ["enter", "tab", "backspace"].contains(name)

            if plain && !(shift || control || alt) { return nil }

            return TerminalKeyInput(key: name, text: nil, shift: shift, control: control, alt: alt, command: false)
        }

        guard control || alt else { return nil }

        let text = key.charactersIgnoringModifiers

        guard !text.isEmpty else { return nil }

        return TerminalKeyInput(key: text.lowercased(), text: text, shift: shift, control: control,
                                alt: alt, command: false)
    }

    private static let namedKeys: [UIKeyboardHIDUsage: String] = [
        .keyboardReturnOrEnter: "enter",
        .keypadEnter: "enter",
        .keyboardTab: "tab",
        .keyboardDeleteOrBackspace: "backspace",
        .keyboardEscape: "escape",
        .keyboardUpArrow: "up",
        .keyboardDownArrow: "down",
        .keyboardLeftArrow: "left",
        .keyboardRightArrow: "right",
        .keyboardHome: "home",
        .keyboardEnd: "end",
        .keyboardPageUp: "pageup",
        .keyboardPageDown: "pagedown",
        .keyboardInsert: "insert",
        .keyboardDeleteForward: "delete",
        .keyboardF1: "f1", .keyboardF2: "f2", .keyboardF3: "f3", .keyboardF4: "f4",
        .keyboardF5: "f5", .keyboardF6: "f6", .keyboardF7: "f7", .keyboardF8: "f8",
        .keyboardF9: "f9", .keyboardF10: "f10", .keyboardF11: "f11", .keyboardF12: "f12",
    ]

    /// Arrows and other navigation keys repeat while held, as they do on
    /// the desktop; UIKit repeats only text.
    private static let repeating: Set<String> = ["up", "down", "left", "right", "delete", "pageup", "pagedown"]

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var unhandled = Set<UIPress>()

        for press in presses {
            guard let key = press.key, let input = terminalKey(key) else {
                unhandled.insert(press)

                continue
            }

            model.sendKey(input.key, text: input.text, shift: input.shift, control: input.control,
                          alt: input.alt)

            startRepeat(input)
        }

        if !unhandled.isEmpty { super.pressesBegan(unhandled, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        stopRepeat()

        super.pressesEnded(presses, with: event)
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        stopRepeat()

        super.pressesCancelled(presses, with: event)
    }

    private func startRepeat(_ input: TerminalKeyInput) {
        stopRepeat()

        guard Self.repeating.contains(input.key) else { return }

        repeatTimer = Timer.scheduledTimer(withTimeInterval: 0.4, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.repeatTimer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
                    MainActor.assumeIsolated {
                        _ = self?.model.sendKey(input.key, shift: input.shift, control: input.control,
                                                alt: input.alt)
                    }
                }
            }
        }
    }

    private func stopRepeat() {
        repeatTimer?.invalidate()

        repeatTimer = nil
    }
}
