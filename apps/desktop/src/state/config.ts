import { DEFAULT_CONFIG } from "../lib/defaults";
import { type Config, getConfig, setConfig } from "../lib/ipc";
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

export function flushConfig(): void {
  if (timer === undefined) return;
  window.clearTimeout(timer);
  void save();
}

/** Restores every setting except onboarding state, which stays complete. */
export function resetConfig(): void {
  patchConfig({ ...DEFAULT_CONFIG, onboardingComplete: true });
}
