import { type AreaLightZone, hueAreaLights, hueRefreshAreaLights } from "../lib/ipc";
import { sameRegion } from "../lib/region";
import { createStore } from "../lib/store";
import { configStore } from "./config";

/** The Hue lights that have a screen zone, as last reported by the shell. */
export const areaLightsStore = createStore<AreaLightZone[]>([]);

/** Ids of lights whose Hue position changed while their zone was pinned by an override. */
export const movedLightsStore = createStore<ReadonlySet<string>>(new Set());

export interface HueSyncState {
  /** A refresh the user asked for is running. */
  busy: boolean;
  /** Message of the last failed refresh; null after a successful one. */
  error: string | null;
  /** Time of the last successful refresh. */
  checkedAt: number | null;
}

export const hueSyncStore = createStore<HueSyncState>({ busy: false, error: null, checkedAt: null });

/** Counts cached reads; only the newest one is applied. */
let requested = 0;
/** Counts bridge refreshes; a cached read that started before one finished is out of date. */
let refreshed = 0;
let inFlight: Promise<void> | null = null;

/** Notes lights whose default zone moved under an override; a light without a differing override follows Hue. */
function trackMoved(lights: AreaLightZone[]): void {
  const zones = configStore.get().config?.lights.hue.zones ?? {};
  const before = new Map(areaLightsStore.get().map((light) => [light.id, light.defaultZone]));
  const previous = movedLightsStore.get();
  const next = new Set<string>();
  for (const light of lights) {
    const override = zones[light.id];
    if (!override || sameRegion(override, light.defaultZone)) continue;
    const earlier = before.get(light.id);
    if (previous.has(light.id) || (earlier && !sameRegion(earlier, light.defaultZone))) next.add(light.id);
  }
  if (next.size !== previous.size || [...next].some((id) => !previous.has(id))) movedLightsStore.set(next);
}

function apply(lights: AreaLightZone[]): void {
  trackMoved(lights);
  areaLightsStore.set(lights);
}

/** Re-reads the lights and their effective zones; a slower, older answer never replaces a newer one. */
export async function refreshAreaLights(): Promise<void> {
  requested += 1;
  const mine = requested;
  const generation = refreshed;
  try {
    const lights = await hueAreaLights();
    if (mine === requested && generation === refreshed) apply(lights);
  } catch {
    // Keep the last known list; the next refresh replaces it.
  }
}

/** Asks the bridge for the current light positions. A call while one is running joins it. */
export function refreshFromHue(manual = false): Promise<void> {
  if (manual) hueSyncStore.set({ ...hueSyncStore.get(), busy: true });
  if (inFlight) return inFlight;
  inFlight = hueRefreshAreaLights()
    .then((lights) => {
      refreshed += 1;
      apply(lights);
      hueSyncStore.set({ busy: false, error: null, checkedAt: Date.now() });
    })
    .catch((error: unknown) => {
      const message = typeof error === "string" ? error : "Could not reach the Hue bridge.";
      hueSyncStore.set({ ...hueSyncStore.get(), busy: false, error: message });
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}
