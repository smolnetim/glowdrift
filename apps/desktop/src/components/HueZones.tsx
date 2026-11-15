import { useState } from "react";
import { useAreaLights } from "../hooks/useAreaLights";
import { useConfig } from "../hooks/useConfig";
import { useHueRefresh } from "../hooks/useHueRefresh";
import type { MonitorPreview } from "../hooks/useMonitorPreview";
import type { AreaLightZone, NormRect } from "../lib/ipc";
import { sameRegion } from "../lib/region";
import { useStore } from "../lib/store";
import { hueSyncStore, movedLightsStore, refreshFromHue } from "../state/areaLights";
import { resetHueZones, setHueZone } from "../state/config";
import { lightsStore } from "../state/lights";
import { Icon } from "./Icon";
import { RectFields, RectPresets } from "./RectFields";
import { type EditorBox, LiveSwatch, RegionEditor } from "./RegionEditor";

/** What the Hue section says while there are no lights to place. */
function emptyMessage(code: string | null | undefined): string {
  if (code === "notPaired") return "Pair your Hue bridge on the Lights page to place its lights on the screen.";
  if (code === "noArea") return "Choose an entertainment area or add a light on the Lights page.";
  return "Your Hue lights appear here once Glowdrift is connected to them.";
}

const originOf = (light: AreaLightZone, pinned: boolean): string =>
  pinned ? "Custom zone" : light.source === "extra" ? "Centred by default" : "At its Hue position";

function SyncStatus() {
  const { busy, error, checkedAt } = useStore(hueSyncStore);
  const text = busy
    ? "Checking the bridge…"
    : error
      ? error
      : checkedAt
        ? `Positions checked at ${new Date(checkedAt).toLocaleTimeString()}`
        : "";
  return (
    <span className="muted-text" role="status">
      {text}
    </span>
  );
}

/** Hue lights on the display: a preview with one box per lamp, a lamp list, refresh and reset. */
export function HueZones({ preview }: { preview: MonitorPreview }) {
  const config = useConfig();
  const lights = useAreaLights();
  const moved = useStore(movedLightsStore);
  const { busy } = useStore(hueSyncStore);
  const hueStatus = useStore(lightsStore).find((l) => l.kind === "hue");
  const [selection, setSelection] = useState("");
  const { zones } = config.lights.hue;
  useHueRefresh(config.lights.hue.bridgeId !== null);

  const toolbar = (
    <div className="zone-toolbar">
      <button type="button" className="btn btn--subtle" onClick={() => void refreshFromHue(true)} disabled={busy}>
        <Icon name="refresh" size={16} />
        Refresh from Hue
      </button>
      <SyncStatus />
    </div>
  );

  if (lights.length === 0) {
    return (
      <div className="stack-tight">
        <p className="muted-text">{emptyMessage(hueStatus?.code)}</p>
        {config.lights.hue.bridgeId !== null && toolbar}
      </div>
    );
  }

  const rectOf = (light: AreaLightZone): NormRect => zones[light.id] ?? light.zone;
  /** An override that no longer matches the light's Hue position. */
  const isPinned = (light: AreaLightZone): boolean => {
    const override = zones[light.id];
    return override !== undefined && !sameRegion(override, light.defaultZone);
  };
  const pinnedLights = lights.filter(isPinned);

  const selected = lights.find((l) => l.id === selection) ?? lights[0];
  const selectedId = selected?.id ?? "";
  const boxes: EditorBox[] = lights.map((light, colorIndex) => ({
    id: light.id,
    label: light.name,
    rect: rectOf(light),
    colorIndex,
  }));

  return (
    <div className="stack">
      <div className="stack-tight">
        <p className="muted-text">
          A lamp you have not moved follows its position in the Hue app and updates by itself. Moving its box, choosing a
          preset or pressing Reset pins the zone. Boxes may overlap.
        </p>
        <div className="section-header">
          {toolbar}
          <button
            type="button"
            className="btn btn--subtle"
            onClick={() => resetHueZones(pinnedLights)}
            disabled={pinnedLights.length === 0}
          >
            Reset all
          </button>
        </div>
      </div>

      <div className="stack-tight">
        <div className="section-header">
          <div className="section-label">Light zones</div>
          <button type="button" className="btn btn--subtle" onClick={preview.refreshPreview} disabled={preview.loading}>
            <Icon name="refresh" size={16} />
            Refresh preview
          </button>
        </div>
        <RegionEditor
          boxes={boxes}
          selectedId={selectedId}
          imageUrl={preview.imageUrl}
          aspectRatio={preview.aspectRatio}
          busy={preview.loading}
          onSelect={setSelection}
          onChange={(id, rect) => setHueZone(id, rect)}
        />
        {preview.failed && <p className="muted-text">Could not capture a preview of this display. You can still place the zones.</p>}
      </div>

      <ul className="zone-list" aria-label="Hue lights">
        {lights.map((light, index) => (
          <li key={light.id} className="zone-row">
            <button
              type="button"
              className="zone-pick"
              aria-pressed={light.id === selectedId}
              onClick={() => setSelection(light.id)}
            >
              <LiveSwatch colorIndex={index} />
              <span className="zone-text">
                <span className="zone-name">{light.name}</span>
                <span className="muted-text"> {originOf(light, isPinned(light))}</span>
                {moved.has(light.id) && isPinned(light) && (
                  <span className="zone-note">Its Hue position changed since you set this zone.</span>
                )}
              </span>
            </button>
            <button
              type="button"
              className="btn btn--subtle"
              onClick={() => resetHueZones([light])}
              disabled={!isPinned(light)}
              aria-label={`Reset zone of ${light.name} to its Hue position`}
            >
              Reset
            </button>
          </li>
        ))}
      </ul>

      {selected && (
        <div className="stack-tight">
          <div className="section-label">Zone of {selected.name}</div>
          <RectPresets
            label={`Zone presets for ${selected.name}`}
            rect={rectOf(selected)}
            onChange={(rect) => setHueZone(selected.id, rect)}
          />
          <RectFields
            label={`Position and size of ${selected.name}`}
            rect={rectOf(selected)}
            onChange={(rect) => setHueZone(selected.id, rect)}
          />
        </div>
      )}
    </div>
  );
}
