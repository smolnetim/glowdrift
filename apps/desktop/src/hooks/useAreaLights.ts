import { useEffect } from "react";
import { useStore } from "../lib/store";
import { areaLightsStore, refreshAreaLights } from "../state/areaLights";
import { configStore } from "../state/config";
import { lightsStore } from "../state/lights";

/** Lets the shell's saved zone overrides land before the effective zones are read back. */
const AFTER_SAVE_MS = 400;

/**
 * Hue lights with their effective zones. Refreshes when anything that changes them does: the Hue
 * connection, area, extra lights, which lights have an override, or the Hue light's status.
 */
export function useAreaLights() {
  const hue = useStore(configStore).config?.lights.hue;
  const status = useStore(lightsStore).find((l) => l.kind === "hue");
  const lights = useStore(areaLightsStore);

  const key = [
    hue?.enabled,
    hue?.bridgeId,
    hue?.areaId,
    hue?.extraLights.join(","),
    Object.keys(hue?.zones ?? {}).sort().join(","),
    status?.state,
    status?.code,
    status?.deviceCount,
  ].join("|");

  useEffect(() => {
    const timer = window.setTimeout(() => void refreshAreaLights(), AFTER_SAVE_MS);
    return () => window.clearTimeout(timer);
  }, [key]);

  useEffect(() => {
    void refreshAreaLights();
  }, []);

  return hue?.enabled ? lights : [];
}
