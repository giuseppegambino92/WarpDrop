// User-level actions shared by cards, the code bar, keyboard and drag & drop.
import { useCallback, useMemo, useRef } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Menu } from "@tauri-apps/api/menu";
import { api } from "./api";
import { codeTarget, type CodeEntry } from "./format";
import { deviceName, useStore, type DeviceModel } from "./store";
import { localizeCmdError, useI18n } from "../i18n";

async function pick(folder: boolean, title: string): Promise<string[]> {
  const r = await open({ multiple: true, directory: folder, recursive: folder, title });
  if (!r) return [];
  return Array.isArray(r) ? r : [r];
}

export function useActions() {
  const { state, dispatch, toast } = useStore();
  const { t } = useI18n();
  const stateRef = useRef(state);
  stateRef.current = state;
  const lastClipboardSend = useRef(0);

  const fail = useCallback(
    (e: unknown, peerName?: string | null) => toast("error", localizeCmdError(t, e, peerName)),
    [t, toast],
  );

  const sendToPeer = useCallback(
    async (peerId: string, paths: string[]) => {
      if (paths.length === 0) return;
      dispatch({ type: "select", peerId });
      try {
        await api.sendFiles(peerId, paths);
      } catch (e) {
        fail(e, deviceName(stateRef.current, peerId));
      }
    },
    [dispatch, fail],
  );

  /** Sends staged items if any, otherwise opens the picker. */
  const activateDevice = useCallback(
    async (peerId: string, folder = false) => {
      dispatch({ type: "select", peerId });
      const staged = stateRef.current.staged;
      if (staged.length > 0) {
        dispatch({ type: "clearStaged" });
        return sendToPeer(peerId, staged);
      }
      const paths = await pick(folder, folder ? t("send_folder") : t("send_files"));
      return sendToPeer(peerId, paths);
    },
    [dispatch, sendToPeer, t],
  );

  const sendToCode = useCallback(
    async (entry: CodeEntry): Promise<boolean> => {
      if (entry.digits.length !== 6) return false;
      let paths = stateRef.current.staged;
      const fromStage = paths.length > 0;
      if (!fromStage) paths = await pick(false, t("send_files"));
      if (paths.length === 0) return false;
      try {
        await api.sendFilesToCode(codeTarget(entry), paths);
        if (fromStage) dispatch({ type: "clearStaged" });
        return true;
      } catch (e) {
        fail(e);
        return false;
      }
    },
    [dispatch, fail, t],
  );

  const sendClipboard = useCallback(
    async (peerId: string | null) => {
      // ⌘V can surface as both keydown and paste; send once.
      const now = Date.now();
      if (now - lastClipboardSend.current < 400) return;
      lastClipboardSend.current = now;
      if (!peerId) {
        toast("info", t("error_pick_device"));
        return;
      }
      dispatch({ type: "select", peerId });
      try {
        await api.sendClipboard({ peerId });
      } catch (e) {
        fail(e, deviceName(stateRef.current, peerId));
      }
    },
    [dispatch, fail, t, toast],
  );

  const removeDevice = useCallback(
    async (deviceId: string) => {
      try {
        await api.untrust(deviceId);
        if (stateRef.current.selectedPeerId === deviceId) dispatch({ type: "select", peerId: null });
      } catch (e) {
        fail(e);
      }
    },
    [dispatch, fail],
  );

  const showDeviceMenu = useCallback(
    async (device: DeviceModel) => {
      dispatch({ type: "select", peerId: device.deviceId });
      const menu = await Menu.new({
        items: [
          { id: "files", text: t("send_files"), action: () => void activateDevice(device.deviceId, false) },
          { id: "folder", text: t("send_folder"), action: () => void activateDevice(device.deviceId, true) },
          { id: "clipboard", text: t("send_clipboard"), action: () => void sendClipboard(device.deviceId) },
          ...(device.trusted
            ? [
                { item: "Separator" as const },
                { id: "remove", text: t("remove_device"), action: () => void removeDevice(device.deviceId) },
              ]
            : []),
        ],
      });
      await menu.popup();
    },
    [activateDevice, dispatch, removeDevice, sendClipboard, t],
  );

  return useMemo(
    () => ({ sendToPeer, activateDevice, sendToCode, sendClipboard, removeDevice, showDeviceMenu, fail }),
    [sendToPeer, activateDevice, sendToCode, sendClipboard, removeDevice, showDeviceMenu, fail],
  );
}
