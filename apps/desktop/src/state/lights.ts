import { type LightStatus, installIcueSdk, listLights, onLightsStatus } from "../lib/ipc";
import { createStore } from "../lib/store";

export const lightsStore = createStore<LightStatus[]>([]);

export async function refreshLights(): Promise<void> {
  try {
    lightsStore.set(await listLights());
  } catch {
    // Keep the last known list; the next status event replaces it.
  }
}

/** Downloads Corsair's iCUE library and applies the returned light statuses. Throws a message. */
export async function downloadIcueLibrary(): Promise<void> {
  try {
    lightsStore.set(await installIcueSdk());
  } catch (error) {
    throw new Error(typeof error === "string" ? error : "The library could not be installed.");
  }
}

export function startLightsSync(): void {
  void refreshLights();
  void onLightsStatus((list) => lightsStore.set(list)).catch(() => undefined);
}
