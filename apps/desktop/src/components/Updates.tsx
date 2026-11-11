import { useStore } from "../lib/store";
import { restartApp } from "../lib/system";
import { checkNow, installUpdate, updateStore } from "../state/updates";
import { Icon } from "./Icon";

/** Non-blocking pill shown once an update only needs a restart. */
export function UpdatePill() {
  const state = useStore(updateStore);
  if (state.phase !== "ready") return null;
  return (
    <button type="button" className="update-pill" onClick={() => void restartApp()}>
      <Icon name="download" size={16} />
      <span>Update ready &ndash; Restart</span>
    </button>
  );
}

/** Check-now control with its status line, used on the Settings page. */
export function UpdateChecker() {
  const state = useStore(updateStore);
  const busy = state.phase === "checking" || state.phase === "downloading";

  let status: string | null = null;
  if (state.phase === "checking") status = "Checking for updates…";
  else if (state.phase === "upToDate") status = "Glowdrift is up to date.";
  else if (state.phase === "available") status = `Version ${state.update.version} is available.`;
  else if (state.phase === "downloading") {
    status =
      state.progress === null
        ? `Downloading version ${state.version}…`
        : `Downloading version ${state.version}… ${Math.round(state.progress * 100)}%`;
  } else if (state.phase === "ready") status = `Version ${state.version} is ready to install.`;
  else if (state.phase === "failed") {
    status = state.stage === "check" ? "Could not check for updates." : "Could not install the update.";
  }

  return (
    <div className="update-checker">
      <div className="update-actions">
        {state.phase === "ready" ? (
          <button type="button" className="btn btn--primary" onClick={() => void restartApp()}>
            Restart to finish
          </button>
        ) : state.phase === "available" ? (
          <button type="button" className="btn btn--primary" onClick={() => void installUpdate(state.update)}>
            Download and install
          </button>
        ) : (
          <button type="button" className="btn" onClick={() => void checkNow()} disabled={busy}>
            Check now
          </button>
        )}
        {status && (
          <span className="muted-text" role="status">
            {status}
          </span>
        )}
      </div>
      {state.phase === "downloading" && state.progress !== null && (
        <progress className="progress" value={state.progress} max={1} aria-label="Download progress" />
      )}
    </div>
  );
}
