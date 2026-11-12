import { useState } from "react";
import { useConfig } from "../hooks/useConfig";
import { OUTPUT_STATE_LABEL } from "../lib/format";
import type { OutputStatus } from "../lib/ipc";
import { useStore } from "../lib/store";
import { patchConfig } from "../state/config";
import { downloadIcueLibrary, outputsStore } from "../state/outputs";
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

type DownloadState = { phase: "idle" } | { phase: "working" } | { phase: "done" } | { phase: "failed"; message: string };

/** Explains the missing iCUE library and offers the verified download. */
function LibraryDownload({ missing }: { missing: boolean }) {
  const [state, setState] = useState<DownloadState>({ phase: "idle" });
  const working = state.phase === "working";

  async function download() {
    setState({ phase: "working" });
    try {
      await downloadIcueLibrary();
      setState({ phase: "done" });
    } catch (error) {
      setState({ phase: "failed", message: error instanceof Error ? error.message : "Download failed." });
    }
  }

  if (!missing && state.phase !== "done") return null;
  return (
    <div className="help">
      <h3>Corsair library needed</h3>
      <p>
        Glowdrift needs Corsair&apos;s official iCUE SDK library. It is downloaded from Corsair&apos;s GitHub and
        verified before use.
      </p>
      {state.phase === "done" && (
        <p role="status">Library downloaded and verified. Glowdrift is now connecting to iCUE.</p>
      )}
      {state.phase === "failed" && <p role="alert">{state.message}</p>}
      {missing && (
        <div className="update-actions">
          <button type="button" className="btn btn--primary" onClick={() => void download()} disabled={working}>
            {working ? "Downloading…" : state.phase === "failed" ? "Try again" : "Download Corsair library"}
          </button>
          {working && (
            <span className="muted-text" role="status">
              Downloading and verifying…
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function OutputCard({ output }: { output: OutputStatus }) {
  const config = useConfig();
  const enabled = output.kind === "icue" ? config.outputs.icue.enabled : output.enabled;
  const libraryMissing = output.kind === "icue" && output.code === "libraryMissing";
  const needsHelp = output.kind === "icue" && output.state !== "connected" && !libraryMissing;

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
      {output.kind === "icue" && <LibraryDownload missing={libraryMissing} />}
      {needsHelp && output.state !== "connecting" && <IcueSetupHelp disabled={!enabled} />}
    </article>
  );
}

/** Output cards with setup guidance; safe with an empty list. */
export function OutputsPanel() {
  const outputs = useStore(outputsStore);
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
