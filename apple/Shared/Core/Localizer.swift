import Foundation
import Observation

/// In-app language override on top of the compiled string catalog.
///
/// `Localizable.xcstrings` compiles to `<lang>.lproj/Localizable.strings`; the
/// localizer resolves keys against the `.lproj` bundle of the chosen language
/// (or the system choice). It is `@Observable`, so every view that reads a
/// string re-renders the moment the language changes — no relaunch needed.
@Observable
@MainActor
final class Localizer {
    static let shared = Localizer()

    enum Language: String, CaseIterable, Identifiable, Sendable {
        case system
        case english = "en"
        case italian = "it"

        var id: String { rawValue }
    }

    var language: Language {
        didSet {
            guard language != oldValue else { return }
            AppGroup.defaults.set(language.rawValue, forKey: SharedKeys.language)
            resolve()
        }
    }

    /// Locale matching the effective language (for number formatting).
    private(set) var locale: Locale = .current
    /// Effective language code ("en" / "it").
    private(set) var languageCode: String = "en"

    @ObservationIgnored private var bundle: Bundle = .main
    @ObservationIgnored private var fallback: Bundle = .main

    private init() {
        let stored = AppGroup.defaults.string(forKey: SharedKeys.language)
        language = stored.flatMap(Language.init(rawValue:)) ?? .system
        resolve()
    }

    /// Re-reads the stored preference (extensions call this on launch).
    func reload() {
        let stored = AppGroup.defaults.string(forKey: SharedKeys.language)
        let lang = stored.flatMap(Language.init(rawValue:)) ?? .system
        if lang != language { language = lang } else { resolve() }
    }

    /// Looks up `key` and substitutes positional `{0}`, `{1}`… placeholders.
    func string(_ key: String, _ args: [String] = []) -> String {
        _ = language // register observation dependency
        var value = bundle.localizedString(forKey: key, value: missing, table: "Localizable")
        if value == missing {
            value = fallback.localizedString(forKey: key, value: key, table: "Localizable")
        }
        for (i, arg) in args.enumerated() {
            value = value.replacingOccurrences(of: "{\(i)}", with: arg)
        }
        return value
    }

    private let missing = "\u{1}missing\u{1}"

    private func resolve() {
        let available = Bundle.main.localizations.filter { $0 != "Base" }
        let code: String
        switch language {
        case .system:
            code = Bundle.preferredLocalizations(from: available, forPreferences: Locale.preferredLanguages).first ?? "en"
        case .english, .italian:
            code = language.rawValue
        }
        languageCode = code
        bundle = Self.lproj(code) ?? .main
        fallback = Self.lproj("en") ?? .main
        if language == .system {
            locale = .autoupdatingCurrent
        } else {
            // Keep the user's region (decimal separator etc.) but switch language.
            let region = Locale.current.region?.identifier
            locale = Locale(identifier: region.map { "\(code)_\($0)" } ?? code)
        }
    }

    private static func lproj(_ code: String) -> Bundle? {
        guard let path = Bundle.main.path(forResource: code, ofType: "lproj") else { return nil }
        return Bundle(path: path)
    }
}

/// Shorthand: `L("send")`, `L("state_waiting", peer)`.
@MainActor
func L(_ key: String, _ args: String...) -> String {
    Localizer.shared.string(key, args)
}
