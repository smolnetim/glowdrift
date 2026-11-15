import { Icon } from "../components/Icon";
import { LightTiles } from "../components/LightTiles";
import { Banner, Card, Page } from "../components/layout";
import { formatFps } from "../lib/format";
import { useStore } from "../lib/store";
import { configStore } from "../state/config";
import { engineStore, toggleEngine } from "../state/engine";

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
              : "Paused: your lights are released and return to how they were."}
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

export function Dashboard() {
  const { status } = useStore(engineStore);
  return (
    <Page title="Dashboard" description="The calm colour Glowdrift sends to each of your lights, live.">
      {status.lastError && (
        <Banner tone="danger" title="Something went wrong">
          {status.lastError}
        </Banner>
      )}
      <section className="stack-tight" aria-label="Light output">
        <h2 className="section-title">Lights</h2>
        <LightTiles />
      </section>
      <EngineCard />
    </Page>
  );
}
