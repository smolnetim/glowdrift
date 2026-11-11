import { useCallback, useEffect, useState } from "react";
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

  useEffect(() => {
    let cancelled = false;
    setState((s) => ({ ...s, loading: true }));
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
