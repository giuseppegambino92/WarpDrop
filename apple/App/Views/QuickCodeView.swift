import SwiftUI
import WarpCore

/// Quick Code tab: the code and QR large, plus the send-to-code entry.
struct QuickCodeView: View {
    @Environment(AppModel.self) private var model
    let compact: Bool
    @State private var showScanner = false

    var body: some View {
        ScrollView {
            VStack(spacing: Theme.Space.xl) {
                if let staged = model.staged {
                    StagedBanner(payload: staged)
                }
                if let code = model.code {
                    VStack(spacing: Theme.Space.s) {
                        Text(L("your_code"))
                            .font(Theme.Fonts.section)
                            .foregroundStyle(Theme.textSecondary)
                        CodeText(text: code.display, size: compact ? 40 : 48)
                            .contentTransition(.numericText())
                            .animation(Theme.Motion.spring, value: code.display)
                        QRCodeView(payload: code.qrPayload)
                            .frame(width: compact ? 200 : 220, height: compact ? 200 : 220)
                            .id(code.qrPayload)
                            .padding(Theme.Space.s)
                            .cardSurface()
                        CountdownText(expiresAtMs: code.expiresAtMs)
                    }
                }

                VStack(alignment: .leading, spacing: Theme.Space.xs) {
                    Text(L("send_to_code"))
                        .font(Theme.Fonts.section)
                        .foregroundStyle(Theme.textSecondary)
                    CodeEntryField(placeholder: "000-000", large: true) { model.submitCode($0) }
                    Text(L("send_to_code_hint"))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.textTertiary)
                    #if os(iOS)
                    Button {
                        showScanner = true
                    } label: {
                        Label(L("scan_qr"), systemImage: "qrcode.viewfinder")
                    }
                    .buttonStyle(QuietButtonStyle())
                    .padding(.top, Theme.Space.xs)
                    #endif
                }
                .frame(maxWidth: 360)
            }
            .frame(maxWidth: .infinity)
            .padding(Theme.Space.window)
            .padding(.top, Theme.Space.s)
        }
        .scrollContentBackground(.hidden)
        #if os(iOS)
        .sheet(isPresented: $showScanner) {
            QRScannerSheet { payload in
                showScanner = false
                model.submitCode(payload)
            }
        }
        #endif
    }
}
