import SwiftUI
import UIKit

/// Colors lifted from the desktop app. Solid colors only — no gradients.
/// Each has a light and a dark value and follows the system appearance; the
/// dark values keep the same warm hues, with text and accents raised so
/// they keep their contrast on dark backgrounds.
enum Theme {
    static let accent = Color(light: 0x7B5A50, dark: 0xC49A8C)
    static let ink = Color(light: 0x1F1B19, dark: 0xF1ECE9)
    static let ink2 = Color(light: 0x4A433F, dark: 0xCFC7C2)
    static let secondary = Color(light: 0x6F6863, dark: 0xA59D98)
    static let tertiary = Color(light: 0x9A938E, dark: 0x7A736E)

    static let canvas = Color(light: 0xF2ECE9, dark: 0x151312)
    static let sheet = Color(light: 0xFAF8F6, dark: 0x1E1B19)
    static let transcriptBackground = Color(light: 0xEFEDEB, dark: 0x171514)
    static let terminalBackground = Color(light: 0xE8E6E3, dark: 0x121110)
    static let bubble = Color(light: 0xDEDAD7, dark: 0x2E2A27)
    static let rule = Color(light: 0xD3CDC8, dark: 0x3A3532)
    static let rowBackground = Color(light: 0xFFFFFF, lightOpacity: 0.62, dark: 0x24201E, darkOpacity: 1)

    /// Text and icons drawn on `accent`.
    static let onAccent = Color(light: 0xFFFFFF, dark: 0x1F1B19)

    /// Raised cards and fields that sit on `canvas` or `sheet`.
    static let card = Color(light: 0xFFFFFF, dark: 0x262220)

    /// Faint fills behind unselected chips and small round buttons.
    static let fill = Color(light: 0x000000, lightOpacity: 0.06, dark: 0xFFFFFF, darkOpacity: 0.1)

    static let attention = Color(light: 0xB0632E, dark: 0xE08A50)
    static let online = Color(light: 0x4F9A62, dark: 0x6CC080)
    static let offline = Color(light: 0xB0A8A2, dark: 0x6E6762)
    static let directory = Color(light: 0x1C6EA4, dark: 0x5AA9E0)

    // Code blocks are dark in both appearances; in dark mode they sit a
    // step below the transcript background.
    static let codeBackground = Color(light: 0x1F1B19, dark: 0x0C0B0A)
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

/// The app's appearance setting. `system` follows the phone's light or dark
/// mode; the others hold the app to one of them.
enum AppAppearance: String, CaseIterable, Identifiable {
    case system
    case light
    case dark

    var id: String { rawValue }

    var label: String {
        switch self {
        case .system: tr("System")
        case .light: tr("Light")
        case .dark: tr("Dark")
        }
    }

    /// Impose the setting on every window of the app. It is set on the
    /// windows rather than as a SwiftUI preferred scheme because a sheet
    /// open while the setting changes, such as Settings itself, does not
    /// always follow a new preferred scheme, and never returns to the
    /// system's once one was set; a window's style covers everything it
    /// presents, and `.unspecified` hands control back to the system.
    @MainActor
    func apply() {
        let style: UIUserInterfaceStyle = switch self {
        case .system: .unspecified
        case .light: .light
        case .dark: .dark
        }
        for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
            for window in scene.windows {
                window.overrideUserInterfaceStyle = style
            }
        }
    }
}

extension Color {
    /// A color that follows the appearance of the view it is drawn in.
    init(light: UInt32, lightOpacity: Double = 1, dark: UInt32, darkOpacity: Double = 1) {
        self.init(uiColor: UIColor { traits in
            traits.userInterfaceStyle == .dark
                ? UIColor(hex: dark, alpha: darkOpacity)
                : UIColor(hex: light, alpha: lightOpacity)
        })
    }

    init(hex: UInt32, opacity: Double = 1) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255,
                  opacity: opacity)
    }
}

extension UIColor {
    convenience init(hex: UInt32, alpha: Double = 1) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255,
                  green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255,
                  alpha: alpha)
    }
}
