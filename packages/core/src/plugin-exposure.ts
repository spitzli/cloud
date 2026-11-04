import {
  PluginError,
  type PluginManifest,
} from "@spitzli/plugin-sdk/contracts";
export type ExposedPlugin = {
  state: string;
  runtime: string;
  manifest: PluginManifest;
};
const active = (p: ExposedPlugin) =>
  p.state === "enabled" && p.runtime === "ready";
export function pluginTools(plugins: ExposedPlugin[], canWrite: boolean) {
  const names = new Set<string>();
  return plugins.filter(active).flatMap((p) =>
    (p.manifest.mcp ?? []).flatMap((tool) => {
      const action = p.manifest.actions.find((a) => a.name === tool.action);
      if (!action || (!canWrite && action.intent === "write")) return [];
      const name = `plugin__${p.manifest.name.replaceAll("-", "_")}__${tool.name}`;
      if (names.has(name))
        throw new PluginError("conflict", "Plugin MCP tool collision");
      names.add(name);
      return [
        {
          ...tool,
          name,
          pluginID: p.manifest.name,
          input: action.input,
          intent: action.intent,
          destructive: action.destructive ?? false,
        },
      ];
    }),
  );
}
export function pluginRoute(
  plugin: ExposedPlugin,
  method: string,
  path: string,
) {
  if (!active(plugin)) return null;
  const route = plugin.manifest.api?.find(
    (r) => r.method === method && r.path === path,
  );
  return route
    ? (plugin.manifest.actions.find((a) => a.name === route.action) ?? null)
    : null;
}
export type ExposureInput = {
  pluginID: string;
  projectID: string;
  action: string;
  input: unknown;
  canWrite: boolean;
  idempotencyKey: string;
};
export async function invokePluginExposure(
  deps: {
    load(id: string): Promise<ExposedPlugin | null>;
    invoke(input: ExposureInput): Promise<unknown>;
  },
  input: ExposureInput,
) {
  const plugin = await deps.load(input.pluginID);
  if (!plugin || !active(plugin))
    throw new PluginError("not_found", "Plugin is unavailable");
  const action = plugin.manifest.actions.find((a) => a.name === input.action);
  if (!action) throw new PluginError("not_found", "Unknown plugin action");
  if (action.intent === "write" && !input.canWrite)
    throw new PluginError("permission_denied", "Write scope required");
  // The invoker authorizes the actual project and current grant. Discovery never grants authority.
  return deps.invoke(input);
}
