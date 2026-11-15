import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import { useConfig } from "../hooks/useConfig";
import {
  type BridgeCandidate,
  type HueArea,
  type HueAreaKind,
  type HueBridge,
  type LightStatus,
  hueDiscover,
  hueListAreas,
  hueProbe,
} from "../lib/ipc";
import { useStore } from "../lib/store";
import {
  PAIR_WINDOW_S,
  chooseArea,
  errorMessage,
  forgetBridge,
  hueStore,
  refreshHueBridge,
  startPairing,
} from "../state/hue";
import { ConfirmDialog } from "./ConfirmDialog";
import { ChoiceGroup } from "./controls";

const bridgeLabel = (bridge: { name: string | null }) => bridge.name ?? "Hue bridge";
const lightsText = (count: number) => `${count} ${count === 1 ? "light" : "lights"}`;

const AREA_GROUPS: { kind: HueAreaKind; label: string }[] = [
  { kind: "room", label: "Rooms" },
  { kind: "zone", label: "Zones" },
  { kind: "entertainment", label: "Entertainment areas" },
];

type Discovery = { phase: "idle" } | { phase: "working" } | { phase: "done" } | { phase: "failed"; message: string };

/** The pairing progress: a countdown over the time the shell waits for the link button. */
function PairingProgress({ startedAt }: { startedAt: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, []);
  const elapsed = Math.min(PAIR_WINDOW_S, (now - startedAt) / 1000);
  const left = Math.ceil(PAIR_WINDOW_S - elapsed);
  return (
    <div className="hue-waiting">
      <p>
        <strong>Press the round link button on your Hue bridge now.</strong>
      </p>
      <p className="visually-hidden" role="status">
        Waiting for the link button to be pressed.
      </p>
      <div className="hue-countdown" aria-hidden="true">
        <progress className="progress" value={elapsed} max={PAIR_WINDOW_S} />
        <span className="muted-text">{left > 0 ? `${left} s left` : "Almost done…"}</span>
      </div>
    </div>
  );
}

