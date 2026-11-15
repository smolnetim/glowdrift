import { type AreaLightZone, hueAreaLights } from "../lib/ipc";
import { createStore } from "../lib/store";

/** The Hue lights that have a screen zone, as last reported by the shell. */
export const areaLightsStore = createStore<AreaLightZone[]>([]);

let requested = 0;

/** Re-reads the lights and their effective zones; a slower, older answer never replaces a newer one. */
export async function refreshAreaLights(): Promise<void> {
  requested += 1;
  const mine = requested;
  try {
    const lights = await hueAreaLights();
    if (mine === requested) areaLightsStore.set(lights);
  } catch {
    // Keep the last known list; the next refresh replaces it.
  }
}
