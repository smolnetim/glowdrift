import { useConfig } from "../hooks/useConfig";
import { OUTPUT_STATE_LABEL } from "../lib/format";
import type { OutputStatus } from "../lib/ipc";
import { useStore } from "../lib/store";
import { patchConfig } from "../state/config";
import { outputsStore } from "../state/outputs";
import { ToggleRow } from "./controls";
import { Icon } from "./Icon";

export function StateBadge({ output }: { output: OutputStatus }) {
  return (
    <span className={`badge badge--${output.state}`}>
      <span className="badge-dot" aria-hidden="true" />
      {OUTPUT_STATE_LABEL[output.state]}
    </span>
  );
}

export function IcueSetupHelp({ disabled }: { disabled: boolean }) {
  return (
    <div className="help">
      <h3>{disabled ? "To connect Corsair iCUE" : "Could not reach Corsair iCUE"}</h3>
      <ol>
        <li>Open the iCUE app.</li>
        <li>Go to Settings, then General, and turn on the SDK or third-party control option.</li>
        <li>Keep iCUE running while you use Glowdrift.</li>
      </ol>
      <p className="muted-text">If it still does not connect, restart iCUE and check this page again.</p>
    </div>
  );
}

function OutputCard({ output }: { output: OutputStatus }) {
  const config = useConfig();
  const enabled = output.kind === "icue" ? config.outputs.icue.enabled : output.enabled;
  const needsHelp = output.kind === "icue" && output.state !== "connected";

  return (
    <article className="card output-card" aria-label={output.name}>
      <div className="output-head">
        <span className="output-icon">
          <Icon name="lights" size={22} />
        </span>
        <div className="output-title">
          <h2>{output.name}</h2>
          <div className="output-meta">
            <StateBadge output={output} />
            {output.state === "connected" && (
              <span className="muted-text">
                {output.deviceCount} {output.deviceCount === 1 ? "device" : "devices"}
              </span>
            )}
          </div>
        </div>
      </div>
      {output.detail && <p className="output-detail">{output.detail}</p>}
      <ToggleRow
        title="Send colour to this output"
        checked={enabled}
        onChange={(next) => patchConfig({ outputs: { ...config.outputs, [output.kind]: { enabled: next } } })}
      />
      {needsHelp && output.state !== "connecting" && <IcueSetupHelp disabled={!enabled} />}
    </article>
  );
}

/** Output cards with setup guidance; safe with an empty list. */
export function OutputsPanel() {
  const outputs = useStore(outputsStore);
  const config = useConfig();
  return (
    <div className="stack">
      {outputs.length === 0 ? (
        <article className="card output-card">
          <div className="output-head">
            <span className="output-icon">
              <Icon name="lights" size={22} />
            </span>
            <div className="output-title">
              <h2>Corsair iCUE</h2>
              <div className="output-meta">
                <span className="muted-text">Checking for lights&hellip;</span>
              </div>
            </div>
          </div>
          <IcueSetupHelp disabled={!config.outputs.icue.enabled} />
        </article>
      ) : (
        outputs.map((output) => <OutputCard key={output.id} output={output} />)
      )}
      <ul className="notes">
        <li>No virtual display or driver is installed. Glowdrift talks to iCUE through its official SDK.</li>
        <li>Philips Hue support is planned for a later version.</li>
      </ul>
    </div>
  );
}
