import type { MonitorInfo } from "../lib/ipc";
import type { MonitorPreview } from "../hooks/useMonitorPreview";
import { useConfig } from "../hooks/useConfig";
import { patchConfig } from "../state/config";
import { ChoiceGroup } from "./controls";

const describeMonitor = (monitor: MonitorInfo): string =>
  `${monitor.width} × ${monitor.height}${monitor.primary ? " · Primary" : ""}`;

/** Chooses the display every light watches. */
export function DisplayPicker({ preview }: { preview: MonitorPreview }) {
  const config = useConfig();
  const { monitors, monitorsLoading, monitorsFailed, refreshMonitors, selected } = preview;
  return (
    <div className="stack-tight">
      <div className="section-label">Display</div>
      {monitors.length > 0 ? (
        <ChoiceGroup
          legend="Display to capture"
          name="monitor"
          value={selected?.id ?? null}
          options={monitors.map((m) => ({ value: m.id, title: m.name, description: describeMonitor(m) }))}
          onChange={(id) => patchConfig({ monitorId: id })}
        />
      ) : (
        <p className="muted-text">
          {monitorsLoading
            ? "Looking for displays…"
            : monitorsFailed
              ? "Could not list your displays."
              : "No displays were found."}{" "}
          {!monitorsLoading && (
            <button type="button" className="link-button" onClick={refreshMonitors}>
              Try again
            </button>
          )}
        </p>
      )}
      {monitors.length > 0 && config.monitorId !== null && !selected && (
        <p className="muted-text">The display you chose is not connected. Pick another one.</p>
      )}
    </div>
  );
}
