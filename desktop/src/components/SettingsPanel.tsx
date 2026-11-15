// Settings slide-over: name, download folder, language, relay, login item, fingerprint.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Check, Copy, X } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { localizeCmdError, useI18n } from "../i18n";
import { api } from "../lib/api";
import { useStore } from "../lib/store";
import type { LanguagePref } from "../lib/types";
import { Button, IconButton, Segmented, Switch } from "./ui";

function Row({ label, hint, children, stacked = false }: { label: string; hint?: string; children: ReactNode; stacked?: boolean }) {
  return (
    <div className={`flex gap-3 py-3.5 ${stacked ? "flex-col" : "items-center justify-between"}`}>
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="type-section">{label}</span>
        {hint && <span className="type-caption text-text-tertiary">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

const field =
  "h-8 w-full rounded-control bg-surface-hover px-2.5 text-[13px] text-text hairline outline-none placeholder:text-text-tertiary focus:bg-surface focus:shadow-[inset_0_0_0_1.5px_var(--accent)]";

export function SettingsPanel({ onClose }: { onClose: () => void }) {
  const { state, dispatch, toast } = useStore();
  const { t } = useI18n();
  const s = state.settings;
  const [name, setName] = useState(s.deviceName);
  const [relay, setRelay] = useState(s.relayUrl ?? "");
  const [relayError, setRelayError] = useState<string | null>(null);
  const [fpCopied, setFpCopied] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    panelRef.current?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const fail = (e: unknown) => toast("error", localizeCmdError(t, e));

  const saveName = async () => {
    const v = name.trim();
    if (!v || v === s.deviceName) return setName(s.deviceName);
    try {
      dispatch({ type: "settings", settings: await api.setDeviceName(v) });
    } catch (e) {
      setName(s.deviceName);
      fail(e);
    }
  };

  const saveRelay = async () => {
    const v = relay.trim();
    if (v === (s.relayUrl ?? "")) return setRelayError(null);
    try {
      dispatch({ type: "settings", settings: await api.setRelayUrl(v || null) });
      setRelayError(null);
    } catch (e) {
      setRelayError(localizeCmdError(t, e));
    }
  };

  const chooseFolder = async () => {
    const dir = await open({ directory: true, multiple: false, defaultPath: s.downloadDir, title: t("settings_download_folder") });
    if (typeof dir !== "string") return;
    try {
      dispatch({ type: "settings", settings: await api.setDownloadDir(dir) });
    } catch (e) {
      fail(e);
    }
  };

  const setLanguage = async (language: LanguagePref) => {
    try {
      const lang = await api.setLanguage(language);
      dispatch({ type: "settings", settings: { ...s, language } });
      dispatch({ type: "lang", lang });
    } catch (e) {
      fail(e);
    }
  };

  const setLogin = async (enabled: boolean) => {
    try {
      dispatch({ type: "settings", settings: await api.setLaunchAtLogin(enabled) });
    } catch (e) {
      fail(e);
    }
  };

  const copyFingerprint = async () => {
    try {
      await api.copyText(state.identity.fingerprint);
      setFpCopied(true);
      window.setTimeout(() => setFpCopied(false), 1500);
    } catch (e) {
      fail(e);
    }
  };

  return (
    <div
      className="fade-in fixed inset-0 z-40 flex justify-end bg-[var(--scrim)]"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label={t("settings")}
        tabIndex={-1}
        className="panel-in flex h-full w-full max-w-[380px] flex-col bg-surface outline-none"
        style={{ boxShadow: "inset 1px 0 0 var(--border), -12px 0 40px rgba(0,0,0,0.12)" }}
      >
        <div className={`flex h-[52px] shrink-0 items-center justify-between px-5 ${state.platform === "macos" ? "pt-1" : ""}`}>
          <h2 className="type-title">{t("settings")}</h2>
          <IconButton label={t("close")} onClick={onClose}>
            <X size={16} />
          </IconButton>
        </div>
        <div className="flex-1 divide-y divide-border overflow-y-auto px-5 pb-6">
          <Row label={t("settings_device_name")} stacked>
            <input
              className={field}
              value={name}
              maxLength={64}
              onChange={(e) => setName(e.target.value)}
              onBlur={() => void saveName()}
              onKeyDown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
            />
          </Row>
          <Row label={t("settings_download_folder")} stacked>
            <div className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate rounded-control bg-surface-hover px-2.5 py-1.5 text-[13px] text-text-secondary hairline" title={s.downloadDir} dir="rtl">
                {/* rtl keeps the end of long paths visible */}
                <bdi>{s.downloadDir}</bdi>
              </span>
              <Button size="md" onClick={() => void chooseFolder()}>
                {t("settings_change")}
              </Button>
            </div>
          </Row>
          <Row label={t("settings_language")} stacked>
            <Segmented<LanguagePref>
              label={t("settings_language")}
              value={s.language}
              onChange={(v) => void setLanguage(v)}
              options={[
                { value: "system", label: t("settings_language_system") },
                { value: "en", label: t("settings_language_english") },
                { value: "it", label: t("settings_language_italian") },
              ]}
            />
          </Row>
          <Row label={t("settings_relay")} hint={t("settings_relay_hint")} stacked>
            <input
              className={field}
              value={relay}
              placeholder="wss://relay.example.com"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(e) => setRelay(e.target.value)}
              onBlur={() => void saveRelay()}
              onKeyDown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
            />
            {relayError && <span className="type-caption text-danger">{relayError}</span>}
          </Row>
          <Row label={t("settings_launch_at_login")}>
            <Switch label={t("settings_launch_at_login")} checked={s.launchAtLogin} onChange={(v) => void setLogin(v)} />
          </Row>
          <Row label={t("settings_fingerprint")} stacked>
            <div className="flex items-center gap-2">
              <code className="flex-1 font-mono text-[13px] tracking-[0.06em] text-text-secondary select-text">{state.identity.fingerprint}</code>
              <IconButton label={t("copy")} onClick={() => void copyFingerprint()}>
                {fpCopied ? <Check size={15} className="text-success" /> : <Copy size={15} strokeWidth={1.75} />}
              </IconButton>
            </div>
          </Row>
          <div className="pt-4 type-caption text-text-tertiary">{t("settings_version", state.version)}</div>
        </div>
      </div>
    </div>
  );
}
