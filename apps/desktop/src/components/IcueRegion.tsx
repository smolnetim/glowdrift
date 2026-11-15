import { useConfig } from "../hooks/useConfig";
import type { MonitorPreview } from "../hooks/useMonitorPreview";
import { patchConfig } from "../state/config";
import { Icon } from "./Icon";
import { RectFields, RectPresets } from "./RectFields";
import { REGION_BOX_ID, RegionEditor } from "./RegionEditor";

const pixels = (fraction: number, total: number | undefined): string =>
  total ? ` (${Math.round(fraction * total)} px)` : "";

interface IcueRegionProps {
  preview: MonitorPreview;
  /** What the region drives, as the subject of the size summary. */
  subject?: string;
  regionLabel?: string;
}

/** The whole-region editor: preview with one draggable frame, presets and numeric fields. */
export function IcueRegion({ preview, subject = "Watching", regionLabel }: IcueRegionProps) {
  const { region } = useConfig();
  const { selected, aspectRatio, imageUrl, loading, failed, refreshPreview } = preview;
  const setRegion = (next: typeof region) => patchConfig({ region: next });

  return (
    <div className="stack">
      <div className="stack-tight">
        <div className="section-header">
          <div className="section-label">Region to watch</div>
          <button type="button" className="btn btn--subtle" onClick={refreshPreview} disabled={loading}>
            <Icon name="refresh" size={16} />
            Refresh preview
          </button>
        </div>
        <RegionEditor
          boxes={[{ id: REGION_BOX_ID, label: "Region", rect: region }]}
          selectedId={REGION_BOX_ID}
          imageUrl={imageUrl}
          aspectRatio={aspectRatio}
          busy={loading}
          regionLabel={regionLabel}
          onSelect={() => undefined}
          onChange={(_id, rect) => setRegion(rect)}
        />
        {failed && <p className="muted-text">Could not capture a preview of this display. You can still set the region.</p>}
        <p className="region-summary" aria-live="polite">
          {subject} {Math.round(region.width * 100)}%{pixels(region.width, selected?.width)} wide by{" "}
          {Math.round(region.height * 100)}%{pixels(region.height, selected?.height)} tall.
        </p>
      </div>
      <RectPresets label="Region presets" rect={region} onChange={setRegion} />
      <RectFields label="Region position and size" rect={region} onChange={setRegion} />
    </div>
  );
}
