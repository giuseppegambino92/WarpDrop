// Localization: strings generated from i18n/strings.json (+ desktop extras)
// by scripts/gen-i18n.mjs. The resolved language comes from Rust, which owns
// the System/English/Italiano preference so the tray matches the window.
import { createContext, useCallback, useContext, useMemo, type ReactNode } from "react";
import en from "./en.json";
import it from "./it.json";
import type { CmdError, Lang, TransferSnapshot } from "../lib/types";
import { isCmdError } from "../lib/types";

export type StringKey = keyof typeof en;
const tables: Record<Lang, Record<StringKey, string>> = { en, it };

export type Translate = (key: StringKey, ...args: (string | number)[]) => string;

export function translate(lang: Lang, key: StringKey, ...args: (string | number)[]): string {
  let s = tables[lang][key] ?? tables.en[key] ?? key;
  args.forEach((a, i) => {
    s = s.split(`{${i}}`).join(String(a));
  });
  return s;
}

interface I18n {
  lang: Lang;
  t: Translate;
}

const Ctx = createContext<I18n>({ lang: "en", t: (k, ...a) => translate("en", k, ...a) });

export function I18nProvider({ lang, children }: { lang: Lang; children: ReactNode }) {
  const t = useCallback<Translate>((key, ...args) => translate(lang, key, ...args), [lang]);
  const value = useMemo(() => ({ lang, t }), [lang, t]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useI18n(): I18n {
  return useContext(Ctx);
}

export function capitalize(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** Peer name for messages; "the other device" when the engine doesn't know it yet. */
export function peerLabel(t: Translate, name: string | null | undefined): string {
  return name && name.trim() ? name : t("other_device");
}

/** Localizes an engine error string (WarpError Display / transfer.error). */
export function localizeEngineError(t: Translate, message: string, peerName?: string | null): string {
  const m = message.trim();
  const lower = m.toLowerCase();
  const peer = peerLabel(t, peerName);
  if (lower === "wrong code" || lower === "invalid code") return t("error_wrong_code");
  if (lower.startsWith("no device is currently showing")) return t("error_code_not_found");
  if (lower.startsWith("rejected by the other device")) return capitalize(t("error_declined", peer));
  if (lower === "device not found" || lower.startsWith("unreachable")) return t("error_unreachable", peer);
  if (lower === "timed out") return capitalize(t("error_timeout", peer));
  if (lower === "cancelled") return t("state_cancelled");
  if (lower === "connection lost") return t("error_connection_lost");
  if (lower.startsWith("i/o error:")) return t("error_disk", m.slice("i/o error:".length).trim());
  return t("error_generic", m);
}

/** Localizes a failed transfer from its stable `errorCode`. */
export function localizeTransferError(t: Translate, x: Pick<TransferSnapshot, "error" | "errorCode" | "peerName">): string {
  const peer = peerLabel(t, x.peerName);
  switch (x.errorCode) {
    case "wrong_code":
      return t("error_wrong_code");
    case "code_not_found":
      return t("error_code_not_found");
    case "declined":
      return capitalize(t("error_declined", peer));
    case "rejected":
      return capitalize(t("error_rejected", peer));
    case "unreachable":
      return t("error_unreachable", peer);
    case "timeout":
      return capitalize(t("error_timeout", peer));
    case "cancelled":
      return t("state_cancelled");
    case "io":
      return t("error_disk", x.error ?? "");
    case "integrity":
      return t("error_integrity");
    case "protocol":
      return t("error_generic", x.error ?? t("state_failed"));
    default:
      // Unknown/absent code (older engine): best-effort on the message.
      return x.error ? localizeEngineError(t, x.error, x.peerName) : t("state_failed");
  }
}

/** Localizes an error thrown by an `invoke` call. */
export function localizeCmdError(t: Translate, e: unknown, peerName?: string | null): string {
  if (!isCmdError(e)) return t("error_generic", String(e));
  const err: CmdError = e;
  const peer = peerLabel(t, peerName);
  switch (err.code) {
    case "invalidCode":
    case "wrongCode":
      return t("error_wrong_code");
    case "codeNotFound":
      return t("error_code_not_found");
    case "peerNotFound":
    case "unreachable":
      return t("error_unreachable", peer);
    case "rejected":
      return capitalize(t("error_declined", peer));
    case "timeout":
      return capitalize(t("error_timeout", peer));
    case "cancelled":
      return t("state_cancelled");
    case "clipboardEmpty":
      return t("clipboard_empty");
    case "clipboardRead":
      return t("error_clipboard_read");
    case "pickDevice":
      return t("error_pick_device");
    case "invalidRelay":
      return t("settings_relay_invalid");
    case "io":
      return t("error_disk", err.message);
    default:
      return localizeEngineError(t, err.message, peerName);
  }
}

/** Engine titles are English ("Clipboard", "Image", "3 items"); localize those. */
export function localizeTitle(t: Translate, title: string): string {
  if (title === "Clipboard") return t("clipboard_title");
  if (title === "Image") return t("image_title");
  const m = /^(\d+) items$/.exec(title);
  if (m) return t("items_count", m[1] ?? "");
  return title;
}
