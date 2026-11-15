// Thin typed wrappers around the Rust commands in src-tauri/src/commands.rs.
import { invoke } from "@tauri-apps/api/core";
import type { BootInfo, Lang, LanguagePref, QrMatrix, Settings, Snapshot, TransferSnapshot } from "./types";

export const api = {
  bootInfo: () => invoke<BootInfo>("boot_info"),
  snapshot: () => invoke<Snapshot>("get_snapshot"),
  rotateCode: () => invoke<void>("rotate_code"),
  qrMatrix: (text: string) => invoke<QrMatrix>("qr_matrix", { text }),
  normalizeCode: (input: string) => invoke<string | null>("normalize_code", { input }),

  sendFiles: (peerId: string, paths: string[]) => invoke<string>("send_files", { peerId, paths }),
  sendFilesToCode: (code: string, paths: string[]) => invoke<string>("send_files_to_code", { code, paths }),
  sendClipboard: (target: { peerId: string } | { code: string }) =>
    invoke<string>("send_clipboard", {
      peerId: "peerId" in target ? target.peerId : null,
      code: "code" in target ? target.code : null,
    }),

  respondToOffer: (offerId: string, accept: boolean) => invoke<void>("respond_to_offer", { offerId, accept }),
  pause: (id: string) => invoke<void>("pause_transfer", { id }),
  resume: (id: string) => invoke<void>("resume_transfer", { id }),
  cancel: (id: string) => invoke<void>("cancel_transfer", { id }),
  clearFinished: () => invoke<TransferSnapshot[]>("clear_finished_transfers"),

  openLinkWindow: (seconds: number) => invoke<void>("open_link_window", { seconds }),
  closeLinkWindow: () => invoke<void>("close_link_window"),
  linkWithCode: (code: string) => invoke<void>("link_with_code", { code }),
  untrust: (deviceId: string) => invoke<void>("untrust_device", { deviceId }),

  copyText: (text: string) => invoke<void>("copy_text", { text }),
  reveal: (path: string) => invoke<void>("reveal_path", { path }),

  setDeviceName: (name: string) => invoke<Settings>("set_device_name", { name }),
  setDownloadDir: (dir: string) => invoke<Settings>("set_download_dir", { dir }),
  setRelayUrl: (url: string | null) => invoke<Settings>("set_relay_url", { url }),
  setLanguage: (language: LanguagePref) => invoke<Lang>("set_language", { language }),
  setLaunchAtLogin: (enabled: boolean) => invoke<Settings>("set_launch_at_login", { enabled }),
};
