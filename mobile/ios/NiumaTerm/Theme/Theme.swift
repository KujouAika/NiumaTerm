import SwiftUI
import UIKit

/// Colors lifted from the desktop app. Solid colors only — no gradients.
enum Theme {
    static let accent = Color(hex: 0x7B5A50)
    static let ink = Color(hex: 0x1F1B19)
    static let ink2 = Color(hex: 0x4A433F)
    static let secondary = Color(hex: 0x6F6863)
    static let tertiary = Color(hex: 0x9A938E)

    static let canvas = Color(hex: 0xF2ECE9)
    static let sheet = Color(hex: 0xFAF8F6)
    static let transcriptBackground = Color(hex: 0xEFEDEB)
    static let terminalBackground = Color(hex: 0xE8E6E3)
    static let bubble = Color(hex: 0xDEDAD7)
    static let rule = Color(hex: 0xD3CDC8)
    static let rowBackground = Color.white.opacity(0.62)

    static let attention = Color(hex: 0xB0632E)
    static let online = Color(hex: 0x4F9A62)
    static let offline = Color(hex: 0xB0A8A2)
    static let directory = Color(hex: 0x1C6EA4)
    static let codeBackground = Color(hex: 0x1F1B19)
    static let codeText = Color(hex: 0xEFE9E5)
    static let codePrompt = Color(hex: 0xC9A89C)

    static let jetBrainsName = "JetBrainsMonoNFM-Regular"
    static var hasJetBrains: Bool { UIFont(name: jetBrainsName, size: 12) != nil }

    /// JetBrains Mono when bundled, SF Mono otherwise.
    static func mono(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        if hasJetBrains {
            return .custom(jetBrainsName, fixedSize: size).weight(weight)
        }
        return .system(size: size, weight: weight, design: .monospaced)
    }

    static func terminalFont(_ size: CGFloat, name: String) -> Font {
        name == "SF Mono" ? .system(size: size, design: .monospaced) : mono(size)
    }
}

extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255,
                  opacity: opacity)
    }
}
