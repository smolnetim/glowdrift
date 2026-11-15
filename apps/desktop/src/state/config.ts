import { DEFAULT_CONFIG } from "../lib/defaults";
import { type Config, type LightKind, type NormRect, getConfig, setConfig } from "../lib/ipc";
import { createStore } from "../lib/store";

interface ConfigState {
  config: Config | null;
  loadFailed: boolean;
  saveFailed: boolean;
}

export const configStore = createStore<ConfigState>({ config: null, loadFailed: false, saveFailed: false });

const SAVE_DELAY_MS = 150;

/** Bumped on every local edit so a stale save response never overwrites newer edits. */
let version = 0;
let timer: number | undefined;

export async function loadConfig(): Promise<void> {
  try {
    const config = await getConfig();
    configStore.set({ config, loadFailed: false, saveFailed: false });
  } catch {
    configStore.set({ ...configStore.get(), loadFailed: true });
  }
}

async function save(): Promise<void> {
  timer = undefined;
  const snapshot = configStore.get().config;
  if (!snapshot) return;
  const sent = version;
  try {
    const stored = await setConfig(snapshot);
    const current = configStore.get().config;
    configStore.set({
      config: sent === version ? stored : current,
      loadFailed: false,
      saveFailed: false,
    });
  } catch {
    configStore.set({ ...configStore.get(), saveFailed: true });
  }
}

/** Applies the patch immediately and persists it after a short debounce. */
export function patchConfig(patch: Partial<Config>): void {
  const state = configStore.get();
  if (!state.config) return;
  version += 1;
  configStore.set({ ...state, config: { ...state.config, ...patch } });
  window.clearTimeout(timer);
  timer = window.setTimeout(() => void save(), SAVE_DELAY_MS);
}

/** Turns one light on or off, keeping the rest of that light's settings (such as Hue's bridge and area). */
export function setLightEnabled(kind: LightKind, enabled: boolean): void {
  const config = configStore.get().config;
  if (!config) return;
  patchConfig({ lights: { ...config.lights, [kind]: { ...config.lights[kind], enabled } } });
}

function patchHue(change: (hue: Config["lights"]["hue"]) => Partial<Config["lights"]["hue"]>): void {
  const config = configStore.get().config;
  if (!config) return;
  const hue = config.lights.hue;
  patchConfig({ lights: { ...config.lights, hue: { ...hue, ...change(hue) } } });
}

/** Sets one Hue light's zone override (a fraction of the monitor), or removes it with `null`. */
export function setHueZone(lightId: string, rect: NormRect | null): void {
  patchHue(({ zones }) => {
    const { [lightId]: _removed, ...rest } = zones;
    return { zones: rect ? { ...rest, [lightId]: rect } : rest };
  });
}

/** Removes every Hue zone override. */
export function clearHueZones(): void {
  patchHue(() => ({ zones: {} }));
}

export function setHueExtraLights(extraLights: string[]): void {
  patchHue(() => ({ extraLights }));
}

/**
 * Takes the Hue bridge and area chosen by the shell into the local config and leaves every other
 * local edit, including ones still waiting to be saved, untouched.
 */
export function applyHueSelection(stored: Config): void {
  const state = configStore.get();
  if (!state.config) {
    configStore.set({ ...state, config: stored });
    return;
  }
  const { bridgeId, areaId } = stored.lights.hue;
  version += 1;
  configStore.set({
    ...state,
    config: {
      ...state.config,
      lights: { ...state.config.lights, hue: { ...state.config.lights.hue, bridgeId, areaId } },
    },
  });
}

export function flushConfig(): void {
  if (timer === undefined) return;
  window.clearTimeout(timer);
  void save();
}

/** Restores every setting except onboarding state, which stays complete, and the paired Hue bridge, area and extra lights. */
export function resetConfig(): void {
  const { bridgeId, areaId, extraLights } = configStore.get().config?.lights.hue ?? DEFAULT_CONFIG.lights.hue;
  patchConfig({
    ...DEFAULT_CONFIG,
    onboardingComplete: true,
    lights: { ...DEFAULT_CONFIG.lights, hue: { ...DEFAULT_CONFIG.lights.hue, bridgeId, areaId, extraLights } },
  });
}
