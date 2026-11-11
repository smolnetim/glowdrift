import { useEffect, useRef } from "react";
import type { ColorEvent } from "./ipc";
import { clamp, luminance } from "./format";

/**
 * Holds the latest engine colour and mirrors it to CSS custom properties on the
 * document root once per animation frame, so colour updates never re-render React.
 */
type Listener = (color: ColorEvent) => void;

const INK_LIGHT = "#ffffff";
const INK_DARK = "#111114";
/** Luminance at which black and white text have equal contrast. */
const INK_THRESHOLD = 0.179;

let current: ColorEvent = { raw: "#000000", smoothed: "#000000" };
let frame = 0;
const listeners = new Set<Listener>();

const inkFor = (hex: string): string => (luminance(hex) > INK_THRESHOLD ? INK_DARK : INK_LIGHT);

function flush(): void {
  frame = 0;
  const style = document.documentElement.style;
  const lum = luminance(current.smoothed);
  style.setProperty("--live", current.smoothed);
  style.setProperty("--live-raw", current.raw);
  style.setProperty("--live-ink", inkFor(current.smoothed));
  style.setProperty("--live-raw-ink", inkFor(current.raw));
  // Near-black or near-white colours fall back to the theme accent for UI chrome.
  style.setProperty("--live-strength", clamp(Math.min(lum / 0.15, (1 - lum) / 0.15), 0, 1).toFixed(3));
  for (const listener of listeners) listener(current);
}

export function pushLiveColor(next: ColorEvent): void {
  if (next.raw === current.raw && next.smoothed === current.smoothed) return;
  current = next;
  if (!frame) frame = requestAnimationFrame(flush);
}

export const getLiveColor = (): ColorEvent => current;

/** Calls the listener immediately and again on every colour change. */
export function subscribeLiveColor(listener: Listener): () => void {
  listeners.add(listener);
  listener(current);
  return () => {
    listeners.delete(listener);
  };
}

/** Writes the live hex string into the returned ref's element without re-rendering. */
export function useLiveHexRef(kind: keyof ColorEvent) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(
    () =>
      subscribeLiveColor((color) => {
        if (ref.current) ref.current.textContent = color[kind];
      }),
    [kind],
  );
  return ref;
}
