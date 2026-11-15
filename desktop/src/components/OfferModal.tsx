// Incoming offer from a device outside the trust ring, with SAS emoji.
import { useState } from "react";
import { localizeCmdError, localizeTitle, useI18n } from "../i18n";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import { useStore } from "../lib/store";
import { Button, DeviceGlyph, Sheet } from "./ui";

export function OfferModal() {
  const { state, dispatch, toast } = useStore();
  const { t, lang } = useI18n();
  const [busy, setBusy] = useState(false);
  const offer = state.offers[0];
  if (!offer) return null;

  const respond = async (accept: boolean) => {
    setBusy(true);
    try {
      await api.respondToOffer(offer.offerId, accept);
    } catch (e) {
      toast("error", localizeCmdError(t, e, offer.peerName));
    } finally {
      dispatch({ type: "dismissOffer", offerId: offer.offerId });
      setBusy(false);
    }
  };

  return (
    <Sheet onClose={() => void respond(false)} width={400} dismissible={false}>
      <div key={offer.offerId} className="flex flex-col items-center gap-4 text-center">
        <span className="flex h-12 w-12 items-center justify-center rounded-[10px] bg-surface-hover text-text-secondary">
          <DeviceGlyph kind={offer.peerKind} size={32} strokeWidth={1.5} />
        </span>
        <div className="flex flex-col gap-1">
          <span className="type-body text-text-secondary">{t("offer_title", offer.peerName)}</span>
          <span className="type-title break-all">{localizeTitle(t, offer.title)}</span>
          <span className="type-caption text-text-tertiary tabular">
            {offer.itemCount > 1 ? `${t("items_count", offer.itemCount)} · ` : ""}
            {formatBytes(lang, offer.totalBytes)}
          </span>
        </div>
        {offer.sasEmoji.length > 0 && (
          <div className="flex flex-col items-center gap-2">
            <div className="flex gap-2" aria-label={offer.sasEmoji.join(" ")}>
              {offer.sasEmoji.map((e, i) => (
                <span key={i} className="flex h-14 w-14 items-center justify-center rounded-card bg-surface-hover text-[32px] leading-none">
                  {e}
                </span>
              ))}
            </div>
            <span className="type-caption text-text-secondary">{t("offer_sas_hint", offer.peerName)}</span>
          </div>
        )}
        <div className="mt-1 flex w-full gap-2">
          <Button className="flex-1" disabled={busy} onClick={() => void respond(false)}>
            {t("decline")}
          </Button>
          <Button className="flex-1" variant="primary" disabled={busy} onClick={() => void respond(true)} autoFocus>
            {t("accept")}
          </Button>
        </div>
      </div>
    </Sheet>
  );
}
