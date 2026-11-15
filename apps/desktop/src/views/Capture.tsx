import { SliderField } from "../components/controls";
import { DisplayPicker } from "../components/DisplayPicker";
import { HueZones } from "../components/HueZones";
import { IcueRegion } from "../components/IcueRegion";
import { Banner, Card, Page } from "../components/layout";
import { useConfig } from "../hooks/useConfig";
import { useMonitorPreview } from "../hooks/useMonitorPreview";
import { CAPTURE_FPS_MAX, CAPTURE_FPS_MIN } from "../lib/defaults";
import { patchConfig } from "../state/config";

export function Capture() {
  const config = useConfig();
  const preview = useMonitorPreview();
  const { icue, hue } = config.lights;
  return (
    <Page
      title="Capture"
      description="Choose which display Glowdrift watches, then which part of it each light follows."
    >
      <Card title="Display and sampling" description="These settings apply to every light.">
        <DisplayPicker preview={preview} />
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
      {icue.enabled && (
        <Card title="Corsair iCUE" description="All iCUE devices show one colour from this region of the display.">
          <IcueRegion preview={preview} subject="iCUE watches" regionLabel="iCUE capture region" />
        </Card>
      )}
      {hue.enabled && (
        <Card title="Philips Hue" description="Each Hue lamp gets its own colour from its own zone of the display.">
          <HueZones preview={preview} />
        </Card>
      )}
      {!icue.enabled && !hue.enabled && (
        <Banner tone="info" title="No lights are turned on">
          Turn on Corsair iCUE or Philips Hue on the Lights page to choose what each of them watches.
        </Banner>
      )}
    </Page>
  );
}
