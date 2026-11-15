// "Your code 849-201 ⧉ ▦" on the left, "Send to code [___-___] →" on the right.
import { useEffect, useRef, useState } from "react";
import { ArrowRight, Check, Copy, QrCode as QrIcon } from "lucide-react";
import { useI18n } from "../i18n";
import { api } from "../lib/api";
import { emptyCode, formatCountdown, type CodeEntry } from "../lib/format";
import { useNow } from "../lib/hooks";
import { useActions } from "../lib/actions";
import { useStore } from "../lib/store";
import { CodeInput } from "./CodeInput";
import { QrCode } from "./Qr";
import { IconButton } from "./ui";

export function useCopyCode() {
  const { state, toast } = useStore();
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(id);
  }, [copied]);
  const copy = async () => {
    try {
      await api.copyText(state.code.display);
      setCopied(true);
      toast("success", t("code_copied"));
    } catch {
      toast("error", t("error_clipboard_read"));
    }
  };
  return { copied, copy };
}

export function ExpiresIn({ at }: { at: number }) {
  const { t } = useI18n();
  const now = useNow(1000);
  return <span className="tabular">{t("code_expires_in", formatCountdown(at - now))}</span>;
}

function QrPopover({ onClose }: { onClose: () => void }) {
  const { state } = useStore();
  const { t } = useI18n();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);
  return (
    <div
      ref={ref}
      role="dialog"
      aria-label={t("show_qr")}
      className="sheet-in absolute top-full left-0 z-30 mt-2 flex flex-col items-center gap-3 rounded-card bg-surface p-4"
      style={{ boxShadow: "inset 0 0 0 1px var(--border), 0 10px 32px rgba(0,0,0,0.16)" }}
    >
      <QrCode text={state.code.qrPayload} size={196} label={t("scan_qr")} />
      <div className="flex flex-col items-center gap-0.5">
        <span className="type-section tabular tracking-[0.04em]">{state.code.display}</span>
        <span className="type-caption text-text-secondary">
          <ExpiresIn at={state.code.expiresAtMs} />
        </span>
      </div>
    </div>
  );
}

export function CodeBar() {
  const { state } = useStore();
  const { t } = useI18n();
  const actions = useActions();
  const { copied, copy } = useCopyCode();
  const [qrOpen, setQrOpen] = useState(false);
  const [entry, setEntry] = useState<CodeEntry>(emptyCode);
  const [busy, setBusy] = useState(false);

  const submit = async (e: CodeEntry) => {
    if (busy || e.digits.length !== 6) return;
    setBusy(true);
    try {
      if (await actions.sendToCode(e)) setEntry(emptyCode);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex shrink-0 flex-wrap items-center gap-x-6 gap-y-3 px-5 pb-4">
      <div className="relative flex items-center gap-3">
        <span className="type-section text-text-secondary">{t("your_code")}</span>
        <span className="type-code select-text" aria-live="polite">
          {state.code.display}
        </span>
        <div className="flex items-center">
          <IconButton label={t("copy_code")} onClick={() => void copy()}>
            {copied ? <Check size={16} className="text-success" /> : <Copy size={16} strokeWidth={1.75} />}
          </IconButton>
          <IconButton label={t("show_qr")} aria-expanded={qrOpen} onClick={() => setQrOpen((v) => !v)}>
            <QrIcon size={16} strokeWidth={1.75} />
          </IconButton>
        </div>
        {qrOpen && <QrPopover onClose={() => setQrOpen(false)} />}
      </div>
      <div className="ml-auto flex items-center gap-2" title={t("send_to_code_hint")}>
        <span className="type-section text-text-secondary">{t("send_to_code")}</span>
        <CodeInput label={t("send_to_code")} value={entry} onChange={setEntry} onComplete={(e) => void submit(e)} disabled={busy} />
        <IconButton
          label={t("send")}
          disabled={entry.digits.length !== 6 || busy}
          onClick={() => void submit(entry)}
          className={entry.digits.length === 6 ? "text-accent!" : ""}
        >
          <ArrowRight size={17} strokeWidth={1.75} />
        </IconButton>
      </div>
    </div>
  );
}
