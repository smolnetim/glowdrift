import { type ReactNode, useEffect, useRef, useState } from "react";
import { useAreaLights } from "../hooks/useAreaLights";
import { inkFor, rgbHex, subscribeLiveColor } from "../lib/liveColor";
import type { LightStatus } from "../lib/ipc";
import { useStore } from "../lib/store";
import { engineStore } from "../state/engine";
import { lightsStore } from "../state/lights";
import { Icon } from "./Icon";
import { StateBadge } from "./LightsPanel";
import { LiveHex } from "./LiveColor";

const COPIED_MS = 1400;

interface LiveTileProps {
  name: string;
  /** Index into the engine's zone colours; undefined = the whole-region colour. */
  colorIndex?: number;
  paused: boolean;
  children: ReactNode;
}

/** A light with a live colour; the swatch is written straight to the DOM so colour updates never re-render. */
function LiveTile({ name, colorIndex, paused, children }: LiveTileProps) {
  const swatch = useRef<HTMLButtonElement>(null);
  const hex = useRef<HTMLSpanElement>(null);
  const timer = useRef<number | undefined>(undefined);
  const [copied, setCopied] = useState(false);

  useEffect(
    () =>
      subscribeLiveColor((color) => {
        const zone = colorIndex === undefined ? undefined : color.zones?.[colorIndex];
        const value = colorIndex === undefined ? color.smoothed : zone ? rgbHex(zone) : null;
        if (swatch.current) {
          swatch.current.style.backgroundColor = value ?? "transparent";
          swatch.current.style.color = value ? inkFor(value) : "";
        }
        if (hex.current) hex.current.textContent = value ?? "–";
      }),
    [colorIndex],
  );
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const copy = () => {
    navigator.clipboard
      .writeText(hex.current?.textContent ?? "")
      .then(() => {
        setCopied(true);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(false), COPIED_MS);
      })
      .catch(() => undefined);
  };

  return (
    <li className={`light-tile${paused ? " light-tile--paused" : ""}`}>
      <button type="button" ref={swatch} className="tile-swatch" onClick={copy} aria-label={`${name} colour. Copy colour code`}>
        <span ref={hex} className="mono" />
        <span className="tile-copy" aria-live="polite">
          <Icon name={copied ? "check" : "copy"} size={16} />
          {copied ? "Copied" : "Copy"}
        </span>
      </button>
      <div className="tile-meta">
        <div className="tile-name">{name}</div>
        <div className="muted-text">{paused ? "Engine paused" : children}</div>
      </div>
    </li>
  );
}

/** A light that has no colour to show: turned off, not connected or still starting. */
function IdleTile({ name, status, children }: { name: string; status?: LightStatus; children: ReactNode }) {
  return (
    <li className="light-tile light-tile--idle">
      <div className="tile-swatch tile-swatch--empty">{status ? <StateBadge light={status} /> : null}</div>
      <div className="tile-meta">
        <div className="tile-name">{name}</div>
        <div className="muted-text">{children}</div>
      </div>
    </li>
  );
}

const idleText = (status: LightStatus | undefined, fallback: string): string => {
  if (!status) return "Checking…";
  if (status.state === "disabled") return "Turned off. Turn it on on the Lights page.";
  if (status.state === "connecting") return "Connecting…";
  return status.detail ?? fallback;
};

/** One tile per active light: iCUE with the whole-region colour, each Hue lamp with its own zone colour. */
export function LightTiles() {
  const lights = useStore(lightsStore);
  const { status: engine } = useStore(engineStore);
  const areaLights = useAreaLights();
  const icue = lights.find((l) => l.kind === "icue");
  const hue = lights.find((l) => l.kind === "hue");
  const paused = !engine.running;

  return (
    <ul className="tile-grid" aria-label="Colour sent to each light">
      {icue?.state === "connected" ? (
        <LiveTile name={icue.name} paused={paused}>
          Whole region · target <LiveHex kind="target" />
        </LiveTile>
      ) : (
        <IdleTile name={icue?.name ?? "Corsair iCUE"} status={icue}>
          {idleText(icue, "Not connected.")}
        </IdleTile>
      )}
      {hue?.state === "connected" && areaLights.length > 0 ? (
        areaLights.map((light, index) => (
          <LiveTile key={light.id} name={light.name} colorIndex={index} paused={paused}>
            Philips Hue · own zone
          </LiveTile>
        ))
      ) : (
        <IdleTile name={hue?.name ?? "Philips Hue"} status={hue}>
          {hue?.state === "connected" ? "Waiting for its lights…" : idleText(hue, "Not connected.")}
        </IdleTile>
      )}
    </ul>
  );
}
