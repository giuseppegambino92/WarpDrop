// Main window: header, code bar, device grid / quick code, transfer HUD.
import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { X } from "lucide-react";
import { localizeEngineError, useI18n } from "./i18n";
import { useActions } from "./lib/actions";
import { basename } from "./lib/format";
import { isTextTarget } from "./lib/hooks";
import { useStore, useWarpEvents } from "./lib/store";
import type { WarpEvent } from "./lib/types";
import { CodeBar } from "./components/CodeBar";
import { DeviceGrid, type DragState } from "./components/DeviceGrid";
import { Header } from "./components/Header";
import { LinkSheet } from "./components/LinkSheet";
import { OfferModal } from "./components/OfferModal";
import { QuickCode } from "./components/QuickCode";
import { SettingsPanel } from "./components/SettingsPanel";
import { Toasts } from "./components/Toasts";
import { TransferHud } from "./components/TransferHud";
import { Button } from "./components/ui";

/**
 * Device id of the card under a drag position. Tauri labels the position
 * "physical", but wry reports WebView2 client pixels (physical) on Windows and
 * view/widget coordinates (already CSS px) on macOS and WebKitGTK.
 */
function peerAt(x: number, y: number): string | null {
  const scale = document.documentElement.dataset.platform === "windows" ? window.devicePixelRatio || 1 : 1;
  const el = document.elementFromPoint(x / scale, y / scale);
  return el?.closest<HTMLElement>("[data-peer-id]")?.dataset.peerId ?? null;
}

function StagedBar() {
  const { state, dispatch } = useStore();
  const { t } = useI18n();
  const n = state.staged.length;
  const what = n === 1 ? basename(state.staged[0] ?? "") : t("items_count", n);
  return (
    <div className="spring-in flex items-center gap-3 rounded-card bg-accent-soft px-4 py-2.5" role="status">
      <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-text">{t("staged_items", what)}</span>
      <Button size="sm" variant="ghost" onClick={() => dispatch({ type: "clearStaged" })}>
        <X size={14} />
        {t("staged_clear")}
      </Button>
    </div>
  );
}

export function App() {
  const { state, dispatch, toast } = useStore();
  const { t } = useI18n();
  const actions = useActions();
  const [drag, setDrag] = useState<DragState>({ active: false, hoverId: null });
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [linkOpen, setLinkOpen] = useState(false);

  const modalOpen = settingsOpen || linkOpen || state.offers.length > 0;
  const live = useRef({ state, actions, modalOpen, linkOpen, t });
  live.current = { state, actions, modalOpen, linkOpen, t };

  // ---- engine events ----
  useWarpEvents(
    useCallback(
      (e: WarpEvent) => {
        const { state: s, linkOpen: sheet, t: tr } = live.current;
        dispatch({ type: "event", event: e });
        switch (e.type) {
          case "error":
            if (s.link.kind !== "pending") toast("error", localizeEngineError(tr, e.message));
            break;
          case "deviceLinked":
            if (!sheet) toast("success", tr("link_success", e.device.name));
            break;
          case "clipboardReceived":
            toast(e.written ? "success" : "error", e.written ? `${tr("clipboard_received", e.fromName)} — ${tr("clipboard_received_body")}` : tr("error_clipboard_read"));
            break;
          default:
            break;
        }
      },
      [dispatch, toast],
    ),
  );

  // ---- native drag & drop (real paths + position) ----
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        const { state: s, actions: a, modalOpen: modal, t: tr } = live.current;
        switch (p.type) {
          case "enter":
          case "over":
            setDrag({ active: true, hoverId: modal ? null : peerAt(p.position.x, p.position.y) });
            break;
          case "drop": {
            setDrag({ active: false, hoverId: null });
            if (p.paths.length === 0) break;
            const target = modal || s.tab === "code" ? null : peerAt(p.position.x, p.position.y);
            if (target) {
              void a.sendToPeer(target, [...s.staged, ...p.paths]);
              if (s.staged.length) dispatch({ type: "clearStaged" });
            } else {
              dispatch({ type: "stage", paths: p.paths });
              if (s.tab === "code") toast("info", tr("staged_items", tr("items_count", s.staged.length + p.paths.length)));
            }
            break;
          }
          case "leave":
            setDrag({ active: false, hoverId: null });
            break;
        }
      })
      .then((u) => {
        if (cancelled) u();
        else unlisten = u;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [dispatch, toast]);

  // ---- ⌘V / Ctrl+V outside text fields = Send Clipboard to the selected device ----
  useEffect(() => {
    const trigger = () => {
      const { state: s, actions: a } = live.current;
      void a.sendClipboard(s.selectedPeerId);
    };
    const onKey = (e: KeyboardEvent) => {
      if (live.current.modalOpen || e.altKey || e.shiftKey) return;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v" && !isTextTarget(document.activeElement)) {
        e.preventDefault();
        trigger();
      }
    };
    const onPaste = (e: ClipboardEvent) => {
      if (live.current.modalOpen || isTextTarget(e.target) || isTextTarget(document.activeElement)) return;
      e.preventDefault();
      trigger();
    };
    window.addEventListener("keydown", onKey);
    document.addEventListener("paste", onPaste);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.removeEventListener("paste", onPaste);
    };
  }, []);

  const openLink = useCallback(() => setLinkOpen(true), []);
  const closeLink = useCallback(() => setLinkOpen(false), []);
  const closeSettings = useCallback(() => setSettingsOpen(false), []);

  const showDeviceArea = state.tab !== "code";

  return (
    <div className="flex h-full flex-col">
      <Header onSettings={() => setSettingsOpen(true)} onLink={openLink} />
      <CodeBar />
      <div className="mx-5 h-px shrink-0 bg-border" />
      <main className="min-h-0 flex-1 overflow-y-auto px-5 pt-5 pb-5">
        {showDeviceArea ? (
          <div className="flex flex-col gap-5">
            <DeviceGrid drag={drag} onLink={openLink} />
            {state.staged.length > 0 ? (
              <StagedBar />
            ) : (
              <p className={`text-center type-body transition-colors duration-150 ${drag.active ? "text-accent" : "text-text-tertiary"}`}>
                {drag.active ? t("drop_on_device") : t("drop_hint")}
              </p>
            )}
          </div>
        ) : (
          <QuickCode onLink={openLink} />
        )}
      </main>
      <TransferHud />
      <Toasts />
      {settingsOpen && <SettingsPanel onClose={closeSettings} />}
      {linkOpen && <LinkSheet onClose={closeLink} />}
      <OfferModal />
    </div>
  );
}
