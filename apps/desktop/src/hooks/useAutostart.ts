import { useCallback, useEffect, useState } from "react";
import { isAutostartEnabled, setAutostart } from "../lib/system";

export function useAutostart() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    isAutostartEnabled()
      .then(setEnabled)
      .catch(() => setEnabled(false));
  }, []);

  const change = useCallback(async (next: boolean) => {
    setEnabled(next);
    setFailed(false);
    try {
      await setAutostart(next);
    } catch {
      setEnabled(!next);
      setFailed(true);
    }
  }, []);

  return { enabled, failed, change };
}
