import type { AreaLightZone, NormRect } from "../lib/ipc";
import { clearHueZones, setHueZone } from "../state/config";
import { LiveSwatch } from "./RegionEditor";

interface LightZonesProps {
  lights: AreaLightZone[];
  zones: Record<string, NormRect>;
  hueEnabled: boolean;
  /** An entertainment area or extra light is chosen. */
  hasArea: boolean;
  selectedId: string;
  onSelect: (id: string) => void;
}

const originOf = (light: AreaLightZone, custom: boolean): string =>
  custom ? "Custom zone" : light.source === "extra" ? "Centred by default" : "At its Hue position";

/** One row per Hue light: select its box, see its live colour, reset its zone. */
export function LightZones({ lights, zones, hueEnabled, hasArea, selectedId, onSelect }: LightZonesProps) {
  if (!hueEnabled) return null;
  if (lights.length === 0) {
    return hasArea ? (
      <p className="muted-text">Your Hue lights appear on the preview once Glowdrift is connected to them.</p>
    ) : null;
  }

  const isCustom = (light: AreaLightZone) => light.id in zones;
  const anyCustom = lights.some(isCustom);

  return (
    <div className="stack-tight">
      <div className="section-header">
        <div className="section-label">Hue light zones</div>
        <button type="button" className="btn btn--subtle" onClick={clearHueZones} disabled={!anyCustom}>
          Reset to Hue positions
        </button>
      </div>
      <p className="muted-text">
        Each Hue light follows its own part of the whole display. Zones start at the light&apos;s position in your
        entertainment area; extra lights start in the middle. Boxes may overlap.
      </p>
      <ul className="zone-list">
        {lights.map((light) => (
          <li key={light.id} className="zone-row">
            <button
              type="button"
              className="zone-pick"
              aria-pressed={light.id === selectedId}
              onClick={() => onSelect(light.id)}
            >
              <LiveSwatch colorIndex={lights.indexOf(light)} />
              <span className="zone-name">{light.name}</span>
              <span className="muted-text">{originOf(light, isCustom(light))}</span>
            </button>
            <button
              type="button"
              className="btn btn--subtle"
              onClick={() => setHueZone(light.id, null)}
              disabled={!isCustom(light)}
              aria-label={`Reset zone of ${light.name}`}
            >
              Reset
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
