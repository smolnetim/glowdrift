import type { MonitorInfo } from "../lib/ipc";
import { useConfig } from "./useConfig";
import { useMonitors } from "./useMonitors";
import { useThumbnail } from "./useThumbnail";

const FALLBACK_ASPECT = 16 / 9;
/** Thumbnails smaller than this are placeholders, not a usable picture. */
const MIN_USABLE_THUMBNAIL = 16;

/** The chosen display and one picture of it, shared by every preview on a page. */
export interface MonitorPreview {
  monitors: MonitorInfo[];
  monitorsLoading: boolean;
  monitorsFailed: boolean;
  refreshMonitors: () => void;
  /** The display being captured; undefined while unknown or no longer connected. */
  selected: MonitorInfo | undefined;
  aspectRatio: number;
  imageUrl: string | null;
  loading: boolean;
  failed: boolean;
  refreshPreview: () => void;
}

export function useMonitorPreview(): MonitorPreview {
  const config = useConfig();
  const { monitors, loading: monitorsLoading, failed: monitorsFailed, refresh: refreshMonitors } = useMonitors();
  const { thumbnail, loading, failed, refresh: refreshPreview } = useThumbnail(config.monitorId);

  const selected =
    monitors.find((m) => m.id === config.monitorId) ??
    (config.monitorId === null ? monitors.find((m) => m.primary) : undefined);

  const usable = thumbnail !== null && thumbnail.width >= MIN_USABLE_THUMBNAIL;
  const aspectRatio = selected
    ? selected.width / selected.height
    : thumbnail && usable
      ? thumbnail.width / thumbnail.height
      : FALLBACK_ASPECT;

  return {
    monitors,
    monitorsLoading,
    monitorsFailed,
    refreshMonitors,
    selected,
    aspectRatio,
    imageUrl: usable ? thumbnail.dataUrl : null,
    loading,
    failed,
    refreshPreview,
  };
}
