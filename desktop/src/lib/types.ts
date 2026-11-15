// TypeScript mirrors of the libwarp FFI types (core-rust/libwarp/src/types.rs)
// as serialized by serde (camelCase fields, unit enums as their variant name).

export type DeviceKind = "Mac" | "IPhone" | "IPad" | "Android" | "Windows" | "Linux" | "Unknown";

export type PathKind =
  | "UsbCable"
  | "ThunderboltBridge"
  | "AdbUsb"
  | "WifiDirect"
  | "Ethernet"
  | "Wifi"
  | "Lan"
  | "Loopback"
  | "InternetP2p"
  | "Relay";

export type TrustOrigin = "ICloud" | "Linked" | "Ring";

export type TransferDirection = "Outgoing" | "Incoming";

export type TransferState =
  | "Connecting"
  | "WaitingForAccept"
  | "Transferring"
  | "Paused"
  | "Reconnecting"
  | "Verifying"
  | "Completed"
  | "Failed"
  | "Cancelled";

/** Stable `TransferSnapshot.errorCode` values. */
export type TransferErrorCode =
  | "wrong_code"
  | "code_not_found"
  | "declined"
  | "rejected"
  | "unreachable"
  | "timeout"
  | "cancelled"
  | "io"
  | "integrity"
  | "protocol";

export interface DeviceIdentity {
  deviceId: string;
  name: string;
  kind: DeviceKind;
  publicKeyHex: string;
  fingerprint: string;
}

export interface PeerInfo {
  deviceId: string;
  name: string;
  kind: DeviceKind;
  trusted: boolean;
  codeOpen: boolean;
  bestPath: PathKind | null;
  addresses: string[];
  lastSeenMs: number;
}

export interface TrustedDevice {
  deviceId: string;
  name: string;
  kind: DeviceKind;
  publicKeyHex: string;
  origin: TrustOrigin;
  addedAtMs: number;
  online: boolean;
}

export interface CodeInfo {
  code: string;
  display: string;
  expiresAtMs: number;
  qrPayload: string;
}

export interface QrMatrix {
  size: number;
  modules: boolean[];
}

export interface TransferSnapshot {
  id: string;
  direction: TransferDirection;
  peerId: string;
  peerName: string;
  title: string;
  fileCount: number;
  totalBytes: number;
  doneBytes: number;
  resumedBytes: number;
  wireBytes: number;
  /** Bytes per second. */
  speedBps: number;
  etaSecs: number | null;
  path: PathKind | null;
  state: TransferState;
  /** Human-readable (English) failure description. */
  error: string | null;
  /** Machine-readable failure code; localize from this, not from `error`. */
  errorCode: TransferErrorCode | string | null;
  /** Creation time (Unix ms), for stable ordering. */
  startedAtMs: number;
  savedPaths: string[];
}

export interface IncomingOffer {
  offerId: string;
  peerId: string;
  peerName: string;
  peerKind: DeviceKind;
  trusted: boolean;
  itemCount: number;
  totalBytes: number;
  title: string;
  sasEmoji: string[];
}

/** Events forwarded by the Rust bridge on the `warp-event` channel. */
export type WarpEvent =
  | { type: "peersChanged"; peers: PeerInfo[] }
  | { type: "codeChanged"; code: CodeInfo }
  | { type: "trustRingChanged"; devices: TrustedDevice[] }
  | { type: "offerReceived"; offer: IncomingOffer }
  | { type: "offerWithdrawn"; offerId: string }
  | { type: "transferUpdated"; transfer: TransferSnapshot }
  | {
      type: "clipboardReceived";
      fromDeviceId: string;
      fromName: string;
      kind: "text" | "url" | "image";
      preview: string;
      written: boolean;
    }
  | { type: "deviceLinked"; device: TrustedDevice }
  | { type: "error"; message: string };

// ---- app-side types (desktop/src-tauri) ----

export type Lang = "en" | "it";
export type LanguagePref = "system" | "en" | "it";
export type Platform = "macos" | "windows" | "linux";
export type Material = "vibrancy" | "mica" | "acrylic" | "none";

export interface Settings {
  deviceName: string;
  downloadDir: string;
  language: LanguagePref;
  relayUrl: string | null;
  launchAtLogin: boolean;
  lastPeerId: string | null;
}

export interface BootInfo {
  error: string | null;
  language: Lang;
  platform: Platform;
  material: Material;
}

export interface Snapshot {
  identity: DeviceIdentity;
  code: CodeInfo;
  peers: PeerInfo[];
  trusted: TrustedDevice[];
  transfers: TransferSnapshot[];
  settings: Settings;
  language: Lang;
  platform: Platform;
  material: Material;
  version: string;
}

export type CmdErrorCode =
  | "invalidCode"
  | "wrongCode"
  | "codeNotFound"
  | "peerNotFound"
  | "unreachable"
  | "rejected"
  | "timeout"
  | "cancelled"
  | "transferNotFound"
  | "io"
  | "generic"
  | "notReady"
  | "clipboardEmpty"
  | "clipboardRead"
  | "pickDevice"
  | "invalidName"
  | "invalidRelay";

export interface CmdError {
  code: CmdErrorCode;
  message: string;
}

export function isCmdError(e: unknown): e is CmdError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}
