import SwiftUI
import WarpCore

/// "<Peer> wants to send you <title> (<size>)" with the four SAS emoji.
struct OfferSheet: View {
    @Environment(AppModel.self) private var model
    let offer: IncomingOffer

    var body: some View {
        VStack(spacing: Theme.Space.l) {
            DeviceGlyph(kind: offer.peerKind)
            VStack(spacing: Theme.Space.xxs) {
                Text(L("offer_title", offer.peerName))
                    .font(Theme.Fonts.title)
                    .foregroundStyle(Theme.text)
                    .multilineTextAlignment(.center)
                Text("\(offer.title) (\(WarpFormat.bytes(offer.totalBytes)))")
                    .font(Theme.Fonts.body)
                    .foregroundStyle(Theme.textSecondary)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
            }
            if !offer.sasEmoji.isEmpty {
                VStack(spacing: Theme.Space.xs) {
                    HStack(spacing: Theme.Space.m) {
                        ForEach(Array(offer.sasEmoji.enumerated()), id: \.offset) { _, emoji in
                            Text(emoji).font(.system(size: 44))
                        }
                    }
                    .padding(.vertical, Theme.Space.s)
                    .padding(.horizontal, Theme.Space.l)
                    .cardSurface()
                    .accessibilityElement(children: .combine)
                    Text(L("offer_sas_hint", offer.peerName))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.textSecondary)
                        .multilineTextAlignment(.center)
                }
            }
            HStack(spacing: Theme.Space.s) {
                Button(L("decline")) { model.respond(to: offer, accept: false) }
                    .buttonStyle(QuietButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button(L("accept")) { model.respond(to: offer, accept: true) }
                    .buttonStyle(QuietButtonStyle(prominent: true))
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(Theme.Space.xxl)
        .frame(minWidth: 360)
        .presentationDetents([.medium])
        .environment(\.locale, Localizer.shared.locale)
    }
}
