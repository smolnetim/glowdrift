import { type EngineStatus, getEngineStatus, onColor, onEngineStatus, setEngineRunning } from "../lib/ipc";
import { pushLiveColor } from "../lib/liveColor";
import { createStore } from "../lib/store";

interface EngineState {
  status: EngineStatus;
  commandFailed: boolean;
}

export const engineStore = createStore<EngineState>({
  status: { running: false, measuredFps: 0, lastError: null, raw: "#000000", smoothed: "#000000" },
  commandFailed: false,
});

function apply(status: EngineStatus): void {
  engineStore.set({ status, commandFailed: false });
  pushLiveColor({ raw: status.raw, smoothed: status.smoothed });
}

export async function refreshEngine(): Promise<void> {
  try {
    apply(await getEngineStatus());
  } catch {
    // The next status event will bring the UI up to date.
  }
}

export async function toggleEngine(): Promise<void> {
  try {
    apply(await setEngineRunning(!engineStore.get().status.running));
  } catch {
    engineStore.set({ ...engineStore.get(), commandFailed: true });
  }
}

export function startEngineSync(): void {
  void refreshEngine();
  void onEngineStatus(apply).catch(() => undefined);
  void onColor(pushLiveColor).catch(() => undefined);
}
