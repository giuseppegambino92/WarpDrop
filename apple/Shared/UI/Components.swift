import SwiftUI
import WarpCore

/// Caption-sized pill (connection / trust / state badges).
struct WarpBadge: View {
    let text: String
    var tint: Color = Theme.textSecondary

    var body: some View {
        Text(text)
            .font(Theme.Fonts.caption)
            .foregroundStyle(tint)
            .lineLimit(1)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(RoundedRectangle(cornerRadius: Theme.Radius.badge, style: .continuous).fill(tint.opacity(0.12)))
    }
}

/// Connection badge for a path (nil → nothing).
struct PathBadge: View {
    let path: PathKind?

    var body: some View {
        if let path {
            WarpBadge(text: L(path.badgeKey), tint: path.isCable ? Theme.cable : Theme.textSecondary)
        }
    }
}

/// 4 pt progress bar; value changes animate 150 ms linear, never faked.
struct ThinProgressBar: View {
    let value: Double
    var tint: Color = Theme.accent

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.border)
                Capsule()
                    .fill(tint)
                    .frame(width: max(0, min(1, value)) * geo.size.width)
            }
        }
        .frame(height: 4)
        .animation(Theme.Motion.progress, value: value)
        .accessibilityElement()
        .accessibilityValue(WarpFormat.percent(value))
    }
}

/// Platform glyph in a 48 pt rounded square.
struct DeviceGlyph: View {
    let kind: DeviceKind
    var size: CGFloat = 48

    var body: some View {
        Image(systemName: kind.symbolName)
            .font(.system(size: size * 0.46, weight: .regular))
            .foregroundStyle(Theme.text)
            .frame(width: size, height: size)
            .background(RoundedRectangle(cornerRadius: Theme.Radius.card - 2, style: .continuous).fill(Theme.surfaceHover))
            .accessibilityHidden(true)
    }
}

/// "Searching" indicator — the only looping animation allowed.
struct PulsingDot: View {
    @State private var dim = false

    var body: some View {
        Circle()
            .fill(Theme.accent)
            .frame(width: 8, height: 8)
            .opacity(dim ? 0.4 : 1)
            .onAppear {
                withAnimation(Theme.Motion.pulse) { dim = true }
            }
            .accessibilityHidden(true)
    }
}

/// The 6-digit code in the code style (tabular digits, 0.04 em tracking).
struct CodeText: View {
    let text: String
    var size: CGFloat = 28

    var body: some View {
        Text(text)
            .font(.system(size: size, weight: .semibold).monospacedDigit())
            .tracking(size * 0.04)
            .foregroundStyle(Theme.text)
            .lineLimit(1)
            .minimumScaleFactor(0.6)
            .textSelection(.enabled)
    }
}

/// QR code drawn from the engine's module matrix as crisp, pixel-snapped
/// squares with a 4-module quiet zone.
struct QRCodeView: View {
    private let matrix: QrMatrix?
    @Environment(\.displayScale) private var displayScale

    init(payload: String) {
        matrix = try? qrMatrix(text: payload)
    }

    var body: some View {
        Canvas { context, size in
            context.fill(Path(CGRect(origin: .zero, size: size)), with: .color(.white))
            guard let matrix, matrix.size > 0 else { return }
            let n = Int(matrix.size)
            let quiet = 4
            let total = n + quiet * 2
            let side = min(size.width, size.height)
            // Snap each module to whole device pixels so edges stay sharp.
            let pixels = floor(side * displayScale / CGFloat(total))
            let module = max(pixels, 1) / displayScale
            let drawn = module * CGFloat(total)
            let originX = ((size.width - drawn) / 2 * displayScale).rounded() / displayScale + module * CGFloat(quiet)
            let originY = ((size.height - drawn) / 2 * displayScale).rounded() / displayScale + module * CGFloat(quiet)
            var path = Path()
            for y in 0..<n {
                for x in 0..<n where matrix.modules[y * n + x] {
                    path.addRect(CGRect(x: originX + CGFloat(x) * module, y: originY + CGFloat(y) * module, width: module, height: module))
                }
            }
            context.fill(path, with: .color(.black))
        }
        .aspectRatio(1, contentMode: .fit)
        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous))
        .accessibilityLabel(L("show_qr"))
    }
}

/// Small section heading (13/600).
struct SectionTitle: View {
    let text: String

    var body: some View {
        Text(text)
            .font(Theme.Fonts.section)
            .foregroundStyle(Theme.textSecondary)
    }
}
