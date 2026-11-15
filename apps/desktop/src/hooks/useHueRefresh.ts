import { useEffect } from "react";
import { refreshFromHue } from "../state/areaLights";

const POLL_MS = 5000;

/** Re-reads the Hue light positions from the bridge every few seconds while `active` and the window is visible. */
export function useHueRefresh(active: boolean): void {
  useEffect(() => {
    if (!active) return;
    const tick = () => {
      if (document.visibilityState === "visible") void refreshFromHue();
    };
    tick();
    const timer = window.setInterval(tick, POLL_MS);
    document.addEventListener("visibilitychange", tick);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [active]);
}
