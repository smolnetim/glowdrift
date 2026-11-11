import { useAutostart } from "../hooks/useAutostart";
import { useConfig } from "../hooks/useConfig";
import { patchConfig } from "../state/config";
import { ToggleRow } from "./controls";

export function StartWithWindowsToggle() {
  const { enabled, failed, change } = useAutostart();
  return (
    <>
      <ToggleRow
        title="Start with Windows"
        description="Launch Glowdrift when you sign in."
        checked={enabled === true}
        disabled={enabled === null}
        onChange={(next) => void change(next)}
      />
      {failed && <p className="muted-text field-error">Could not change this setting. Try again.</p>}
    </>
  );
}

export function StartMinimisedToggle() {
  const config = useConfig();
  return (
    <ToggleRow
      title="Start minimised to tray"
      description="Open quietly in the notification area instead of showing this window."
      checked={config.startMinimized}
      onChange={(startMinimized) => patchConfig({ startMinimized })}
    />
  );
}

export function AutoUpdateToggle() {
  const config = useConfig();
  return (
    <ToggleRow
      title="Install updates automatically"
      description="Download new versions in the background and ask you to restart."
      checked={config.autoUpdate}
      onChange={(autoUpdate) => patchConfig({ autoUpdate })}
    />
  );
}
