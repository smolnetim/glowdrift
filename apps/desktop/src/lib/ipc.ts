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
  outputs: { icue: { enabled: boolean } };
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
  raw: HexColor;
  smoothed: HexColor;
}

export type OutputKind = "icue";
export type OutputState = "connected" | "connecting" | "unavailable" | "error" | "disabled";

export interface OutputStatus {
  id: string;
  kind: OutputKind;
  name: string;
  enabled: boolean;
  state: OutputState;
  detail: string | null;
  deviceCount: number;
}

export interface AppInfo {
  version: string;
  name: string;
}

export interface ColorEvent {
  raw: HexColor;
  smoothed: HexColor;
}

export interface UpdaterReadyEvent {
  version: string;
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
export const listOutputs = () => call<OutputStatus[]>("list_outputs", undefined, (m) => m.listOutputs());
/** Installs the update the shell has already downloaded; resolves false when none is staged. */
export const installPendingUpdate = () =>
  call<boolean>("install_pending_update", undefined, () => Promise.resolve(false));
export const getAppInfo = () => call<AppInfo>("get_app_info", undefined, (m) => m.getAppInfo());

/** Emitted at most ~20/s, only while the main window is visible. */
export const onColor = (handler: (e: ColorEvent) => void): Promise<UnlistenFn> =>
  subscribe<ColorEvent>("engine://color", handler);
export const onEngineStatus = (handler: (s: EngineStatus) => void): Promise<UnlistenFn> =>
  subscribe<EngineStatus>("engine://status", handler);
export const onOutputsStatus = (handler: (s: OutputStatus[]) => void): Promise<UnlistenFn> =>
  subscribe<OutputStatus[]>("outputs://status", handler);
/** An update has been downloaded and only needs a restart. */
export const onUpdaterReady = (handler: (e: UpdaterReadyEvent) => void): Promise<UnlistenFn> =>
  subscribe<UpdaterReadyEvent>("updater://ready", handler);
