import type { NormRect } from "../lib/ipc";
import { REGION_PRESETS, clampRegion, sameRegion } from "../lib/region";
import { PercentField } from "./PercentField";

interface RectPresetsProps {
  label: string;
  rect: NormRect;
  onChange: (rect: NormRect) => void;
}

/** The shared region presets (full screen, strips, ...) for one rectangle. */
export function RectPresets({ label, rect, onChange }: RectPresetsProps) {
  return (
    <div className="preset-row" role="group" aria-label={label}>
      {REGION_PRESETS.map((preset) => (
        <button
          key={preset.label}
          type="button"
          className="chip-button"
          aria-pressed={sameRegion(rect, preset.region)}
          onClick={() => onChange(preset.region)}
        >
          {preset.label}
        </button>
      ))}
    </div>
  );
}

/** Left, top, width and height of one rectangle as percentages. */
export function RectFields({ label, rect, onChange }: RectPresetsProps) {
  return (
    <div className="percent-grid" role="group" aria-label={label}>
      <PercentField label="Left" value={rect.x} onCommit={(x) => onChange(clampRegion({ ...rect, x }))} />
      <PercentField label="Top" value={rect.y} onCommit={(y) => onChange(clampRegion({ ...rect, y }))} />
      <PercentField
        label="Width"
        value={rect.width}
        onCommit={(width) => onChange(clampRegion({ ...rect, width: Math.min(width, 1 - rect.x) }))}
      />
      <PercentField
        label="Height"
        value={rect.height}
        onCommit={(height) => onChange(clampRegion({ ...rect, height: Math.min(height, 1 - rect.y) }))}
      />
    </div>
  );
}
