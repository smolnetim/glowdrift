// In-memory stand-in for the Rust shell, used when the UI runs in a plain browser.
// Query flags: ?onboarded starts with setup complete, ?icue=unavailable simulates
// iCUE being unreachable, ?update=1 simulates an available update.

import type { UnlistenFn } from "@tauri-apps/api/event";
import { CAPTURE_FPS_MAX, CAPTURE_FPS_MIN, DEFAULT_CONFIG, SATURATION_MAX, SMOOTHING_MS_MAX } from "./defaults";
import { clamp } from "./format";
import type {
  AppInfo,
  ColorEvent,
  Config,
  EngineStatus,
  MonitorInfo,
  OutputStatus,
  Thumbnail,
} from "./ipc";
import type { AvailableUpdate } from "./system";

const params = new URLSearchParams(window.location.search);

const MONITORS: MonitorInfo[] = [
  { id: "mon-1", name: "Display 1", x: 0, y: 0, width: 5120, height: 1440, primary: true, scaleFactor: 1.25 },
  { id: "mon-2", name: "Display 2", x: -3440, y: 0, width: 3440, height: 1440, primary: false, scaleFactor: 1 },
  { id: "mon-3", name: "Display 3", x: -5360, y: 180, width: 1920, height: 1080, primary: false, scaleFactor: 1 },
];

let config: Config = { ...DEFAULT_CONFIG, onboardingComplete: params.has("onboarded") };
let running = true;
let engineColor: ColorEvent = { raw: "#000000", smoothed: "#000000" };

type Handler = (payload: unknown) => void;
const handlers = new Map<string, Set<Handler>>();

export function mockListen<T>(event: string, handler: (payload: T) => void): UnlistenFn {
  const set = handlers.get(event) ?? new Set<Handler>();
  const wrapped = handler as Handler;
  set.add(wrapped);
  handlers.set(event, set);
  return () => {
    set.delete(wrapped);
  };
}

function emit(event: string, payload: unknown): void {
  for (const handler of handlers.get(event) ?? []) handler(payload);
}

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

function validated(next: Config): Config {
  return {
    ...next,
    schemaVersion: 1,
    monitorId: next.monitorId || null,
    captureFps: Math.round(clamp(next.captureFps, CAPTURE_FPS_MIN, CAPTURE_FPS_MAX)),
    smoothingMs: Math.round(clamp(next.smoothingMs, 0, SMOOTHING_MS_MAX)),
    brightness: clamp(next.brightness, 0, 1),
    saturation: clamp(next.saturation, 0, SATURATION_MAX),
  };
}

function engineStatus(): EngineStatus {
  const jitter = (Math.sin(Date.now() / 900) + 1) / 2;
  return {
    running,
    measuredFps: running ? Math.max(0, config.captureFps - jitter * 0.8) : 0,
    lastError: null,
    ...engineColor,
  };
}

function outputs(): OutputStatus[] {
  const base = { id: "icue", kind: "icue", name: "Corsair iCUE", enabled: config.outputs.icue.enabled } as const;
  if (!base.enabled) return [{ ...base, state: "disabled", detail: null, deviceCount: 0 }];
  if (params.get("icue") === "unavailable") {
    return [{ ...base, state: "unavailable", detail: "Could not reach the iCUE SDK.", deviceCount: 0 }];
  }
  return [{ ...base, state: "connected", detail: "Connected to iCUE 5.52", deviceCount: 7 }];
}

export const getConfig = async (): Promise<Config> => config;

export async function setConfig(next: Config): Promise<Config> {
  const previous = config;
  config = validated(next);
  if (previous.outputs.icue.enabled !== config.outputs.icue.enabled) emit("outputs://status", outputs());
  return config;
}

export const listMonitors = async (): Promise<MonitorInfo[]> => MONITORS;

export async function getMonitorThumbnail(monitorId: string | null, maxWidth: number): Promise<Thumbnail> {
  const monitor = MONITORS.find((m) => m.id === monitorId) ?? MONITORS[0];
  const width = Math.min(maxWidth, monitor?.width ?? 960);
  const height = Math.round(width / ((monitor?.width ?? 16) / (monitor?.height ?? 9)));
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (ctx) {
    const sky = ctx.createLinearGradient(0, 0, width, height);
    sky.addColorStop(0, "#1b2a6b");
    sky.addColorStop(0.55, "#b4456a");
    sky.addColorStop(1, "#f2a65a");
    ctx.fillStyle = sky;
    ctx.fillRect(0, 0, width, height);
    ctx.fillStyle = "rgba(10, 12, 30, 0.55)";
    ctx.fillRect(width * 0.06, height * 0.12, width * 0.34, height * 0.62);
    ctx.fillRect(width * 0.46, height * 0.12, width * 0.48, height * 0.3);
    ctx.fillRect(width * 0.46, height * 0.48, width * 0.48, height * 0.26);
    ctx.fillStyle = "rgba(255, 255, 255, 0.85)";
    ctx.fillRect(0, height * 0.92, width, height * 0.08);
  }
  return { width, height, dataUrl: canvas.toDataURL("image/png") };
}

export async function getEngineStatus(): Promise<EngineStatus> {
  return engineStatus();
}

export async function setEngineRunning(next: boolean): Promise<EngineStatus> {
  running = next;
  const status = engineStatus();
  emit("engine://status", status);
  return status;
}

export const listOutputs = async (): Promise<OutputStatus[]> => outputs();
export const getAppInfo = async (): Promise<AppInfo> => ({ name: "Glowdrift", version: "0.1.0" });

const SCENES = ["#3b6cff", "#e5484d", "#2fbf71", "#f5a524", "#9b5de5", "#12a594"];

function channel(hex: string, index: number): number {
  return Number.parseInt(hex.slice(1 + index * 2, 3 + index * 2), 16);
}

function mix(from: string, to: string, weight: number): string {
  const parts = [0, 1, 2].map((i) => {
    const value = Math.round(channel(from, i) + (channel(to, i) - channel(from, i)) * weight);
    return value.toString(16).padStart(2, "0");
  });
  return `#${parts.join("")}`;
}

const TICK_MS = 50;
let scene = 0;
let elapsed = 0;
setInterval(() => {
  if (!running || document.hidden) return;
  elapsed += TICK_MS;
  if (elapsed >= 2500) {
    elapsed = 0;
    scene = (scene + 1) % SCENES.length;
  }
  const raw = SCENES[scene] ?? "#000000";
  const weight = 1 - Math.exp(-TICK_MS / Math.max(config.smoothingMs, 1));
  engineColor = { raw, smoothed: mix(engineColor.smoothed, raw, weight) };
  emit("engine://color", engineColor);
}, TICK_MS);

let autostart = false;
export const isAutostartEnabled = async (): Promise<boolean> => autostart;
export async function setAutostart(enabled: boolean): Promise<void> {
  autostart = enabled;
}

export async function checkForUpdate(): Promise<AvailableUpdate | null> {
  await delay(700);
  if (!params.has("update")) return null;
  return {
    version: "0.2.0",
    async install(onProgress) {
      for (let step = 1; step <= 10; step += 1) {
        await delay(200);
        onProgress(step / 10);
      }
    },
  };
}
