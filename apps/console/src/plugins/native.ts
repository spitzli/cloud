import { useQuery } from "@tanstack/react-query";
import { pluginUILoader } from "./registry";

/** The plugin's own React UI, if it ships one. Loaded only when an installed plugin is shown. */
export function useNativeUI(plugin: string | undefined) {
  const load = plugin ? pluginUILoader(plugin) : undefined;
  return useQuery({
    queryKey: ["native-ui", plugin],
    queryFn: async () => (load ? (await load()).default : null),
    staleTime: Infinity,
    enabled: plugin !== undefined,
  });
}
