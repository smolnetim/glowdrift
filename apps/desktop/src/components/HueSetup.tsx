import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import { useConfig } from "../hooks/useConfig";
import {
  type BridgeCandidate,
  type BridgeLight,
  type HueArea,
  type HueBridge,
  type LightStatus,
  hueDiscover,
  hueListAreas,
  hueListLights,
  hueProbe,
} from "../lib/ipc";
import { useStore } from "../lib/store";
import { setHueExtraLights } from "../state/config";
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

/** Entertainment areas to choose from. */
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
      <p className="muted-text">
        Pick the entertainment area whose lights should follow your screen. Each light&apos;s place on the screen
        comes from the positions set up for the area in the Hue app (Entertainment areas).
      </p>
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
        <p role="status">
          No entertainment areas were found. Create one in the Hue app first: Settings, Entertainment areas.
        </p>
      )}
      {areas.length > 0 && (
        <fieldset className="choice-group hue-area-group">
          <legend className="section-label">Entertainment areas</legend>
          <div className="choice-grid choice-grid--auto">
            {areas.map((area) => (
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
      )}
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

/** Single bridge lights driven in addition to the area, each with its own zone. */
function ExtraLights() {
  const config = useConfig();
  const extras = config.lights.hue.extraLights;
  const areaId = config.lights.hue.areaId;
  const [lights, setLights] = useState<BridgeLight[]>([]);
  const [state, setState] = useState<AreasState>({ phase: "loading" });
  const [choice, setChoice] = useState("");
  const selectId = useId();

  async function load() {
    setState({ phase: "loading" });
    try {
      setLights(await hueListLights());
      setState({ phase: "ready" });
    } catch (error) {
      setState({ phase: "failed", message: errorMessage(error, "The bridge's lights could not be loaded.") });
    }
  }

  // The shell leaves out the lights of the chosen area, so the list is read again when the area changes.
  useEffect(() => {
    void load();
  }, [areaId]);

  const available = lights.filter((l) => !extras.includes(l.id));
  const nameOf = (id: string) => lights.find((l) => l.id === id)?.name ?? "Unknown light";
  const picked = available.some((l) => l.id === choice) ? choice : (available[0]?.id ?? "");

  return (
    <div className="help hue-flow">
      <h3>Extra lights</h3>
      <p className="muted-text">
        Add single lights that are not in the entertainment area, such as a desk lamp. Place each light&apos;s zone on
        the Capture page; it starts in the middle of the display.
      </p>
      {extras.length > 0 && (
        <ul className="zone-list">
          {extras.map((id) => (
            <li key={id} className="zone-row">
              <span className="zone-pick zone-pick--static">
                <span className="zone-name">{nameOf(id)}</span>
              </span>
              <button
                type="button"
                className="btn btn--subtle"
                aria-label={`Remove ${nameOf(id)}`}
                onClick={() => setHueExtraLights(extras.filter((e) => e !== id))}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
      {state.phase === "loading" && (
        <p className="muted-text" role="status">
          Loading lights…
        </p>
      )}
      {state.phase === "failed" && (
        <>
          <p role="alert">{state.message}</p>
          <div className="update-actions">
            <button type="button" className="btn" onClick={() => void load()}>
              Try again
            </button>
          </div>
        </>
      )}
      {state.phase === "ready" && available.length === 0 && (
        <p className="muted-text">No other lights are available to add.</p>
      )}
      {state.phase === "ready" && available.length > 0 && (
        <div className="text-field-row">
          <label className="visually-hidden" htmlFor={selectId}>
            Light to add
          </label>
          <select id={selectId} className="text-input" value={picked} onChange={(e) => setChoice(e.currentTarget.value)}>
            {available.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn"
            disabled={!picked}
            onClick={() => setHueExtraLights([...extras, picked])}
          >
            Add light
          </button>
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

      <ExtraLights />

      <ConfirmDialog
        open={confirmForget}
        title="Forget this bridge?"
        message="Glowdrift deletes its pairing with this bridge and stops sending colour to Hue. Your lights return to how they were before Glowdrift started. You can pair again at any time."
        confirmLabel="Forget bridge"
        onConfirm={() => void forget()}
        onCancel={() => setConfirmForget(false)}
      />
    </>
  );
}
