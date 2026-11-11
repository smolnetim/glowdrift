import { Page } from "../components/layout";
import { OutputsPanel } from "../components/Outputs";

export function Lights() {
  return (
    <Page title="Lights" description="Choose where Glowdrift sends its colour.">
      <OutputsPanel />
    </Page>
  );
}
