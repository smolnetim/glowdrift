import { useConfig } from "../hooks/useConfig";
import { SMOOTHING_PRESETS, SMOOTHING_SLIDER_MAX, sliderToSmoothing, smoothingToSlider } from "../lib/curve";
import { formatMs } from "../lib/format";
import type { ReduceMode } from "../lib/ipc";
import { patchConfig } from "../state/config";
import { ChoiceGroup, SliderField } from "./controls";

const MODE_OPTIONS = [
  {
    value: "average",
    title: "Average",
    description: "Blends every pixel in the region into one soft wash of colour.",
  },
  {
    value: "dominant",
    title: "Dominant",
    description: "Picks the most common colour, which stays vivid on busy scenes.",
  },
] satisfies { value: ReduceMode; title: string; description: string }[];

export function ModeChoice() {
  const config = useConfig();
  return (
    <ChoiceGroup
      legend="How to pick the colour"
      name="mode"
      value={config.mode}
      options={MODE_OPTIONS}
      onChange={(mode) => patchConfig({ mode })}
    />
  );
}

export function SmoothingControl() {
  const config = useConfig();
  return (
    <SliderField
      label="Smoothing"
      description="How slowly the light follows your screen. Higher values fade more gently."
      value={smoothingToSlider(config.smoothingMs)}
      min={0}
      max={SMOOTHING_SLIDER_MAX}
      step={1}
      valueText={formatMs(config.smoothingMs)}
      onChange={(position) => patchConfig({ smoothingMs: sliderToSmoothing(position) })}
    >
      <div className="preset-row" role="group" aria-label="Smoothing presets">
        {SMOOTHING_PRESETS.map((preset) => (
          <button
            key={preset.label}
            type="button"
            className="chip-button"
            aria-pressed={config.smoothingMs === preset.ms}
            onClick={() => patchConfig({ smoothingMs: preset.ms })}
          >
            {preset.label}
          </button>
        ))}
      </div>
    </SliderField>
  );
}
