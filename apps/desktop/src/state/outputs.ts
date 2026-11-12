import { type OutputStatus, installIcueSdk, listOutputs, onOutputsStatus } from "../lib/ipc";
import { createStore } from "../lib/store";

export const outputsStore = createStore<OutputStatus[]>([]);

export async function refreshOutputs(): Promise<void> {
  try {
    outputsStore.set(await listOutputs());
  } catch {
    // Keep the last known list; the next status event replaces it.
  }
}

/** Downloads Corsair's iCUE library and applies the returned output statuses. Throws a message. */
export async function downloadIcueLibrary(): Promise<void> {
  try {
    outputsStore.set(await installIcueSdk());
  } catch (error) {
    throw new Error(typeof error === "string" ? error : "The library could not be installed.");
  }
}

export function startOutputsSync(): void {
  void refreshOutputs();
  void onOutputsStatus((list) => outputsStore.set(list)).catch(() => undefined);
}
