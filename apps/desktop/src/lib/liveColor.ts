import { useEffect, useRef } from "react";
import type { ColorEvent, Rgb } from "./ipc";
import { clamp, luminance } from "./format";

/**
 * Holds the latest engine colour and mirrors it to CSS custom properties on the
 * document root once per animation frame, so colour updates never re-render React.
 */
type Listener = (color: ColorEvent) => void;

/** The hex colours of the live event. */
export type LiveColorKind = "target" | "smoothed";

const INK_LIGHT = "#ffffff";
const INK_DARK = "#111114";
/** Luminance at which black and white text have equal contrast. */
const INK_THRESHOLD = 0.179;

let current: ColorEvent = { target: "#000000", smoothed: "#000000", zones: [] };
let frame = 0;
const listeners = new Set<Listener>();

const inkFor = (hex: string): string => (luminance(hex) > INK_THRESHOLD ? INK_DARK : INK_LIGHT);

function flush(): void {
  frame = 0;
  const style = document.documentElement.style;
  const lum = luminance(current.smoothed);
  style.setProperty("--live", current.smoothed);
  style.setProperty("--live-target", current.target);
  style.setProperty("--live-ink", inkFor(current.smoothed));
  style.setProperty("--live-target-ink", inkFor(current.target));
  // Near-black or near-white colours fall back to the theme accent for UI chrome.
  style.setProperty("--live-strength", clamp(Math.min(lum / 0.15, (1 - lum) / 0.15), 0, 1).toFixed(3));
  for (const listener of listeners) listener(current);
}

const sameZones = (a: readonly Rgb[], b: readonly Rgb[]): boolean =>
  a === b || (a.length === b.length && a.every((c, i) => c.r === b[i]?.r && c.g === b[i]?.g && c.b === b[i]?.b));

/** An event without `zones` (such as an engine status) leaves the last known zone colours in place. */
export function pushLiveColor(next: ColorEvent): void {
  const zones = next.zones ?? current.zones ?? [];
  if (next.target === current.target && next.smoothed === current.smoothed && sameZones(zones, current.zones ?? [])) {
    return;
  }
  current = { target: next.target, smoothed: next.smoothed, zones };
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
export function useLiveHexRef(kind: LiveColorKind) {
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

/** CSS colour for a live zone swatch. */
export const rgbCss = (c: Rgb): string => `rgb(${c.r} ${c.g} ${c.b})`;
