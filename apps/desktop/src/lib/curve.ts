import { SMOOTHING_MS_MAX } from "./defaults";

function buildSmoothingSteps(): readonly number[] {
  const steps = [0];
  for (let ms = 5; ms < 100; ms += 5) steps.push(ms);
  for (let ms = 100; ms < 1000; ms += 25) steps.push(ms);
  for (let ms = 1000; ms <= SMOOTHING_MS_MAX; ms += 50) steps.push(ms);
  return steps;
}

/** Allowed smoothing values in ms: finer near zero, coarser near the top. The slider position is an index into this list. */
export const SMOOTHING_STEPS: readonly number[] = buildSmoothingSteps();

/** Highest slider position. */
export const SMOOTHING_SLIDER_MAX = SMOOTHING_STEPS.length - 1;

/** The slider position whose value is closest to `ms`. */
export function smoothingToSlider(ms: number): number {
  let best = 0;
  for (let i = 1; i < SMOOTHING_STEPS.length; i += 1) {
    const step = SMOOTHING_STEPS[i] as number;
    if (Math.abs(step - ms) < Math.abs((SMOOTHING_STEPS[best] as number) - ms)) best = i;
  }
  return best;
}

/** Maps a slider position (index) to milliseconds. */
export function sliderToSmoothing(position: number): number {
  const index = Math.min(SMOOTHING_SLIDER_MAX, Math.max(0, Math.round(position)));
  return SMOOTHING_STEPS[index] as number;
}

export const SMOOTHING_PRESETS = [
  { label: "Snappy", ms: 150 },
  { label: "Balanced", ms: 500 },
  { label: "Calm", ms: 1500 },
  { label: "Glacial", ms: 4000 },
] as const;
