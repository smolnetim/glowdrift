import { useEffect, useRef, useState } from "react";
import type { ColorEvent } from "../lib/ipc";
import { getLiveColor, useLiveHexRef } from "../lib/liveColor";
import { Icon } from "./Icon";

const COPIED_MS = 1400;

/** Hex text that follows the live colour without re-rendering. */
export function LiveHex({ kind }: { kind: keyof ColorEvent }) {
  const ref = useLiveHexRef(kind);
  return <span ref={ref} className="mono">{getLiveColor()[kind]}</span>;
}

interface ColorChipProps {
  kind: keyof ColorEvent;
  label: string;
  size: "large" | "small";
}

/** A live swatch that copies its hex value when activated. */
export function ColorChip({ kind, label, size }: ColorChipProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const copy = () => {
    navigator.clipboard
      .writeText(getLiveColor()[kind])
      .then(() => {
        setCopied(true);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(false), COPIED_MS);
      })
      .catch(() => undefined);
  };

  return (
    <button
      type="button"
      className={`color-chip color-chip--${kind} color-chip--${size}`}
      onClick={copy}
      aria-label={`${label}. Copy colour code`}
    >
      <span className="color-chip-label">{label}</span>
      <span className="color-chip-value">
        <LiveHex kind={kind} />
        <span className="color-chip-copy" aria-live="polite">
          <Icon name={copied ? "check" : "copy"} size={16} />
          {copied ? "Copied" : "Copy"}
        </span>
      </span>
    </button>
  );
}

/** Plain live swatch for previews. */
export function LiveSwatch({ label }: { label: string }) {
  return (
    <div className="live-swatch" role="img" aria-label={label}>
      <LiveHex kind="smoothed" />
    </div>
  );
}
