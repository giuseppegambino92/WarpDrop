// Quick Code tab: the big code, its QR and Link My Device.
import { Check, Copy, Link2 } from "lucide-react";
import { useI18n } from "../i18n";
import { useStore } from "../lib/store";
import { ExpiresIn, useCopyCode } from "./CodeBar";
import { QrCode } from "./Qr";
import { Button } from "./ui";

export function QuickCode({ onLink }: { onLink: () => void }) {
  const { state } = useStore();
  const { t } = useI18n();
  const { copied, copy } = useCopyCode();
  return (
    <div className="flex flex-col items-center gap-5 py-6">
      <QrCode text={state.code.qrPayload} size={212} label={t("scan_qr")} />
      <div className="flex flex-col items-center gap-1">
        <span className="type-section text-text-secondary">{t("your_code")}</span>
        <span className="select-text text-[40px] leading-[48px] font-semibold tracking-[0.04em] tabular">{state.code.display}</span>
        <span className="type-caption text-text-secondary">
          <ExpiresIn at={state.code.expiresAtMs} />
        </span>
      </div>
      <p className="max-w-sm text-center type-body text-text-tertiary">{t("send_to_code_hint")}</p>
      <div className="flex gap-2">
        <Button onClick={() => void copy()}>
          {copied ? <Check size={15} className="text-success" /> : <Copy size={15} strokeWidth={1.75} />}
          {t("copy_code")}
        </Button>
        <Button onClick={onLink}>
          <Link2 size={15} strokeWidth={1.75} />
          {t("link_device")}
        </Button>
      </div>
    </div>
  );
}
