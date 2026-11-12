import { useCallback, useEffect, useRef, useState } from "react";
import { type Thumbnail, getMonitorThumbnail } from "../lib/ipc";

const THUMBNAIL_MAX_WIDTH = 960;

interface ThumbnailState {
  thumbnail: Thumbnail | null;
  loading: boolean;
  failed: boolean;
}

export function useThumbnail(monitorId: string | null) {
  const [state, setState] = useState<ThumbnailState>({ thumbnail: null, loading: true, failed: false });
  const [nonce, setNonce] = useState(0);

  const shownMonitor = useRef(monitorId);

  useEffect(() => {
    let cancelled = false;
    // A manual refresh keeps the old picture until the new one arrives; a different monitor may have another shape.
    const monitorChanged = shownMonitor.current !== monitorId;
    shownMonitor.current = monitorId;
    setState((s) => ({ thumbnail: monitorChanged ? null : s.thumbnail, loading: true, failed: false }));
    getMonitorThumbnail(monitorId, THUMBNAIL_MAX_WIDTH)
      .then((thumbnail) => {
        if (!cancelled) setState({ thumbnail, loading: false, failed: false });
      })
      .catch(() => {
        if (!cancelled) setState({ thumbnail: null, loading: false, failed: true });
      });
    return () => {
      cancelled = true;
    };
  }, [monitorId, nonce]);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { ...state, refresh };
}
