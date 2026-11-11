import type { Config } from "../lib/ipc";
import { useStore } from "../lib/store";
import { configStore } from "../state/config";

/** For components rendered only once the config has loaded. */
export function useConfig(): Config {
  const { config } = useStore(configStore);
  if (!config) throw new Error("config used before it was loaded");
  return config;
}
