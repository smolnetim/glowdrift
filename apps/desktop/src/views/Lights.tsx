import { Page } from "../components/layout";
import { LightsPanel } from "../components/LightsPanel";

export function Lights() {
  return (
    <Page title="Lights" description="Choose where Glowdrift sends its colour.">
      <LightsPanel />
    </Page>
  );
}
