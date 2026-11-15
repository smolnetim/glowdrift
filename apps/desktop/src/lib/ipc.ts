// Typed contract between the frontend and the Rust shell.
// Commands use snake_case names; payloads are camelCase.
// Outside Tauri (plain browser dev server) every call is served by ./ipc.mock.

import { type InvokeArgs, invoke, isTauri } from "@tauri-apps/api/core";
import { type UnlistenFn, listen } from "@tauri-apps/api/event";

/** "#rrggbb", lowercase. */
export type HexColor = string;

export interface NormRect {
  /** Fractions 0..1 of the monitor. */
  x: number;
  y: number;
  width: number;
  height: number;
}

export type ReduceMode = "average" | "dominant";

export interface Config {
  schemaVersion: number;
  onboardingComplete: boolean;
  /** null = primary monitor. */
  monitorId: string | null;
  region: NormRect;
  /** 1..=60 */
  captureFps: number;
  mode: ReduceMode;
  /** 0..=5000, exponential time constant. */
  smoothingMs: number;
  /** 0..=1 */
  brightness: number;
  /** 0..=2 */
  saturation: number;
  lights: {
    icue: { enabled: boolean };
    hue: {
      enabled: boolean;
      bridgeId: string | null;
      areaId: string | null;
      /** Hue light id -> zone override, as fractions of the monitor. */
      zones: Record<string, NormRect>;
      /** Ids of single bridge lights driven in addition to the area. */
      extraLights: string[];
    };
  };
  startMinimized: boolean;
  autoUpdate: boolean;
}

export interface MonitorInfo {
  id: string;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  primary: boolean;
  scaleFactor: number;
}

export interface Thumbnail {
  width: number;
  height: number;
  /** PNG data URL of the whole monitor. */
  dataUrl: string;
}

export interface EngineStatus {
  running: boolean;
  measuredFps: number;
  lastError: string | null;
  target: HexColor;
  smoothed: HexColor;
}

export type LightKind = "icue" | "hue";
export type LightState = "connected" | "connecting" | "unavailable" | "error" | "disabled";

/** Machine-readable cause behind a light state; `detail` is for display only. */
export type LightCode = "libraryMissing" | "notPaired" | "noArea";

export interface LightStatus {
  id: string;
  kind: LightKind;
  name: string;
  enabled: boolean;
  state: LightState;
  detail: string | null;
  deviceCount: number;
  /** Absent or null when the state has no specific cause. */
  code?: LightCode | null;
}

/** A Hue bridge found on the network. */
export interface BridgeCandidate {
  id: string;
  address: string;
  name: string | null;
}

/** The paired Hue bridge; carries no secrets. */
export interface HueBridge {
  id: string;
  address: string;
  name: string | null;
}

export type HueAreaKind = "room" | "zone" | "entertainment";

/** A room, zone or entertainment area whose lights can receive the colour. */
export interface HueArea {
  id: string;
  kind: HueAreaKind;
  name: string;
  lightCount: number;
}

export interface AppInfo {
  version: string;
  name: string;
}

export interface ColorEvent {
  target: HexColor;
  smoothed: HexColor;
}

export interface UpdaterReadyEvent {
  version: string;
  /** Installing shows a Windows permission prompt (a per-machine install). */
  needsElevation: boolean;
}

type Mock = typeof import("./ipc.mock");
const loadMock = (): Promise<Mock> => import("./ipc.mock");

function call<T>(command: string, args: InvokeArgs | undefined, mocked: (m: Mock) => Promise<T>): Promise<T> {
  return isTauri() ? invoke<T>(command, args) : loadMock().then(mocked);
}

function subscribe<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  return isTauri()
    ? listen<T>(event, (e) => handler(e.payload))
    : loadMock().then((m) => m.mockListen(event, handler));
}

export const getConfig = () => call<Config>("get_config", undefined, (m) => m.getConfig());
export const setConfig = (config: Config) =>
  call<Config>("set_config", { config }, (m) => m.setConfig(config));
