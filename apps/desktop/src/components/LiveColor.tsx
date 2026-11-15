import { type LiveColorKind, getLiveColor, useLiveHexRef } from "../lib/liveColor";

/** Hex text that follows the live colour without re-rendering. */
export function LiveHex({ kind }: { kind: LiveColorKind }) {
  const ref = useLiveHexRef(kind);
  return <span ref={ref} className="mono">{getLiveColor()[kind]}</span>;
}

/** Plain live swatch for previews. */
export function LiveSwatch({ label }: { label: string }) {
  return (
    <div className="live-swatch" role="img" aria-label={label}>
      <LiveHex kind="smoothed" />
    </div>
  );
}
