import { useCallback, useEffect, useState } from "react";
import { type MonitorInfo, listMonitors } from "../lib/ipc";

interface MonitorsState {
  monitors: MonitorInfo[];
  loading: boolean;
  failed: boolean;
}

export function useMonitors() {
  const [state, setState] = useState<MonitorsState>({ monitors: [], loading: true, failed: false });

  const refresh = useCallback(() => {
    setState((s) => ({ ...s, loading: true }));
    listMonitors()
      .then((monitors) => setState({ monitors, loading: false, failed: false }))
      .catch(() => setState({ monitors: [], loading: false, failed: true }));
  }, []);

  useEffect(refresh, [refresh]);
  return { ...state, refresh };
}
