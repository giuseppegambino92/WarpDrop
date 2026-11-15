// App state: one reducer fed by the initial snapshot and engine events.
import { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  CodeInfo,
  DeviceIdentity,
  IncomingOffer,
  Lang,
  Material,
  PeerInfo,
  Platform,
  Settings,
  Snapshot,
  TransferSnapshot,
  TrustedDevice,
  WarpEvent,
} from "./types";

export type Tab = "my" | "nearby" | "code";

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  text: string;
}

export type LinkStatus =
  | { kind: "idle" }
  | { kind: "pending" }
  | { kind: "linked"; device: TrustedDevice }
  | { kind: "failed"; message: string; localized: boolean };

export interface AppState {
  identity: DeviceIdentity;
  code: CodeInfo;
  peers: PeerInfo[];
  trusted: TrustedDevice[];
  /** Newest first. */
  transfers: TransferSnapshot[];
  settings: Settings;
  lang: Lang;
  platform: Platform;
  material: Material;
  version: string;
  offers: IncomingOffer[];
  staged: string[];
  selectedPeerId: string | null;
  tab: Tab;
  link: LinkStatus;
  /** Device ids linked during this session (for the arrival animation). */
  freshlyLinked: string[];
  toasts: Toast[];
}

type Action =
  | { type: "event"; event: WarpEvent }
  | { type: "transfers"; transfers: TransferSnapshot[] }
  | { type: "settings"; settings: Settings }
  | { type: "lang"; lang: Lang }
  | { type: "stage"; paths: string[] }
  | { type: "clearStaged" }
  | { type: "select"; peerId: string | null }
  | { type: "tab"; tab: Tab }
  | { type: "link"; status: LinkStatus }
  | { type: "dismissOffer"; offerId: string }
  | { type: "toast"; toast: Toast }
  | { type: "dropToast"; id: number };

