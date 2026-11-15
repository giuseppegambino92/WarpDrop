// "Link My Device": show our code (opens the link window) or enter theirs.
import { useEffect, useRef, useState } from "react";
import { CheckCircle2 } from "lucide-react";
import { localizeCmdError, localizeEngineError, useI18n } from "../i18n";
import { api } from "../lib/api";
import { codeTarget, emptyCode, formatCountdown, type CodeEntry } from "../lib/format";
import { useNow } from "../lib/hooks";
import { useStore } from "../lib/store";
import { CodeInput } from "./CodeInput";
import { QrCode } from "./Qr";
import { Button, DeviceGlyph, Segmented, Sheet } from "./ui";

const LINK_WINDOW_SECS = 300;

function ShowMyCode() {
  const { state, toast } = useStore();
  const { t } = useI18n();
  const now = useNow(1000);
  const [deadline, setDeadline] = useState(() => Date.now() + LINK_WINDOW_SECS * 1000);
  const remaining = deadline - now;

  useEffect(() => {
    api.openLinkWindow(LINK_WINDOW_SECS).catch((e: unknown) => toast("error", localizeCmdError(t, e)));
    return () => {
      void api.closeLinkWindow().catch(() => undefined);
    };
    // Opened once per mount (not on language change); reopening after
    // expiry is explicit, below.
  }, []);

  const reopen = () => {
    setDeadline(Date.now() + LINK_WINDOW_SECS * 1000);
    api.openLinkWindow(LINK_WINDOW_SECS).catch((e: unknown) => toast("error", localizeCmdError(t, e)));
  };

  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <QrCode text={state.code.qrPayload} size={180} label={t("scan_qr")} />
      <span className="type-code select-text">{state.code.display}</span>
      <p className="max-w-xs type-body text-text-secondary">{t("link_show_code_body")}</p>
      {remaining > 0 ? (
        <span className="flex items-center gap-2 type-caption text-text-secondary">
          <span className="searching-dot h-1.5 w-1.5 rounded-full bg-accent" aria-hidden />
          {t("link_waiting")} · <span className="tabular">{t("code_expires_in", formatCountdown(remaining))}</span>
        </span>
      ) : (
        <Button size="sm" onClick={reopen}>
          {t("link_show_code")}
        </Button>
      )}
    </div>
  );
}

function EnterCode() {
  const { state, dispatch } = useStore();
  const { t } = useI18n();
  const [entry, setEntry] = useState<CodeEntry>(emptyCode);
  const pending = state.link.kind === "pending";

  const submit = async (e: CodeEntry) => {
    if (e.digits.length !== 6 || pending) return;
    dispatch({ type: "link", status: { kind: "pending" } });
    try {
      await api.linkWithCode(codeTarget(e));
    } catch (err) {
      dispatch({ type: "link", status: { kind: "failed", message: localizeCmdError(t, err), localized: true } });
    }
  };

  const error =
    state.link.kind === "failed" ? (state.link.localized ? state.link.message : localizeEngineError(t, state.link.message)) : null;

  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <p className="max-w-xs type-body text-text-secondary">{t("link_enter_code_body")}</p>
      <CodeInput large autoFocus label={t("link_enter_code")} value={entry} onChange={setEntry} onComplete={(e) => void submit(e)} disabled={pending} />
      <div className="min-h-5 type-caption">
        {pending && <span className="text-text-secondary">{t("linking")}</span>}
        {error && <span className="text-danger">{error}</span>}
      </div>
      <Button variant="primary" disabled={entry.digits.length !== 6 || pending} onClick={() => void submit(entry)}>
        {t("link")}
      </Button>
    </div>
  );
}

export function LinkSheet({ onClose }: { onClose: () => void }) {
  const { state, dispatch } = useStore();
  const { t } = useI18n();
  const [mode, setMode] = useState<"show" | "enter">("show");
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  // Fresh status each time the sheet opens.
  useEffect(() => {
    dispatch({ type: "link", status: { kind: "idle" } });
    return () => dispatch({ type: "link", status: { kind: "idle" } });
  }, [dispatch]);

  useEffect(() => {
    if (state.link.kind !== "linked") return;
    const id = window.setTimeout(() => closeRef.current(), 1800);
    return () => window.clearTimeout(id);
  }, [state.link.kind]);

  return (
    <Sheet title={t("link_device")} onClose={onClose} width={420}>
      {state.link.kind === "linked" ? (
        <div className="spring-in flex flex-col items-center gap-3 py-6 text-center">
          <span className="relative flex h-14 w-14 items-center justify-center rounded-card bg-surface-hover text-text-secondary">
            <DeviceGlyph kind={state.link.device.kind} size={36} />
            <CheckCircle2 size={20} className="absolute -right-1.5 -bottom-1.5 rounded-full bg-surface text-success" />
          </span>
          <span className="type-section">{t("link_success", state.link.device.name)}</span>
        </div>
      ) : (
        <div className="flex flex-col items-center gap-5">
          <Segmented
            label={t("link_device")}
            value={mode}
            onChange={(m) => {
              setMode(m);
              dispatch({ type: "link", status: { kind: "idle" } });
            }}
            options={[
              { value: "show", label: t("link_show_code") },
              { value: "enter", label: t("link_enter_code") },
            ]}
          />
          {mode === "show" ? <ShowMyCode /> : <EnterCode />}
        </div>
      )}
    </Sheet>
  );
}
