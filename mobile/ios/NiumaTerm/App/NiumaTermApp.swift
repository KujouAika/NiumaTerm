import SwiftUI
import CoreText

@main
struct NiumaTermApp: App {
    @State private var app = AppModel()

    init() {
        FontRegistrar.registerBundledFonts()
    }

    var body: some Scene {
        WindowGroup {
            HostListView()
                .environment(app)
                .tint(Theme.accent)
        }
    }
}

/// Registers every .ttf/.otf in the bundle so no UIAppFonts plist entry is needed.
/// Drop JetBrainsMonoNerdFontMono-*.ttf into NiumaTerm/Resources/Fonts to enable it (design doc §8.2).
enum FontRegistrar {
    static func registerBundledFonts() {
        let urls = (Bundle.main.urls(forResourcesWithExtension: "ttf", subdirectory: nil) ?? [])
            + (Bundle.main.urls(forResourcesWithExtension: "otf", subdirectory: nil) ?? [])
        for url in urls {
            _ = CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
    }
}
