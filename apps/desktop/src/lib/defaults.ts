import type { Config } from "./ipc";

export const DEFAULT_CONFIG: Config = {
  schemaVersion: 1,
  onboardingComplete: false,
  monitorId: null,
  region: { x: 0, y: 0, width: 1, height: 1 },
  captureFps: 20,
  mode: "average",
  smoothingMs: 500,
  brightness: 1,
  saturation: 1,
  outputs: { icue: { enabled: true } },
  startMinimized: true,
  autoUpdate: true,
};

export const CAPTURE_FPS_MIN = 1;
export const CAPTURE_FPS_MAX = 60;
export const SMOOTHING_MS_MAX = 5000;
export const SATURATION_MAX = 2;
