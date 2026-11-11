import type { CSSProperties } from "react";
import type { OutputState } from "./ipc";

export const clamp = (value: number, min: number, max: number): number =>
  Math.min(max, Math.max(min, value));

export function formatMs(ms: number): string {
  if (ms <= 0) return "Off";
  if (ms < 1000) return `${ms} ms`;
  return `${Number((ms / 1000).toFixed(2))} s`;
}

export const formatPercent = (fraction: number): string => `${Math.round(fraction * 100)}%`;

export const formatFps = (fps: number): string => `${fps.toFixed(1)} fps`;

export const OUTPUT_STATE_LABEL: Record<OutputState, string> = {
  connected: "Connected",
  connecting: "Connecting",
  unavailable: "Not available",
  error: "Error",
  disabled: "Turned off",
};

/** Typed helper for passing CSS custom properties through the `style` prop. */
export function cssVars(vars: Record<`--${string}`, string | number>): CSSProperties {
  return vars as CSSProperties;
}

export interface Rgb {
  r: number;
  g: number;
  b: number;
}

export function parseHex(hex: string): Rgb | null {
  const match = /^#?([0-9a-f]{6})$/i.exec(hex);
  const digits = match?.[1];
  if (!digits) return null;
  const value = Number.parseInt(digits, 16);
  return { r: (value >> 16) & 255, g: (value >> 8) & 255, b: value & 255 };
}

function toLinear(channel: number): number {
  const c = channel / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

/** Relative luminance (WCAG), 0..1. */
export function luminance(hex: string): number {
  const rgb = parseHex(hex);
  if (!rgb) return 0;
  return 0.2126 * toLinear(rgb.r) + 0.7152 * toLinear(rgb.g) + 0.0722 * toLinear(rgb.b);
}
