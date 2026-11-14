import { useStore } from "../lib/store";
import { checkNow, installUpdate, restartToUpdate, updateStore } from "../state/updates";
import { Icon } from "./Icon";

const PERMISSION_HINT = "Windows will ask for permission to install the update.";

/** Non-blocking pill shown once an update has been downloaded and only needs to be installed. */
export function UpdatePill() {
  const state = useStore(updateStore);
  if (state.phase !== "ready") return null;
  return (
    <button
      type="button"
      className="update-pill"
      title={state.error ?? (state.needsElevation ? PERMISSION_HINT : undefined)}
      onClick={() => void restartToUpdate()}
    >
      <Icon name="download" size={16} />
      <span>{pillLabel(state.needsElevation, state.error !== undefined)}</span>
    </button>
  );
}

function pillLabel(needsElevation: boolean, failed: boolean): string {
  if (failed) return "Update failed – Try again";
  return needsElevation ? "Update ready – Install (needs permission)" : "Update ready – Restart";
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
  } else if (state.phase === "ready") {
    status = state.error ?? `Version ${state.version} is ready to install.`;
    if (state.needsElevation && !state.error) status += " Windows will ask for permission.";
  }
  else if (state.phase === "failed") {
    status = state.stage === "check" ? "Could not check for updates." : "Could not install the update.";
  }

  return (
    <div className="update-checker">
      <div className="update-actions">
        {state.phase === "ready" ? (
          <button type="button" className="btn btn--primary" onClick={() => void restartToUpdate()}>
            {state.needsElevation ? "Install update" : "Restart to finish"}
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