/** Find a bridge (automatically or by address), then pair with it. */
function BridgeFinder({ known, onPaired }: { known: HueBridge | null; onPaired: () => void }) {
  const { pairing } = useStore(hueStore);
  const waiting = pairing.phase === "waiting";
  const addressId = useId();
  const addressErrorId = useId();
  const [candidates, setCandidates] = useState<BridgeCandidate[]>(known ? [known] : []);
  const [selected, setSelected] = useState<string | null>(known?.id ?? null);
  const [discovery, setDiscovery] = useState<Discovery>({ phase: "idle" });
  const [address, setAddress] = useState("");
  const [probing, setProbing] = useState(false);
  const [addressError, setAddressError] = useState<string | null>(null);

  const chosen = candidates.find((c) => c.id === selected) ?? null;

  function merge(found: BridgeCandidate[]) {
    setCandidates((current) => [...current.filter((c) => !found.some((f) => f.id === c.id)), ...found]);
  }

  async function find() {
    setDiscovery({ phase: "working" });
    try {
      const found = await hueDiscover();
      merge(found);
      setSelected((current) => current ?? found[0]?.id ?? null);
      setDiscovery({ phase: "done" });
    } catch (error) {
      setDiscovery({ phase: "failed", message: errorMessage(error, "The search did not work. Try again.") });
    }
  }

  async function check(event: FormEvent) {
    event.preventDefault();
    if (!address.trim()) return;
    setProbing(true);
    setAddressError(null);
    try {
      const found = await hueProbe(address.trim());
      merge([found]);
      setSelected(found.id);
      setAddress("");
    } catch (error) {
      setAddressError(errorMessage(error, "No Hue bridge answered at that address."));
    } finally {
      setProbing(false);
    }
  }

  async function pair() {
    if (!chosen) return;
    if (await startPairing(chosen.address)) onPaired();
  }

  return (
    <div className="help hue-flow">
      <h3>Connect your Hue bridge</h3>
      <p className="muted-text">
        Your PC and the Hue bridge need to be on the same network. Everything stays local; no Hue account is used.
      </p>

      <div className="hue-step" inert={waiting}>
        <div className="update-actions">
          <button
            type="button"
            className="btn"
            onClick={() => void find()}
            disabled={discovery.phase === "working" || waiting}
          >
            {discovery.phase === "working" ? "Looking…" : candidates.length > 0 ? "Search again" : "Find bridge"}
          </button>
          {discovery.phase === "working" && (
            <span className="muted-text" role="status">
              Looking for bridges on your network…
            </span>
          )}
        </div>
        {discovery.phase === "failed" && <p role="alert">{discovery.message}</p>}
        {discovery.phase === "done" && candidates.length === 0 && (
          <p role="status">No bridge was found. Enter its address below; the Hue app shows it in the bridge settings.</p>
        )}
        {candidates.length > 0 && (
          <ChoiceGroup
            legend="Hue bridges"
            name="hue-bridge"
            columns="one"
            value={selected}
            onChange={setSelected}
            options={candidates.map((c) => ({
              value: c.id,
              title: bridgeLabel(c),
              description: `${c.address} · ID ${c.id}`,
            }))}
          />
        )}
        <form className="hue-address" onSubmit={(event) => void check(event)}>
          <label className="text-field" htmlFor={addressId}>
            Or enter the bridge&apos;s address
          </label>
          <div className="text-field-row">
            <input
              id={addressId}
              className="text-input"
              type="text"
              inputMode="decimal"
              autoComplete="off"
              spellCheck={false}
              placeholder="192.168.1.23"
              value={address}
              aria-invalid={addressError ? true : undefined}
              aria-describedby={addressError ? addressErrorId : undefined}
              onChange={(e) => setAddress(e.currentTarget.value)}
            />
            <button type="submit" className="btn" disabled={probing || !address.trim()}>
              {probing ? "Checking…" : "Check address"}
            </button>
          </div>
          {addressError && (
            <p className="field-error" id={addressErrorId} role="alert">
              {addressError}
            </p>
          )}
        </form>
      </div>

      {chosen && (
        <div className="hue-step">
          <h3>Pair with {bridgeLabel(chosen)}</h3>
          {waiting ? (
            <PairingProgress startedAt={pairing.startedAt} />
          ) : (
            <p>
              Press <strong>Pair</strong>, then press the round link button on top of the bridge within {PAIR_WINDOW_S}{" "}
              seconds.
            </p>
          )}
          {pairing.phase === "failed" && <p role="alert">{pairing.message}</p>}
          <div className="update-actions">
            <button type="button" className="btn btn--primary" onClick={() => void pair()} disabled={waiting}>
              {waiting ? "Waiting for the link button…" : pairing.phase === "failed" ? "Try again" : "Pair"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/** Rooms, zones and entertainment areas to choose from, grouped by kind. */
function AreaPicker({
  areas,
  loading,
  error,
  currentId,
  focusOnMount,
  onRetry,
  onDone,
  onCancel,
}: {
  areas: HueArea[];
  loading: boolean;
  error: string | null;
  currentId: string | null;
  focusOnMount: boolean;
  onRetry: () => void;
  onDone: () => void;
  onCancel: (() => void) | null;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const selected = picked ?? (areas.some((a) => a.id === currentId) ? currentId : null);

  useEffect(() => {
    if (focusOnMount) heading.current?.focus({ preventScroll: true });
  }, [focusOnMount]);

  async function save() {
    if (!selected) return;
    setSaving(true);
    setSaveError(null);
    try {
      await chooseArea(selected);
      onDone();
    } catch (err) {
      setSaveError(errorMessage(err, "The area could not be saved."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="help hue-flow">
      <h3 ref={heading} tabIndex={-1}>
        Choose where the colour goes
      </h3>
      <p className="muted-text">Pick the room, zone or entertainment area whose lights should follow your screen.</p>
      {loading && (
        <p className="muted-text" role="status">
          Loading areas…
        </p>
      )}
      {error && (
        <>
          <p role="alert">{error}</p>
          <div className="update-actions">
            <button type="button" className="btn" onClick={onRetry}>
              Try again
            </button>
          </div>
        </>
      )}
      {!loading && !error && areas.length === 0 && (
        <p role="status">No rooms, zones or entertainment areas were found. Create one in the Hue app first.</p>
      )}
      {AREA_GROUPS.map(({ kind, label }) => {
        const group = areas.filter((a) => a.kind === kind);
        if (group.length === 0) return null;
        return (
          <fieldset className="choice-group hue-area-group" key={kind}>
            <legend className="section-label">{label}</legend>
            <div className="choice-grid choice-grid--auto">
              {group.map((area) => (
                <label className="choice" key={area.id}>
                  <input
                    type="radio"
                    name="hue-area"
                    value={area.id}
                    checked={area.id === selected}
                    onChange={() => setPicked(area.id)}
                  />
                  <span className="choice-body">
                    <span className="choice-title">{area.name}</span>
                    <span className="choice-description">{lightsText(area.lightCount)}</span>
                  </span>
                </label>
              ))}
            </div>
          </fieldset>
        );
      })}
      {saveError && <p role="alert">{saveError}</p>}
      {areas.length > 0 && (
        <div className="update-actions">
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => void save()}
            disabled={!selected || saving}
          >
            {saving ? "Saving…" : "Use this area"}
          </button>
          {onCancel && (
            <button type="button" className="btn btn--subtle" onClick={onCancel} disabled={saving}>
              Cancel
            </button>
          )}
        </div>
      )}
    </div>
  );
}

type AreasState = { phase: "idle" | "loading" | "ready" } | { phase: "failed"; message: string };

/** Hue setup inside the Hue light card: pair, choose an area, change or forget. */
export function HueSetup({ light }: { light: LightStatus }) {
  const config = useConfig();
  const { bridge } = useStore(hueStore);
  const [repairing, setRepairing] = useState(false);
  const [changingArea, setChangingArea] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);
  const [forgetError, setForgetError] = useState<string | null>(null);
  const [areas, setAreas] = useState<HueArea[]>([]);
  const [areasState, setAreasState] = useState<AreasState>({ phase: "idle" });
  const acted = useRef(false);

  useEffect(() => {
    void refreshHueBridge();
  }, [light.code]);

  const needsPairing = bridge === null || light.code === "notPaired" || repairing;
  const bridgeId = bridge?.id ?? null;

  async function loadAreas() {
    setAreasState({ phase: "loading" });
    try {
      setAreas(await hueListAreas());
      setAreasState({ phase: "ready" });
    } catch (error) {
      setAreasState({ phase: "failed", message: errorMessage(error, "The areas could not be loaded.") });
    }
  }

  useEffect(() => {
    setAreas([]);
    setAreasState({ phase: "idle" });
    if (bridgeId && !needsPairing) void loadAreas();
  }, [bridgeId, needsPairing]);

  if (bridge === undefined) {
    return (
      <p className="muted-text" role="status">
        Checking for a paired bridge…
      </p>
    );
  }

  if (needsPairing || bridge === null) {
    return (
      <BridgeFinder
        known={bridge}
        onPaired={() => {
          acted.current = true;
          setRepairing(false);
        }}
      />
    );
  }

  const currentId = config.lights.hue.areaId;
  const current = areas.find((a) => a.id === currentId) ?? null;
  const picking = light.code === "noArea" || !currentId || changingArea;
  const reconnecting = light.state === "unavailable" && !light.code;

  async function forget() {
    setConfirmForget(false);
    setForgetError(null);
    try {
      await forgetBridge();
      setChangingArea(false);
    } catch (error) {
      setForgetError(errorMessage(error, "The bridge could not be forgotten."));
    }
  }

  return (
    <>
      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-title">Bridge</div>
          <div className="setting-description">{`${bridgeLabel(bridge)} · ${bridge.address}`}</div>
        </div>
        <div className="update-actions">
          {light.state === "error" && (
            <button type="button" className="btn" onClick={() => setRepairing(true)}>
              Pair again
            </button>
          )}
          <button type="button" className="btn btn--danger-outline" onClick={() => setConfirmForget(true)}>
            Forget bridge
          </button>
        </div>
      </div>
      {forgetError && <p role="alert">{forgetError}</p>}
      {reconnecting && (
        <p className="muted-text">Glowdrift keeps trying to reach the bridge and reconnects when it answers.</p>
      )}

      {picking ? (
        <AreaPicker
          areas={areas}
          loading={areasState.phase === "loading"}
          error={areasState.phase === "failed" ? areasState.message : null}
          currentId={currentId}
          focusOnMount={acted.current}
          onRetry={() => void loadAreas()}
          onDone={() => setChangingArea(false)}
          onCancel={changingArea && currentId ? () => setChangingArea(false) : null}
        />
      ) : (
        <div className="setting-row">
          <div className="setting-text">
            <div className="setting-title">Area</div>
            <div className="setting-description">
              {current ? `${current.name} · ${lightsText(current.lightCount)}` : "Chosen area"}
            </div>
          </div>
          <button
            type="button"
            className="btn"
            onClick={() => {
              acted.current = true;
              setChangingArea(true);
              void loadAreas();
            }}
          >
            Change area
          </button>
        </div>
      )}

      <ConfirmDialog
        open={confirmForget}
        title="Forget this bridge?"
        message="Glowdrift deletes its pairing with this bridge and stops sending colour to Hue. Your lights keep their current colour. You can pair again at any time."
        confirmLabel="Forget bridge"
        onConfirm={() => void forget()}
        onCancel={() => setConfirmForget(false)}
      />
    </>
  );
}
