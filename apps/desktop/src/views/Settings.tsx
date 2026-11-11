import { useState } from "react";
import { ModeChoice, SmoothingControl } from "../components/ColorFeel";
import { SliderField } from "../components/controls";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Card, Page } from "../components/layout";
import { AutoUpdateToggle, StartMinimisedToggle, StartWithWindowsToggle } from "../components/StartupToggles";
import { UpdateChecker } from "../components/Updates";
import { useConfig } from "../hooks/useConfig";
import { SATURATION_MAX } from "../lib/defaults";
import { formatPercent } from "../lib/format";
import { patchConfig, resetConfig } from "../state/config";

export function Settings() {
  const config = useConfig();
  const [confirmingReset, setConfirmingReset] = useState(false);

  return (
    <Page title="Settings" description="Tune how the light behaves and how Glowdrift starts.">
      <Card title="Colour">
        <div className="stack">
          <div className="stack-tight">
            <div className="section-label">How to pick the colour</div>
            <ModeChoice />
          </div>
          <SmoothingControl />
          <SliderField
            label="Brightness"
            description="Scales the colour sent to your lights."
            value={config.brightness}
            min={0}
            max={1}
            step={0.01}
            valueText={formatPercent(config.brightness)}
            onChange={(brightness) => patchConfig({ brightness })}
          />
          <SliderField
            label="Saturation"
            description="Below 100% washes colours out; above 100% makes them richer."
            value={config.saturation}
            min={0}
            max={SATURATION_MAX}
            step={0.01}
            valueText={formatPercent(config.saturation)}
            onChange={(saturation) => patchConfig({ saturation })}
          />
        </div>
      </Card>

      <Card title="Startup">
        <StartWithWindowsToggle />
        <StartMinimisedToggle />
      </Card>

      <Card title="Updates">
        <AutoUpdateToggle />
        <UpdateChecker />
      </Card>

      <Card title="Setup">
        <div className="setting-row">
          <div className="setting-text">
            <div className="setting-title">Run setup again</div>
            <div className="setting-description">Walk through choosing a display, lights and feel once more.</div>
          </div>
          <button type="button" className="btn" onClick={() => patchConfig({ onboardingComplete: false })}>
            Run setup
          </button>
        </div>
        <div className="setting-row">
          <div className="setting-text">
            <div className="setting-title">Reset to defaults</div>
            <div className="setting-description">Restores every setting except Start with Windows.</div>
          </div>
          <button type="button" className="btn btn--danger-outline" onClick={() => setConfirmingReset(true)}>
            Reset
          </button>
        </div>
      </Card>

      <ConfirmDialog
        open={confirmingReset}
        title="Reset all settings?"
        message="Your display, region, colour and startup choices will go back to their defaults."
        confirmLabel="Reset settings"
        onCancel={() => setConfirmingReset(false)}
        onConfirm={() => {
          resetConfig();
          setConfirmingReset(false);
        }}
      />
    </Page>
  );
}
