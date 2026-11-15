// Number, size, speed, time and code formatting.
import type { Lang } from "./types";

function num(lang: Lang, v: number, digits: number): string {
  return new Intl.NumberFormat(lang, { maximumFractionDigits: digits, minimumFractionDigits: 0 }).format(v);
}

/** Decimal units (1 MB = 1,000,000 bytes), matching the engine and the other apps. */
export function formatBytes(lang: Lang, bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  const digits = i === 0 || v >= 100 ? 0 : 1;
  return `${num(lang, v, digits)} ${units[i]}`;
}

/** `412 MB/s`, switching to `Gbit/s` above 1000 MB/s. */
export function formatSpeed(lang: Lang, bytesPerSec: number): string {
  const mbs = bytesPerSec / 1e6;
  if (mbs >= 1000) return `${num(lang, (bytesPerSec * 8) / 1e9, 1)} Gbit/s`;
  if (mbs >= 1) return `${num(lang, mbs, mbs >= 100 ? 0 : 1)} MB/s`;
  return `${num(lang, bytesPerSec / 1e3, 0)} KB/s`;
}

/** `m:ss` for countdowns. */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** Up to six digits, with the dash after the third: `482-91`. */
export function formatCodeDigits(digits: string): string {
  return digits.length > 3 ? `${digits.slice(0, 3)}-${digits.slice(3)}` : digits;
}

export interface CodeEntry {
  /** 0–6 digits. */
  digits: string;
  /** Full `warpdrop://` link when one was pasted (carries addresses + key). */
  uri: string | null;
}

export const emptyCode: CodeEntry = { digits: "", uri: null };

/** Interprets pasted or typed text: `482910`, `482-910`, `482 910`, `warpdrop://c/482910?…`. */
export function parseCodeText(text: string): CodeEntry | null {
  const t = text.trim();
  const link = /warpdrop:\/?\/?[^\s]+/i.exec(t);
  if (link) {
    const m = /(\d{3})[-\s]?(\d{3})(?!\d)/.exec(link[0].split("?")[0] ?? "");
    if (m) return { digits: `${m[1]}${m[2]}`, uri: link[0] };
    return null;
  }
  const digits = t.replace(/[\s-]/g, "");
  if (/^\d{1,6}$/.test(digits)) return { digits, uri: null };
  return null;
}

/** What to hand to the engine: the link when present (faster, pinned key), else the digits. */
export function codeTarget(entry: CodeEntry): string {
  return entry.uri ?? entry.digits;
}

export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
