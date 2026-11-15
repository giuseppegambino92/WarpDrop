import SwiftUI
import WarpCore

/// "Your code 849-201 ⧉ ▦  ·  Send to code [___-___] →"
struct CodeBarView: View {
    @Environment(AppModel.self) private var model
    let compact: Bool
    @State private var showQR = false
    @State private var showScanner = false

    var body: some View {
        Group {
            if compact {
                VStack(alignment: .leading, spacing: Theme.Space.s) {
                    yourCode
                    sendToCode
                }
            } else {
                HStack(alignment: .center, spacing: Theme.Space.xl) {
                    yourCode
                    Spacer(minLength: Theme.Space.m)
                    sendToCode.frame(maxWidth: 300)
                }
            }
        }
        .padding(.horizontal, Theme.Space.window)
        .padding(.vertical, Theme.Space.m)
        #if os(iOS)
        .sheet(isPresented: $showScanner) {
            QRScannerSheet { payload in
                showScanner = false
                model.submitCode(payload)
            }
        }
        #endif
    }

    private var yourCode: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(L("your_code"))
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.textSecondary)
            HStack(spacing: Theme.Space.xs) {
                CodeText(text: model.code?.display ?? "———")
                    .contentTransition(.numericText())
                    .animation(Theme.Motion.spring, value: model.code?.display)
                Button {
                    if let code = model.code {
                        PlatformServices.copy(code.display)
                        model.show(L("code_copied"))
                    }
                } label: {
                    Image(systemName: "doc.on.doc")
                }
                .buttonStyle(IconButtonStyle())
                .help(L("copy_code"))
                .accessibilityLabel(L("copy_code"))

                Button { showQR.toggle() } label: {
                    Image(systemName: "qrcode")
                }
                .buttonStyle(IconButtonStyle())
                .help(L("show_qr"))
                .accessibilityLabel(L("show_qr"))
                .popover(isPresented: $showQR, arrowEdge: .bottom) {
                    QRPopoverContent()
                        .environment(model)
                        #if os(iOS)
                        .presentationCompactAdaptation(.popover)
                        #endif
                }

                if let code = model.code {
                    CountdownText(expiresAtMs: code.expiresAtMs)
                        .padding(.leading, Theme.Space.xxs)
                }
            }
        }
    }

    private var sendToCode: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(L("send_to_code"))
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.textSecondary)
            HStack(spacing: Theme.Space.xs) {
                CodeEntryField(placeholder: "000-000") { target in
                    model.submitCode(target)
                }
                #if os(iOS)
                Button { showScanner = true } label: {
                    Image(systemName: "qrcode.viewfinder")
                        .font(.system(size: 16, weight: .medium))
                        .foregroundStyle(Theme.text)
                        .frame(width: 34, height: 34)
                        .cardSurface(radius: Theme.Radius.control)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(L("scan_qr"))
                #endif
            }
        }
    }
}

/// "Expires in 7:42", ticking once per second.
struct CountdownText: View {
    let expiresAtMs: UInt64

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let remaining = Int((Double(expiresAtMs) / 1000 - context.date.timeIntervalSince1970).rounded(.down))
            Text(L("code_expires_in", WarpFormat.countdown(remaining)))
                .font(Theme.Fonts.caption.monospacedDigit())
                .foregroundStyle(Theme.textTertiary)
        }
    }
}

/// QR of the current code with a quiet zone and expiry.
struct QRPopoverContent: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: Theme.Space.s) {
            if let code = model.code {
                QRCodeView(payload: code.qrPayload)
                    .frame(width: 220, height: 220)
                    .id(code.qrPayload)
                CodeText(text: code.display)
                CountdownText(expiresAtMs: code.expiresAtMs)
            }
        }
        .padding(Theme.Space.l)
    }
}
