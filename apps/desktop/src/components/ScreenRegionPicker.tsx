import { useState } from "react";
import { useAreaLights } from "../hooks/useAreaLights";
import { useMonitors } from "../hooks/useMonitors";
import { useThumbnail } from "../hooks/useThumbnail";
import { useConfig } from "../hooks/useConfig";
import type { MonitorInfo, NormRect } from "../lib/ipc";
import { REGION_PRESETS, clampRegion, sameRegion } from "../lib/region";
import { patchConfig, setHueZone } from "../state/config";
import { ChoiceGroup } from "./controls";
import { Icon } from "./Icon";
import { LightZones } from "./LightZones";
import { PercentField } from "./PercentField";
import { type EditorBox, REGION_BOX_ID, RegionEditor } from "./RegionEditor";

const FALLBACK_ASPECT = 16 / 9;
/** Thumbnails smaller than this are placeholders, not a usable picture. */
const MIN_USABLE_THUMBNAIL = 16;

const describeMonitor = (monitor: MonitorInfo): string =>
  `${monitor.width} × ${monitor.height}${monitor.primary ? " · Primary" : ""}`;

function MonitorPicker({ monitors, selectedId }: { monitors: MonitorInfo[]; selectedId: string | null }) {
  return (
    <ChoiceGroup
      legend="Display to capture"
      name="monitor"
      value={selectedId}
      options={monitors.map((m) => ({ value: m.id, title: m.name, description: describeMonitor(m) }))}
      onChange={(id) => patchConfig({ monitorId: id })}
    />
  );
}

/**
 * Display picker plus the preview editor, shared by setup and the Capture page. With `withZones`, the
 * preview also carries one box per Hue light.
 */
export function ScreenRegionPicker({ withZones = false }: { withZones?: boolean }) {
  const config = useConfig();
  const areaLights = useAreaLights();
  const [selection, setSelection] = useState(REGION_BOX_ID);
  const { monitors, loading: monitorsLoading, failed: monitorsFailed, refresh: refreshMonitors } = useMonitors();
  const { thumbnail, loading, failed, refresh: refreshThumbnail } = useThumbnail(config.monitorId);

  const selected =
    monitors.find((m) => m.id === config.monitorId) ??
    (config.monitorId === null ? monitors.find((m) => m.primary) : undefined);

  const thumbnailUsable = thumbnail !== null && thumbnail.width >= MIN_USABLE_THUMBNAIL;
  const aspectRatio = selected
    ? selected.width / selected.height
    : thumbnail && thumbnailUsable
      ? thumbnail.width / thumbnail.height
      : FALLBACK_ASPECT;

  const setRegion = (region: NormRect) => patchConfig({ region });
  const { region } = config;
  const hueZones = config.lights.hue.zones;
  const zoneLights = withZones ? areaLights : [];

  const boxes: EditorBox[] = [
    { id: REGION_BOX_ID, label: "iCUE", rect: region },
    ...zoneLights.map((light, colorIndex) => ({
      id: light.id,
      label: light.name,
      rect: hueZones[light.id] ?? light.zone,
      colorIndex,
    })),
  ];
  const multi = boxes.length > 1;
  const selectedId = boxes.some((b) => b.id === selection) ? selection : REGION_BOX_ID;
  const changeBox = (id: string, rect: NormRect) => (id === REGION_BOX_ID ? setRegion(rect) : setHueZone(id, rect));

  const pixels = (fraction: number, total: number | undefined): string =>
    total ? ` (${Math.round(fraction * total)} px)` : "";

  return (
    <div className="stack">
      <div className="stack-tight">
        <div className="section-label">Display</div>
        {monitors.length > 0 ? (
          <MonitorPicker monitors={monitors} selectedId={selected?.id ?? null} />
        ) : (
          <p className="muted-text">
            {monitorsLoading
              ? "Looking for displays…"
              : monitorsFailed
                ? "Could not list your displays."
                : "No displays were found."}{" "}
            {!monitorsLoading && (
              <button type="button" className="link-button" onClick={refreshMonitors}>
                Try again
              </button>
            )}
          </p>
        )}
        {monitors.length > 0 && config.monitorId !== null && !selected && (
          <p className="muted-text">The display you chose is not connected. Pick another one.</p>
        )}
      </div>

      <div className="stack-tight">
        <div className="section-header">
          <div className="section-label">Region to watch</div>
          <button type="button" className="btn btn--subtle" onClick={refreshThumbnail} disabled={loading}>
            <Icon name="refresh" size={16} />
            Refresh preview
          </button>
        </div>
        <RegionEditor
          boxes={boxes}
          selectedId={selectedId}
          imageUrl={thumbnailUsable ? thumbnail.dataUrl : null}
          aspectRatio={aspectRatio}
          busy={loading}
          onSelect={setSelection}
          onChange={changeBox}
        />
        {failed && <p className="muted-text">Could not capture a preview of this display. You can still set the region.</p>}
        <p className="region-summary" aria-live="polite">
          {multi ? "iCUE watches" : "Watching"} {Math.round(region.width * 100)}%{pixels(region.width, selected?.width)} wide by{" "}
          {Math.round(region.height * 100)}%{pixels(region.height, selected?.height)} tall.
        </p>
      </div>

      {withZones && (
        <LightZones
          lights={zoneLights}
          zones={hueZones}
          hueEnabled={config.lights.hue.enabled}
          hasArea={config.lights.hue.areaId !== null || config.lights.hue.extraLights.length > 0}
          selectedId={selectedId}
          onSelect={setSelection}
        />
      )}

      {multi && <div className="section-label">iCUE region</div>}
      <div className="preset-row" role="group" aria-label="Region presets">
        {REGION_PRESETS.map((preset) => (
          <button
            key={preset.label}
            type="button"
            className="chip-button"
            aria-pressed={sameRegion(region, preset.region)}
            onClick={() => setRegion(preset.region)}
          >
            {preset.label}
          </button>
        ))}
      </div>

      <div className="percent-grid">
        <PercentField label="Left" value={region.x} onCommit={(x) => setRegion(clampRegion({ ...region, x }))} />
        <PercentField label="Top" value={region.y} onCommit={(y) => setRegion(clampRegion({ ...region, y }))} />
        <PercentField
          label="Width"
          value={region.width}
          onCommit={(width) => setRegion(clampRegion({ ...region, width: Math.min(width, 1 - region.x) }))}
        />
        <PercentField
          label="Height"
          value={region.height}
          onCommit={(height) => setRegion(clampRegion({ ...region, height: Math.min(height, 1 - region.y) }))}
        />
      </div>
    </div>
  );
}
