import { useState } from "react";
import { useConfig } from "../hooks/useConfig";
import { LIGHT_STATE_LABEL } from "../lib/format";
import type { LightStatus } from "../lib/ipc";
import { useStore } from "../lib/store";
import { setLightEnabled } from "../state/config";
import { downloadIcueLibrary, lightsStore } from "../state/lights";
import { ToggleRow } from "./controls";
import { HueSetup } from "./HueSetup";
import { Icon } from "./Icon";

export function StateBadge({ light }: { light: LightStatus }) {
  return (
    <span className={`badge badge--${light.state}`}>
      <span className="badge-dot" aria-hidden="true" />
      {LIGHT_STATE_LABEL[light.state]}
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

function LightCard({ light }: { light: LightStatus }) {
  const config = useConfig();
  const enabled = config.lights[light.kind].enabled;
  const libraryMissing = light.kind === "icue" && light.code === "libraryMissing";
  const needsHelp = light.kind === "icue" && light.state !== "connected" && !libraryMissing;
  // Hue's setup flow explains its coded states, and "Connected" adds nothing next to the badge.
  const showDetail = light.kind === "icue" || (light.state !== "connected" && !light.code);

  return (
    <article className="card light-card" aria-label={light.name}>
      <div className="light-head">
        <span className="light-icon">
          <Icon name="lights" size={22} />
        </span>
        <div className="light-title">
          <h2>{light.name}</h2>
          <div className="light-meta">
            <StateBadge light={light} />
            {light.state === "connected" && (
              <span className="muted-text">
                {light.deviceCount} {light.deviceCount === 1 ? "device" : "devices"}
              </span>
            )}
          </div>
        </div>
      </div>
      {light.detail && showDetail && <p className="light-detail">{light.detail}</p>}
      <ToggleRow
        title="Send colour to this light"
        checked={enabled}
        onChange={(next) => setLightEnabled(light.kind, next)}
      />
      {light.kind === "icue" && <LibraryDownload missing={libraryMissing} />}
      {needsHelp && light.state !== "connecting" && <IcueSetupHelp disabled={!enabled} />}
      {light.kind === "hue" && <HueSetup light={light} />}
    </article>
  );
}

/** Light cards with setup guidance; safe with an empty list. */
export function LightsPanel() {
  const lights = useStore(lightsStore);
  return (
    <div className="stack">
      {lights.length === 0 ? (
        <article className="card light-card">
          <div className="light-head">
            <span className="light-icon">
              <Icon name="lights" size={22} />
            </span>
            <div className="light-title">
              <h2>Corsair iCUE</h2>
              <div className="light-meta">
                <span className="muted-text">Checking for lights&hellip;</span>
              </div>
            </div>
          </div>
        </article>
      ) : (
        lights.map((light) => <LightCard key={light.id} light={light} />)
      )}
      <ul className="notes">
        <li>No virtual display or driver is installed. Glowdrift talks to iCUE through its official SDK.</li>
        <li>Philips Hue is controlled through your bridge on the local network; pairing needs one press of its link button.</li>
      </ul>
    </div>
  );
}
