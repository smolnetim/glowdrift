import { onUpdaterReady } from "../lib/ipc";
import { createStore } from "../lib/store";
import { type AvailableUpdate, checkForUpdate } from "../lib/system";

export type UpdateState =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "upToDate" }
  | { phase: "available"; update: AvailableUpdate }
  | { phase: "downloading"; version: string; progress: number | null }
  | { phase: "ready"; version: string }
  | { phase: "failed"; stage: "check" | "install" };

export const updateStore = createStore<UpdateState>({ phase: "idle" });

export async function checkNow(): Promise<void> {
  updateStore.set({ phase: "checking" });
  try {
    const update = await checkForUpdate();
    updateStore.set(update ? { phase: "available", update } : { phase: "upToDate" });
  } catch {
    updateStore.set({ phase: "failed", stage: "check" });
  }
}

export async function installUpdate(update: AvailableUpdate): Promise<void> {
  updateStore.set({ phase: "downloading", version: update.version, progress: null });
  try {
    await update.install((progress) =>
      updateStore.set({ phase: "downloading", version: update.version, progress }),
    );
    updateStore.set({ phase: "ready", version: update.version });
  } catch {
    updateStore.set({ phase: "failed", stage: "install" });
  }
}

export function startUpdateSync(): void {
  void onUpdaterReady((e) => updateStore.set({ phase: "ready", version: e.version })).catch(() => undefined);
}