/** Newest first, by engine start time (id breaks ties deterministically). */
function sortTransfers(list: TransferSnapshot[]): TransferSnapshot[] {
  return list.sort((a, b) => b.startedAtMs - a.startedAtMs || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

function upsertTransfer(list: TransferSnapshot[], t: TransferSnapshot): TransferSnapshot[] {
  const i = list.findIndex((x) => x.id === t.id);
  if (i === -1) return sortTransfers([t, ...list]);
  const next = list.slice();
  next[i] = t;
  return next;
}

function reduce(s: AppState, a: Action): AppState {
  switch (a.type) {
    case "event": {
      const e = a.event;
      switch (e.type) {
        case "peersChanged":
          return { ...s, peers: e.peers };
        case "codeChanged":
          return { ...s, code: e.code };
        case "trustRingChanged":
          return { ...s, trusted: e.devices };
        case "offerReceived":
          return s.offers.some((o) => o.offerId === e.offer.offerId) ? s : { ...s, offers: [...s.offers, e.offer] };
        case "offerWithdrawn":
          return { ...s, offers: s.offers.filter((o) => o.offerId !== e.offerId) };
        case "transferUpdated":
          return { ...s, transfers: upsertTransfer(s.transfers, e.transfer) };
        case "deviceLinked": {
          const trusted = s.trusted.some((d) => d.deviceId === e.device.deviceId) ? s.trusted : [...s.trusted, e.device];
          return {
            ...s,
            trusted,
            link: { kind: "linked", device: e.device },
            freshlyLinked: [...s.freshlyLinked, e.device.deviceId],
          };
        }
        case "error":
          return s.link.kind === "pending" ? { ...s, link: { kind: "failed", message: e.message, localized: false } } : s;
        case "clipboardReceived":
          return s;
      }
      return s;
    }
    case "transfers":
      return { ...s, transfers: sortTransfers([...a.transfers]) };
    case "settings":
      return { ...s, settings: a.settings };
    case "lang":
      return { ...s, lang: a.lang };
    case "stage": {
      const merged = [...s.staged, ...a.paths.filter((p) => !s.staged.includes(p))];
      return { ...s, staged: merged };
    }
    case "clearStaged":
      return { ...s, staged: [] };
    case "select":
      return { ...s, selectedPeerId: a.peerId };
    case "tab":
      return { ...s, tab: a.tab };
    case "link":
      return { ...s, link: a.status };
    case "dismissOffer":
      return { ...s, offers: s.offers.filter((o) => o.offerId !== a.offerId) };
    case "toast":
      return { ...s, toasts: [...s.toasts.slice(-3), a.toast] };
    case "dropToast":
      return { ...s, toasts: s.toasts.filter((t) => t.id !== a.id) };
  }
}

function initialState(snap: Snapshot): AppState {
  return {
    identity: snap.identity,
    code: snap.code,
    peers: snap.peers,
    trusted: snap.trusted,
    transfers: sortTransfers([...snap.transfers]),
    settings: snap.settings,
    lang: snap.language,
    platform: snap.platform,
    material: snap.material,
    version: snap.version,
    offers: [],
    staged: [],
    selectedPeerId: snap.settings.lastPeerId,
    tab: snap.trusted.length > 0 ? "my" : "nearby",
    link: { kind: "idle" },
    freshlyLinked: [],
    toasts: [],
  };
}

interface Store {
  state: AppState;
  dispatch: (a: Action) => void;
  toast: (kind: Toast["kind"], text: string) => void;
}

const StoreCtx = createContext<Store | null>(null);

let toastSeq = 0;

export function StoreProvider({ snapshot, children }: { snapshot: Snapshot; children: ReactNode }) {
  const [state, dispatch] = useReducer(reduce, snapshot, initialState);

  const toast = useCallback((kind: Toast["kind"], text: string) => {
    const id = ++toastSeq;
    dispatch({ type: "toast", toast: { id, kind, text } });
    window.setTimeout(() => dispatch({ type: "dropToast", id }), kind === "error" ? 6000 : 3500);
  }, []);

  const value = useMemo(() => ({ state, dispatch, toast }), [state, toast]);
  return <StoreCtx.Provider value={value}>{children}</StoreCtx.Provider>;
}

export function useStore(): Store {
  const s = useContext(StoreCtx);
  if (!s) throw new Error("useStore outside StoreProvider");
  return s;
}

// One `warp-event` listener for the app's lifetime. Events that arrive before
// the UI mounts (e.g. while the snapshot loads) are buffered and replayed.
const hub: { sink: ((e: WarpEvent) => void) | null; buffer: WarpEvent[]; started: boolean } = {
  sink: null,
  buffer: [],
  started: false,
};

export async function startEventHub(): Promise<void> {
  if (hub.started) return;
  hub.started = true;
  await listen<WarpEvent>("warp-event", (msg) => {
    if (hub.sink) hub.sink(msg.payload);
    else hub.buffer.push(msg.payload);
  });
}

export function useWarpEvents(handler: (e: WarpEvent) => void) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    hub.sink = (e) => ref.current(e);
    for (const e of hub.buffer.splice(0)) ref.current(e);
    return () => {
      hub.sink = null;
    };
  }, []);
}

/** Card model shared by both device tabs. */
export interface DeviceModel {
  deviceId: string;
  name: string;
  kind: PeerInfo["kind"];
  trusted: boolean;
  online: boolean;
  bestPath: PeerInfo["bestPath"];
}

export function myDevices(s: AppState): DeviceModel[] {
  return s.trusted
    .map((d) => {
      const peer = s.peers.find((p) => p.deviceId === d.deviceId);
      return {
        deviceId: d.deviceId,
        name: peer?.name ?? d.name,
        kind: peer?.kind ?? d.kind,
        trusted: true,
        online: d.online || !!peer,
        bestPath: peer?.bestPath ?? null,
      };
    })
    .sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name));
}

export function nearbyDevices(s: AppState): DeviceModel[] {
  return s.peers
    .filter((p) => !p.trusted && !s.trusted.some((d) => d.deviceId === p.deviceId))
    .map((p) => ({ deviceId: p.deviceId, name: p.name, kind: p.kind, trusted: false, online: true, bestPath: p.bestPath }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

export function deviceName(s: AppState, id: string): string | null {
  return s.peers.find((p) => p.deviceId === id)?.name ?? s.trusted.find((d) => d.deviceId === id)?.name ?? null;
}
