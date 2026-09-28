import SwiftUI
import Observation
import UIKit

/// Stand-in for `TerminalHandle` (design doc §8.2). The real app pulls a `TerminalFrame`
/// (cell rows + style runs) from the core on each display-link tick and draws it with Core Text
/// in a UIKit `TerminalSurface`. This mock keeps a list of attributed lines and a local line editor.
@MainActor
@Observable
final class TerminalSessionModel {
    let session: Session
    var lines: [AttributedString]
    var input = ""
    var ctrl = false
    var alt = false
    var cols = 52
    var rows = 24

    init(session: Session) {
        self.session = session
        self.lines = [AttributedString("PS \(session.cwd)> ls")] + MockData.listing(cwd: session.cwd)
    }

    var prompt: String { "PS \(session.cwd)> " }

    func submit() {
        let command = input
        input = ""
        let trimmed = command.trimmingCharacters(in: .whitespaces)
        if trimmed == "clear" || trimmed == "cls" {
            lines = []
            return
        }
        lines.append(AttributedString(prompt + command))
        lines.append(contentsOf: MockData.output(for: command, cwd: session.cwd))
    }

    /// Sticky Ctrl: the next typed character becomes a control sequence.
    func inputChanged(from old: String, to new: String) {
        guard ctrl, new.count == old.count + 1, let c = new.last else { return }
        input = old
        ctrl = false
        lines.append(AttributedString(prompt + old + "^" + String(c).uppercased()))
        if c.lowercased() == "l" { lines = [] }
    }

    func press(_ key: AccessoryKey) {
        switch key.action {
        case .ctrl: ctrl.toggle()
        case .alt: alt.toggle()
        case .esc: input = ""
        case .text(let s): input += s
        case .paste: input += UIPasteboard.general.string ?? ""
        case .tab, .arrow, .nav: break // Real app: encoded by the core's key encoder and sent to the PTY.
        }
        if key.action != .ctrl && key.action != .alt {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
        }
    }
}

struct AccessoryKey: Identifiable {
    enum Action: Equatable {
        case esc, tab, ctrl, alt, paste
        case arrow(String)
        case nav(String)
        case text(String)
    }
    let label: String
    let action: Action
    var id: String { label }

    /// Default layout (§8.2); user-editable in Settings in the real app.
    static let defaultLayout: [AccessoryKey] = [
        .init(label: "Esc", action: .esc),
        .init(label: "Tab", action: .tab),
        .init(label: "Ctrl", action: .ctrl),
        .init(label: "Alt", action: .alt),
        .init(label: "←", action: .arrow("left")),
        .init(label: "↑", action: .arrow("up")),
        .init(label: "↓", action: .arrow("down")),
        .init(label: "→", action: .arrow("right")),
        .init(label: "|", action: .text("|")),
        .init(label: "~", action: .text("~")),
        .init(label: "/", action: .text("/")),
        .init(label: "-", action: .text("-")),
        .init(label: "Home", action: .nav("home")),
        .init(label: "End", action: .nav("end")),
        .init(label: "PgUp", action: .nav("pgup")),
        .init(label: "PgDn", action: .nav("pgdn")),
        .init(label: "Paste", action: .paste),
    ]
}
