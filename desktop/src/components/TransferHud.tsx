// Live transfers. Every number shown comes from the engine's snapshots.
import { ArrowDownLeft, ArrowUpRight, FolderOpen, Pause, Play, X } from "lucide-react";
import { localizeCmdError, localizeTitle, localizeTransferError, peerLabel, useI18n, type Translate } from "../i18n";
import { api } from "../lib/api";
import { formatBytes, formatSpeed } from "../lib/format";
import { useStore } from "../lib/store";
import type { Lang, TransferSnapshot } from "../lib/types";
import { IconButton, PathBadge } from "./ui";

const terminal = (t: TransferSnapshot) => t.state === "Completed" || t.state === "Failed" || t.state === "Cancelled";

function eta(t: Translate, secs: number | null): string | null {
  if (secs === null) return null;
  if (secs < 60) return t("eta_seconds", Math.max(1, Math.round(secs)));
  return t("eta_minutes", Math.ceil(secs / 60));
}

function statusLine(t: Translate, lang: Lang, x: TransferSnapshot): { text: string; tone: string } {
  const peer = peerLabel(t, x.peerName);
  const sizes = `${formatBytes(lang, x.doneBytes)} / ${formatBytes(lang, x.totalBytes)}`;
  switch (x.state) {
    case "Connecting":
      return { text: t("state_connecting"), tone: "text-text-secondary" };
    case "WaitingForAccept":
      return { text: t("state_waiting", peer), tone: "text-text-secondary" };
    case "Reconnecting":
      return { text: `${t("state_reconnecting")} · ${sizes}`, tone: "text-warning" };
    case "Verifying":
      return { text: t("state_verifying"), tone: "text-text-secondary" };
    case "Paused":
      return { text: `${t("state_paused")} · ${sizes}`, tone: "text-text-secondary" };
    case "Transferring": {
      const parts = [sizes];
      if (x.speedBps > 0) parts.push(formatSpeed(lang, x.speedBps));
      const e = eta(t, x.etaSecs);
      if (e) parts.push(e);
      return { text: parts.join(" · "), tone: "text-text-secondary" };
    }
    case "Completed":
      return {
        text: `✓ ${x.direction === "Outgoing" ? t("state_sent") : t("state_saved")} · ${formatBytes(lang, x.totalBytes)}`,
        tone: "text-success",
      };
    case "Failed":
      return { text: `✗ ${localizeTransferError(t, x)}`, tone: "text-danger" };
    case "Cancelled":
      return { text: t("state_cancelled"), tone: "text-text-tertiary" };
  }
}

function TransferRow({ x }: { x: TransferSnapshot }) {
  const { t, lang } = useI18n();
  const { toast } = useStore();
  const outgoing = x.direction === "Outgoing";
  const done = x.state === "Completed";
  const pct = x.totalBytes > 0 ? Math.min(100, (x.doneBytes / x.totalBytes) * 100) : done ? 100 : 0;
  const status = statusLine(t, lang, x);
  const revealTarget = !outgoing && done ? x.savedPaths[0] : undefined;
  const run = (p: Promise<void>) => p.catch((e: unknown) => toast("error", localizeCmdError(t, e, x.peerName)));
  const name = x.peerName || (outgoing ? "…" : t("unknown_device"));

  return (
    <li className="flex flex-col gap-1.5 py-2.5">
      <div className="flex items-center gap-2">
        <span className="text-text-tertiary" aria-hidden>
          {outgoing ? <ArrowUpRight size={15} strokeWidth={1.75} /> : <ArrowDownLeft size={15} strokeWidth={1.75} />}
        </span>
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">
          {localizeTitle(t, x.title || "…")}
          <span className="text-text-tertiary"> {outgoing ? "→" : "←"} </span>
          <span className="text-text-secondary">{name}</span>
        </span>
        <PathBadge path={x.path} />
        {!terminal(x) && x.totalBytes > 0 && (
          <span className="w-10 text-right type-caption text-text-secondary tabular">{Math.floor(pct)}%</span>
        )}
        <div className="flex items-center">
          {outgoing && !terminal(x) && x.state !== "Paused" && x.state !== "Connecting" && x.state !== "WaitingForAccept" && (
            <IconButton label={t("pause")} onClick={() => void run(api.pause(x.id))} className="h-7 w-7">
              <Pause size={14} strokeWidth={1.75} />
            </IconButton>
          )}
          {outgoing && x.state === "Paused" && (
            <IconButton label={t("resume")} onClick={() => void run(api.resume(x.id))} className="h-7 w-7">
              <Play size={14} strokeWidth={1.75} />
            </IconButton>
          )}
          {!terminal(x) && (
            <IconButton label={t("cancel")} onClick={() => void run(api.cancel(x.id))} className="h-7 w-7">
              <X size={15} strokeWidth={1.75} />
            </IconButton>
          )}
          {revealTarget && (
            <button
              type="button"
              onClick={() => void run(api.reveal(revealTarget))}
              className="inline-flex h-7 items-center gap-1 rounded-control px-2 type-caption text-success hover:bg-surface-hover"
            >
              <FolderOpen size={14} strokeWidth={1.75} />
              {t("show_in_folder")}
            </button>
          )}
        </div>
      </div>
      <div
        className="h-1 w-full overflow-hidden rounded-full bg-surface-hover"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(pct)}
      >
        <div
          className={`progress-fill h-full rounded-full ${done ? "bg-success" : x.state === "Failed" ? "bg-danger" : "bg-accent"}`}
          style={{ width: `${pct}%`, opacity: x.state === "Cancelled" ? 0.35 : 1 }}
        />
      </div>
      <span className={`truncate type-caption tabular ${status.tone}`} title={status.text}>
        {status.text}
      </span>
    </li>
  );
}

export function TransferHud() {
  const { state, dispatch } = useStore();
  const { t } = useI18n();
  if (state.transfers.length === 0) return null;
  const anyFinished = state.transfers.some(terminal);
  return (
    <section
      aria-label={t("notif_channel_transfers")}
      className="max-h-[42%] shrink-0 overflow-y-auto border-t border-border bg-surface px-5"
    >
      <div className="sticky top-0 z-10 -mx-5 flex h-8 items-center justify-between bg-surface px-5 pt-1">
        <span className="type-caption text-text-tertiary">{t("notif_channel_transfers")}</span>
        {anyFinished && (
          <button
            type="button"
            className="rounded-control px-2 py-0.5 type-caption text-text-secondary hover:bg-surface-hover hover:text-text"
            onClick={() => void api.clearFinished().then((transfers) => dispatch({ type: "transfers", transfers }))}
          >
            {t("clear_finished")}
          </button>
        )}
      </div>
      <ul className="divide-y divide-border">
        {state.transfers.map((x) => (
          <TransferRow key={x.id} x={x} />
        ))}
      </ul>
    </section>
  );
}
