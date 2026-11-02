import { z } from "zod";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import {
  pluginTools,
  invokePluginExposure,
} from "@webdock/core/plugin-exposure";
import { PluginError } from "@webdock/plugin-sdk/contracts";
import {
  getRuntimePlugin,
  invokeRuntimePlugin,
  listRuntimePlugins,
} from "./runtime";
export const invokeExposedPlugin = (
  input: Parameters<typeof invokePluginExposure>[1],
) =>
  invokePluginExposure(
    {
      load: getRuntimePlugin,
      invoke: ({ canWrite, ...request }) =>
        invokeRuntimePlugin(request, { canWrite }),
    },
    input,
  );
/** MCP servers are built per request, so disabled tools disappear from the next tools/list. */
export async function registerPluginTools(
  server: McpServer,
  canWrite: boolean,
  discover: typeof listRuntimePlugins = listRuntimePlugins,
) {
  if (!process.env.WEBDOCK_PLUGIN_RUNTIME_FILE) return;
  for (const tool of pluginTools(await discover(), canWrite)) {
    const schema = tool.input
      ? z.fromJSONSchema(tool.input as any)
      : z.strictObject({});
    server.registerTool(
      tool.name,
      {
        description: tool.description,
        inputSchema: {
          projectID: z.string().regex(/^[0-9]{1,20}$/),
          input: schema,
          idempotencyKey:
            tool.intent === "write"
              ? z.string().min(1).max(128)
              : z.string().min(1).max(128).optional(),
        },
        annotations: {
          readOnlyHint: tool.intent === "read",
          destructiveHint: tool.destructive,
        },
      },
      async (args) => {
        try {
          const result = await invokeExposedPlugin({
            pluginID: tool.pluginID,
            projectID: args.projectID,
            action: tool.action,
            input: args.input,
            canWrite,
            idempotencyKey: args.idempotencyKey ?? crypto.randomUUID(),
          });
          return {
            content: [{ type: "text" as const, text: JSON.stringify(result) }],
          };
        } catch (error) {
          return {
            isError: true,
            content: [
              {
                type: "text" as const,
                text: JSON.stringify({
                  error:
                    error instanceof PluginError
                      ? error.toJSON()
                      : {
                          code: "unavailable",
                          message: "Plugin request failed",
                        },
                }),
              },
            ],
          };
        }
      },
    );
  }
}