export const listMonitors = () => call<MonitorInfo[]>("list_monitors", undefined, (m) => m.listMonitors());
export const getMonitorThumbnail = (monitorId: string | null, maxWidth: number) =>
  call<Thumbnail>("get_monitor_thumbnail", { monitorId, maxWidth }, (m) =>
    m.getMonitorThumbnail(monitorId, maxWidth),
  );
export const getEngineStatus = () =>
  call<EngineStatus>("get_engine_status", undefined, (m) => m.getEngineStatus());
export const setEngineRunning = (running: boolean) =>
  call<EngineStatus>("set_engine_running", { running }, (m) => m.setEngineRunning(running));
export const listLights = () => call<LightStatus[]>("list_lights", undefined, (m) => m.listLights());
/** Installs the update the shell has already downloaded; resolves false when none is staged. */
export const installPendingUpdate = () =>
  call<boolean>("install_pending_update", undefined, () => Promise.resolve(false));
/** True when installing an update shows a Windows permission prompt (a per-machine install). */
export const updateNeedsElevation = () =>
  call<boolean>("update_needs_elevation", undefined, (m) => m.updateNeedsElevation());
/**
 * Downloads and verifies Corsair's iCUE SDK library, then makes the iCUE light retry loading it.
 * Rejects with a user-presentable message.
 */
export const installIcueSdk = () =>
  call<LightStatus[]>("install_icue_sdk", undefined, (m) => m.installIcueSdk());
/** Looks for Hue bridges on the network (mDNS, then the Hue discovery service); takes about 4 s. */
export const hueDiscover = () => call<BridgeCandidate[]>("hue_discover", undefined, (m) => m.hueDiscover());
/** Reads a manually entered bridge address. Rejects with a user-presentable message. */
export const hueProbe = (address: string) =>
  call<BridgeCandidate>("hue_probe", { address }, (m) => m.hueProbe(address));
/**
 * Pairs with the bridge at `address`: the user must press the bridge's link button while this runs
 * (it waits up to 30 s). Rejects with a user-presentable message, including on timeout.
 */
export const huePair = (address: string) =>
  call<HueBridge>("hue_pair", { address }, (m) => m.huePair(address));
/** The paired bridge, or null when not paired. */
export const hueBridge = () => call<HueBridge | null>("hue_bridge", undefined, (m) => m.hueBridge());
/** Rooms, zones and entertainment areas of the paired bridge. */
export const hueListAreas = () => call<HueArea[]>("hue_list_areas", undefined, (m) => m.hueListAreas());
export const hueSelectArea = (areaId: string) =>
  call<Config>("hue_select_area", { areaId }, (m) => m.hueSelectArea(areaId));
/** Deletes the stored bridge credentials and clears the bridge and area from the config. */
export const hueForget = () => call<Config>("hue_forget", undefined, (m) => m.hueForget());
/** Tells the shell the first screen is rendered so it can show the window. */
export const frontendReady = () => call<void>("frontend_ready", undefined, () => Promise.resolve());
/** Downloads and verifies an available update and stages it; resolves its version, or null when none. */
export const stageUpdate = () =>
  call<string | null>("stage_update", undefined, () => Promise.resolve(null));
export const getAppInfo = () => call<AppInfo>("get_app_info", undefined, (m) => m.getAppInfo());

/** Emitted at most ~20/s, only while the main window is visible. */
export const onColor = (handler: (e: ColorEvent) => void): Promise<UnlistenFn> =>
  subscribe<ColorEvent>("engine://color", handler);
export const onEngineStatus = (handler: (s: EngineStatus) => void): Promise<UnlistenFn> =>
  subscribe<EngineStatus>("engine://status", handler);
export const onLightsStatus = (handler: (s: LightStatus[]) => void): Promise<UnlistenFn> =>
  subscribe<LightStatus[]>("lights://status", handler);
/** An update has been downloaded and only needs to be installed. */
export const onUpdaterReady = (handler: (e: UpdaterReadyEvent) => void): Promise<UnlistenFn> =>
  subscribe<UpdaterReadyEvent>("updater://ready", handler);
