import { SliderField } from "../components/controls";
import { Card, Page } from "../components/layout";
import { ScreenRegionPicker } from "../components/ScreenRegionPicker";
import { useConfig } from "../hooks/useConfig";
import { CAPTURE_FPS_MAX, CAPTURE_FPS_MIN } from "../lib/defaults";
import { patchConfig } from "../state/config";

export function Capture() {
  const config = useConfig();
  return (
    <Page title="Capture" description="Choose which part of which display Glowdrift watches.">
      <Card>
        <ScreenRegionPicker />
      </Card>
      <Card>
        <SliderField
          label="Capture rate"
          description="How often the screen is sampled. Higher rates react sooner but use more CPU and GPU. Colour fades stay smooth at any rate."
          value={config.captureFps}
          min={CAPTURE_FPS_MIN}
          max={CAPTURE_FPS_MAX}
          valueText={`${config.captureFps} fps`}
          onChange={(captureFps) => patchConfig({ captureFps })}
        />
      </Card>
    </Page>
  );
}
