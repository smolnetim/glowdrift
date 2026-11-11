import { type OutputStatus, listOutputs, onOutputsStatus } from "../lib/ipc";
import { createStore } from "../lib/store";

export const outputsStore = createStore<OutputStatus[]>([]);

export async function refreshOutputs(): Promise<void> {
  try {
    outputsStore.set(await listOutputs());
  } catch {
    // Keep the last known list; the next status event replaces it.
  }
}

export function startOutputsSync(): void {
  void refreshOutputs();
  void onOutputsStatus((list) => outputsStore.set(list)).catch(() => undefined);
}
