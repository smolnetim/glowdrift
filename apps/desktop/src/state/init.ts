import { flushConfig, loadConfig } from "./config";
import { refreshEngine, startEngineSync } from "./engine";
import { refreshOutputs, startOutputsSync } from "./outputs";
import { startUpdateSync } from "./updates";

/** Loads initial state and subscribes to backend events. Call once at startup. */
export function startBackendSync(): void {
  void loadConfig();
  startEngineSync();
  startOutputsSync();
  startUpdateSync();

  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      flushConfig();
    } else {
      void refreshEngine();
      void refreshOutputs();
    }
  });
}
