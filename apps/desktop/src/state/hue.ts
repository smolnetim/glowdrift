import { type HueBridge, getConfig, hueBridge, hueForget, huePair, hueSelectArea } from "../lib/ipc";
import { createStore } from "../lib/store";
import { applyHueSelection, setLightEnabled } from "./config";

/** How long the shell waits for the link button, in seconds. */
export const PAIR_WINDOW_S = 30;

export type PairingState =
  | { phase: "idle" }
  | { phase: "waiting"; address: string; startedAt: number }
  | { phase: "failed"; message: string };

interface HueState {
  /** undefined until first loaded; null when not paired. */
  bridge: HueBridge | null | undefined;
  pairing: PairingState;
}

/** Lives outside the components so a pairing in progress survives leaving the Lights page. */
export const hueStore = createStore<HueState>({ bridge: undefined, pairing: { phase: "idle" } });

export function errorMessage(error: unknown, fallback: string): string {
  if (typeof error === "string" && error) return error;
  if (error instanceof Error && error.message) return error.message;
  return fallback;
}

export async function refreshHueBridge(): Promise<void> {
  try {
    hueStore.set({ ...hueStore.get(), bridge: await hueBridge() });
  } catch {
    const state = hueStore.get();
    if (state.bridge === undefined) hueStore.set({ ...state, bridge: null });
  }
}

/** Pairs with the bridge at `address`; resolves true when paired. The outcome is also in the store. */
export async function startPairing(address: string): Promise<boolean> {
  if (hueStore.get().pairing.phase === "waiting") return false;
  hueStore.set({ ...hueStore.get(), pairing: { phase: "waiting", address, startedAt: Date.now() } });
  try {
    const bridge = await huePair(address);
    hueStore.set({ bridge, pairing: { phase: "idle" } });
    try {
      applyHueSelection(await getConfig());
    } catch {
      // The next config load picks up the stored bridge.
    }
    setLightEnabled("hue", true);
    return true;
  } catch (error) {
    hueStore.set({
      ...hueStore.get(),
      pairing: { phase: "failed", message: errorMessage(error, "Pairing did not work. Try again.") },
    });
    return false;
  }
}

/** Throws a message when the shell refuses. */
export async function chooseArea(areaId: string): Promise<void> {
  try {
    applyHueSelection(await hueSelectArea(areaId));
  } catch (error) {
    throw new Error(errorMessage(error, "The area could not be saved."));
  }
}

/** Deletes the stored pairing. Throws a message when the shell refuses. */
export async function forgetBridge(): Promise<void> {
  try {
    applyHueSelection(await hueForget());
    hueStore.set({ bridge: null, pairing: { phase: "idle" } });
  } catch (error) {
    throw new Error(errorMessage(error, "The bridge could not be forgotten."));
  }
}
