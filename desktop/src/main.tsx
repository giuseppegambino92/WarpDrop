import { StrictMode, useEffect, useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "./styles.css";
import { App } from "./App";
import { I18nProvider, translate } from "./i18n";
import { api } from "./lib/api";
import { startEventHub, StoreProvider, useStore } from "./lib/store";
import type { BootInfo, Snapshot } from "./lib/types";
import { Droplet } from "./components/ui";

function applyChrome(info: { platform: string; material: string; language: string }) {
  const root = document.documentElement;
  root.dataset.platform = info.platform;
  root.dataset.material = info.material;
  root.lang = info.language;
}

/** Keeps the I18n provider and <html lang> in sync with the store's language. */
function LangBridge({ children }: { children: ReactNode }) {
  const { state } = useStore();
  useEffect(() => {
    document.documentElement.lang = state.lang;
  }, [state.lang]);
  return <I18nProvider lang={state.lang}>{children}</I18nProvider>;
}

function Fatal({ boot, message }: { boot: BootInfo | null; message: string }) {
  const lang = boot?.language ?? "en";
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center" data-tauri-drag-region>
      <span className="text-accent">
        <Droplet size={28} />
      </span>
      <p className="max-w-md type-body text-text-secondary select-text">{translate(lang, "error_generic", message)}</p>
    </div>
  );
}

function Root() {
  const [boot, setBoot] = useState<BootInfo | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const info = await api.bootInfo();
        if (!alive) return;
        setBoot(info);
        applyChrome(info);
        if (info.error !== null) {
          setError(info.error || "engine not started");
          return;
        }
        await startEventHub();
        const snap = await api.snapshot();
        if (alive) setSnapshot(snap);
      } catch (e) {
        if (alive) setError(typeof e === "object" && e && "message" in e ? String((e as { message: unknown }).message) : String(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  if (error !== null) return <Fatal boot={boot} message={error} />;
  if (!snapshot) return <div className="h-full" data-tauri-drag-region />;
  return (
    <StoreProvider snapshot={snapshot}>
      <LangBridge>
        <App />
      </LangBridge>
    </StoreProvider>
  );
}

const container = document.getElementById("root");
if (container) {
  createRoot(container).render(
    <StrictMode>
      <Root />
    </StrictMode>,
  );
}
