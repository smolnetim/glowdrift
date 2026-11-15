import { useMonitorPreview } from "../hooks/useMonitorPreview";
import { DisplayPicker } from "./DisplayPicker";
import { IcueRegion } from "./IcueRegion";

/** Display picker plus the whole-region editor, as used by setup. */
export function ScreenRegionPicker() {
  const preview = useMonitorPreview();
  return (
    <div className="stack">
      <DisplayPicker preview={preview} />
      <IcueRegion preview={preview} />
    </div>
  );
}
