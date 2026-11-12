import { onUpdaterReady } from "../lib/ipc";
import { createStore } from "../lib/store";
import { type AvailableUpdate, checkForUpdate, restartApp } from "../lib/system";

export type UpdateState =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "upToDate" }
  | { phase: "available"; update: AvailableUpdate }
  | { phase: "downloading"; version: string; progress: number | null }
  | { phase: "ready"; version: string; error?: string }
  | { phase: "failed"; stage: "check" | "install"; message?: string };

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
  } catch (error) {
    updateStore.set({ phase: "failed", stage: "install", message: errorText(error) });
  }
}

/** Installs the staged update and restarts. On failure the update stays ready and the error is shown. */
export async function restartToUpdate(): Promise<void> {
  const state = updateStore.get();
  try {
    await restartApp();
  } catch (error) {
    const version = state.phase === "ready" ? state.version : "";
    updateStore.set({ phase: "ready", version, error: errorText(error) ?? "The update could not be installed." });
  }
}

/** Commands reject with a plain message string; anything else has no presentable text. */
function errorText(error: unknown): string | undefined {
  return typeof error === "string" && error.length > 0 ? error : undefined;
}

export function startUpdateSync(): void {
  void onUpdaterReady((e) => updateStore.set({ phase: "ready", version: e.version })).catch(() => undefined);
}
