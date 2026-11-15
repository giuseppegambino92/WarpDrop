import SwiftUI
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// Design tokens from docs/design-system.md. One accent, 1 px inner borders,
/// no shadows, no gradients.
enum Theme {
    static let bg = Color(light: 0xF6F6F7, dark: 0x141416)
    static let surface = Color(light: 0xFFFFFF, dark: 0x1C1C1F)
    static let surfaceHover = Color(light: 0xF1F1F3, dark: 0x232327)
    static let border = Color(light: 0x000000, lightAlpha: 0.08, dark: 0xFFFFFF, darkAlpha: 0.08)
    static let text = Color(light: 0x111113, dark: 0xEDEDEF)
    static let textSecondary = Color(light: 0x6B6B73, dark: 0x9A9AA3)
    static let textTertiary = Color(light: 0x9A9AA3, dark: 0x6B6B73)
    static let accent = Color(light: 0x2F6BFF, dark: 0x4C82FF)
    static let accentSoft = Color(light: 0x2F6BFF, lightAlpha: 0.10, dark: 0x4C82FF, darkAlpha: 0.16)
    static let success = Color(light: 0x1F9D55, dark: 0x34C26F)
    static let warning = Color(light: 0xC27C0E, dark: 0xE0A030)
    static let danger = Color(light: 0xD93D3D, dark: 0xF06262)
    static let cable = Color(light: 0xC27C0E, dark: 0xE0A030)

    enum Radius {
        static let card: CGFloat = 12
        static let control: CGFloat = 8
        static let badge: CGFloat = 6
    }

    enum Space {
        static let xxs: CGFloat = 4
        static let xs: CGFloat = 8
        static let s: CGFloat = 12
        static let m: CGFloat = 16
        static let l: CGFloat = 20
        static let xl: CGFloat = 24
        static let xxl: CGFloat = 32
        #if os(macOS)
        static let window: CGFloat = 20
        #else
        static let window: CGFloat = 16
        #endif
    }

    enum Fonts {
        static let title = Font.system(size: 20, weight: .semibold)
        static let section = Font.system(size: 13, weight: .semibold)
        #if os(macOS)
        static let body = Font.system(size: 14)
        static let bodyStrong = Font.system(size: 14, weight: .semibold)
        #else
        static let body = Font.system(size: 16)
        static let bodyStrong = Font.system(size: 16, weight: .semibold)
        #endif
        static let caption = Font.system(size: 12, weight: .medium)
        static let code = Font.system(size: 28, weight: .semibold).monospacedDigit()
        /// 0.04 em of the 28 pt code style.
        static let codeTracking: CGFloat = 28 * 0.04
    }

    enum Motion {
        static let spring = Animation.spring(response: 0.35, dampingFraction: 0.8)
        static let progress = Animation.linear(duration: 0.15)
        static let pulse = Animation.easeInOut(duration: 1.2).repeatForever(autoreverses: true)
    }
}

extension Color {
    init(light: UInt32, lightAlpha: CGFloat = 1, dark: UInt32, darkAlpha: CGFloat = 1) {
        #if os(macOS)
        let color = NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return isDark ? NSColor(hex: dark, alpha: darkAlpha) : NSColor(hex: light, alpha: lightAlpha)
        }
        self.init(nsColor: color)
        #else
        let color = UIColor { traits in
            traits.userInterfaceStyle == .dark ? UIColor(hex: dark, alpha: darkAlpha) : UIColor(hex: light, alpha: lightAlpha)
        }
        self.init(uiColor: color)
        #endif
    }
}

#if os(macOS)
extension NSColor {
    convenience init(hex: UInt32, alpha: CGFloat) {
        self.init(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }
}
#else
extension UIColor {
    convenience init(hex: UInt32, alpha: CGFloat) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }
}
#endif

// MARK: - Surfaces

struct CardSurface: ViewModifier {
    var fill: Color = Theme.surface
    var border: Color = Theme.border
    var lineWidth: CGFloat = 1
    var radius: CGFloat = Theme.Radius.card

    func body(content: Content) -> some View {
        content
            .background(RoundedRectangle(cornerRadius: radius, style: .continuous).fill(fill))
            .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(border, lineWidth: lineWidth))
    }
}

extension View {
    func cardSurface(fill: Color = Theme.surface, border: Color = Theme.border, lineWidth: CGFloat = 1, radius: CGFloat = Theme.Radius.card) -> some View {
        modifier(CardSurface(fill: fill, border: border, lineWidth: lineWidth, radius: radius))
    }
}

/// Quiet bordered button used across the app (no fills, no shadows).
struct QuietButtonStyle: ButtonStyle {
    var prominent = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.Fonts.bodyStrong)
            .foregroundStyle(prominent ? Color.white : Theme.text)
            .padding(.horizontal, Theme.Space.s)
            .padding(.vertical, Theme.Space.xs - 1)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                    .fill(prominent ? Theme.accent : (configuration.isPressed ? Theme.surfaceHover : Theme.surface))
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                    .strokeBorder(prominent ? Color.clear : Theme.border, lineWidth: 1)
            )
            .opacity(configuration.isPressed && prominent ? 0.85 : 1)
            .contentShape(Rectangle())
    }
}

/// Icon-only button: 28 pt hit target, secondary tint.
struct IconButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(Theme.textSecondary)
            .frame(width: 28, height: 28)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                    .fill(configuration.isPressed ? Theme.surfaceHover : Color.clear)
            )
            .contentShape(Rectangle())
    }
}
