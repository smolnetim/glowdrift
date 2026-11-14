import { flushConfig, loadConfig } from "./config";
import { refreshEngine, startEngineSync } from "./engine";
import { refreshLights, startLightsSync } from "./lights";
import { startUpdateSync } from "./updates";

/** Loads initial state and subscribes to backend events. Call once at startup. */
export function startBackendSync(): void {
  void loadConfig();
  startEngineSync();
  startLightsSync();
  startUpdateSync();

  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      flushConfig();
    } else {
      void refreshEngine();
      void refreshLights();
    }
  });
}
