import { SMOOTHING_MS_MAX } from "./defaults";
import { clamp } from "./format";

/** Number of positions on the smoothing slider. */
export const SMOOTHING_SLIDER_STEPS = 1000;
const EXPONENT = 2.5;

export function smoothingToSlider(ms: number): number {
  const fraction = clamp(ms / SMOOTHING_MS_MAX, 0, 1);
  return Math.round(fraction ** (1 / EXPONENT) * SMOOTHING_SLIDER_STEPS);
}

/** Maps a slider position to milliseconds, finer near zero and coarser near the top. */
export function sliderToSmoothing(position: number): number {
  const raw = SMOOTHING_MS_MAX * (clamp(position, 0, SMOOTHING_SLIDER_STEPS) / SMOOTHING_SLIDER_STEPS) ** EXPONENT;
  const step = raw < 100 ? 5 : raw < 1000 ? 25 : 50;
  return Math.min(SMOOTHING_MS_MAX, Math.round(raw / step) * step);
}

export const SMOOTHING_PRESETS = [
  { label: "Snappy", ms: 150 },
  { label: "Balanced", ms: 500 },
  { label: "Calm", ms: 1500 },
  { label: "Glacial", ms: 4000 },
] as const;
