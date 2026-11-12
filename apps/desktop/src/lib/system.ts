// Wrappers over the official Tauri plugins (autostart, updater, process).
// Outside Tauri they are served by ./ipc.mock.

import { isTauri } from "@tauri-apps/api/core";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { relaunch } from "@tauri-apps/plugin-process";
import { installPendingUpdate, stageUpdate } from "./ipc";
import { check } from "@tauri-apps/plugin-updater";

export interface AvailableUpdate {
  version: string;
  /** Downloads and installs; `onProgress` receives 0..1, or null when the size is unknown. */
  install: (onProgress: (fraction: number | null) => void) => Promise<void>;
}

const loadMock = () => import("./ipc.mock");

export const isAutostartEnabled = (): Promise<boolean> =>
  isTauri() ? isEnabled() : loadMock().then((m) => m.isAutostartEnabled());

export const setAutostart = (enabled: boolean): Promise<void> => {
  if (!isTauri()) return loadMock().then((m) => m.setAutostart(enabled));
  return enabled ? enable() : disable();
};

export async function checkForUpdate(): Promise<AvailableUpdate | null> {
  if (!isTauri()) return (await loadMock()).checkForUpdate();
  const update = await check();
  if (!update) return null;
  return {
    version: update.version,
    // The shell downloads and stages the update so it can stop the lights before the installer runs.
    async install(onProgress) {
      onProgress(null);
      if ((await stageUpdate()) === null) throw new Error("no update is available");
    },
  };
}

export const restartApp = (): Promise<void> => {
  if (!isTauri()) {
    window.location.reload();
    return Promise.resolve();
  }
  return installPendingUpdate().then((installed) => (installed ? undefined : relaunch()));
};
