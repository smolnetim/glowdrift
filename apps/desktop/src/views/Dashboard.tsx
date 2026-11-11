import { ColorChip } from "../components/LiveColor";
import { Icon } from "../components/Icon";
import { Banner, Card, Page } from "../components/layout";
import { StateBadge } from "../components/Outputs";
import { formatFps } from "../lib/format";
import { useStore } from "../lib/store";
import { configStore } from "../state/config";
import { engineStore, toggleEngine } from "../state/engine";
import { outputsStore } from "../state/outputs";

function EngineCard() {
  const { status, commandFailed } = useStore(engineStore);
  const config = useStore(configStore).config;
  return (
    <Card title="Engine">
      <div className="engine-row">
        <div>
          <div className={`engine-state engine-state--${status.running ? "on" : "off"}`}>
            <span className="badge-dot" aria-hidden="true" />
            {status.running ? "Running" : "Paused"}
          </div>
          <p className="muted-text">
            {status.running
              ? `Measured capture rate: ${formatFps(status.measuredFps)}${config ? ` of ${config.captureFps} fps` : ""}`
              : "Your lights keep their last colour while paused."}
          </p>
        </div>
        <button type="button" className="btn btn--primary" onClick={() => void toggleEngine()}>
          <Icon name={status.running ? "pause" : "play"} size={16} />
          {status.running ? "Pause" : "Resume"}
        </button>
      </div>
      {commandFailed && <p className="muted-text field-error">Could not change the engine state. Try again.</p>}
    </Card>
  );
}

function OutputSummary() {
  const outputs = useStore(outputsStore);
  return (
    <Card title="Lights">
      {outputs.length === 0 ? (
        <p className="muted-text">No lights detected yet. Open the Lights page to set one up.</p>
      ) : (
        <ul className="chip-list">
          {outputs.map((output) => (
            <li key={output.id} className="chip">
              <span>{output.name}</span>
              <StateBadge output={output} />
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

export function Dashboard() {
  const { status } = useStore(engineStore);
  return (
    <Page title="Dashboard" description="What Glowdrift sees on your screen, and the calm colour it sends to your lights.">
      {status.lastError && (
        <Banner tone="danger" title="Something went wrong">
          {status.lastError}
        </Banner>
      )}
      <section className="hero" aria-label="Live colour preview">
        <div className="hero-glow" aria-hidden="true" />
        <div className="hero-swatches">
          <ColorChip kind="smoothed" size="large" label="Smoothed colour sent to lights" />
          <ColorChip kind="raw" size="small" label="Raw colour from screen" />
        </div>
      </section>
      <div className="grid-2">
        <EngineCard />
        <OutputSummary />
      </div>
    </Page>
  );
}
