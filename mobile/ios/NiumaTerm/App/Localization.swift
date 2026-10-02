import Foundation
import Observation
import SwiftUI

/// The language the app shows, chosen in Settings. Stored by raw value in
/// user defaults, so the cases' names must not change.
enum AppLanguage: String, CaseIterable, Identifiable {
    case system
    case english
    case simplifiedChinese

    static let storageKey = "appLanguage"

    var id: String { rawValue }

    /// Each language is named in itself, so a reader finds theirs whatever
    /// language the app shows now.
    var label: String {
        switch self {
        case .system: tr("Follow System")
        case .english: "English"
        case .simplifiedChinese: "简体中文"
        }
    }

    /// The locale that resolves the app's strings. It keeps the region iOS
    /// reports, so dates and numbers stay formatted the way the user set
    /// them up while the words change language.
    var locale: Locale {
        let language = switch self {
        // The localization iOS picked for this app, from the system
        // languages or the per-app language in the Settings app.
        case .system: Bundle.main.preferredLocalizations.first ?? "en"
        case .english: "en"
        case .simplifiedChinese: "zh-Hans"
        }
        guard let region = Locale.current.region?.identifier else { return Locale(identifier: language) }
        return Locale(identifier: "\(language)_\(region)")
    }
}

/// The chosen language. Views that read it, directly or through `tr`, are
/// redrawn when it changes, so switching needs no relaunch.
///
/// Not main-actor isolated because value types compute their labels with
/// `tr` from nonisolated contexts; it is only written from Settings, on the
/// main thread.
@Observable
final class Localization: @unchecked Sendable {
    static let shared = Localization()

    var language: AppLanguage {
        didSet { UserDefaults.standard.set(language.rawValue, forKey: AppLanguage.storageKey) }
    }

    var locale: Locale { language.locale }

    private init() {
        language = UserDefaults.standard.string(forKey: AppLanguage.storageKey)
            .flatMap(AppLanguage.init(rawValue:)) ?? .system
    }
}

/// A string from the catalog in the chosen language, for text built outside
/// a SwiftUI `Text`. `Text` literals resolve through the `locale`
/// environment the app root sets instead.
func tr(_ resource: LocalizedStringResource) -> String {
    var resource = resource
    resource.locale = Localization.shared.locale
    return String(localized: resource)
}

/// Resolves the `Text` literals below it in the chosen language. A modifier
/// rather than a line in the app's scene body, because a view body tracks
/// the observable language and redraws when it changes.
struct AppLocaleEnvironment: ViewModifier {
    private let localization = Localization.shared

    func body(content: Content) -> some View {
        content.environment(\.locale, localization.locale)
    }
}
