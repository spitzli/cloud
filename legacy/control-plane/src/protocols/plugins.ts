import { readAuthProtocolSelection } from "@webdock/auth-configuration/protocols";
import Elysia from "elysia";
import { withDatabaseScope } from "@webdock/database-runtime";
import { PluginError } from "@webdock/plugin-sdk/contracts";
import { pluginRoute } from "@webdock/core/plugin-exposure";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import { authorizeRegistry } from "../modules/mcp/mcp-auth";
import {
  getRuntimePlugin,
  pluginData,
  pluginHTTP,
  pluginApps,
} from "../modules/plugins/runtime";
import { invokeExposedPlugin } from "../modules/plugins/exposure";
import { readJSON, HTTPError } from "../plugins/body";
function failure(error: unknown) {
  const status =
    error instanceof HTTPError
      ? error.status
      : error instanceof PluginError
        ? ((
            {
              permission_denied: 403,
              not_found: 404,
              invalid_input: 400,
              conflict: 409,
            } as Record<string, number>
          )[error.code] ?? 503)
        : 503;
  return Response.json(
    {
      error:
        error instanceof PluginError
          ? error.toJSON()
          : { code: "unavailable", message: "Plugin request failed" },
    },
    { status, headers: { "cache-control": "no-store" } },
  );
}
export function pluginProtocol(
  config: ApiConfig,
  authenticate: typeof authorizeRegistry = authorizeRegistry,
): Mount {
  const app = new Elysia()
    .all("/v1/plugin/data/*", ({ request }) => pluginData.handle(request), {
      parse: "none",
    })
    .put("/v1/plugin/data", ({ request }) => pluginData.handle(request), {
      parse: "none",
    })
    .post(
      "/v1/plugin/apps",
      async ({ request }) => {
        try {
          return Response.json(
            await pluginApps.request(request, await readJSON(request, 32768)),
            { headers: { "cache-control": "no-store" } },
          );
        } catch (error) {
          return failure(error);
        }
      },
      { parse: "none" },
    )
    .post(
      "/v1/plugin/http",
      async ({ request }) => {
        try {
          return Response.json(
            await pluginHTTP.request(request, await readJSON(request, 1048576)),
          );
        } catch (e) {
          return failure(e);
        }
      },
      { parse: "none" },
    )
    .all(
      "/v1/plugins/:pluginID/projects/:projectID/api/*",
      async ({ request, params }) => {
        try {
          if (
            readAuthProtocolSelection(process.env).oauthProvider === "disabled"
          )
            return new Response(null, { status: 404 });
          const access = await authenticate(request, request.method !== "GET");
          if (access instanceof Response) return access;
          return await withDatabaseScope(
            { kind: "session", ...access.actor.identity },
            async () => {
              const plugin = await getRuntimePlugin(params.pluginID);
              const action = pluginRoute(
                plugin,
                request.method,
                "/" + params["*"],
              );
              if (!action) return new Response(null, { status: 404 });
              let input: unknown;
              if (request.method === "GET") {
                const q = new URL(request.url).searchParams;
                for (const key of q.keys())
                  if (q.getAll(key).length !== 1)
                    throw new PluginError(
                      "invalid_input",
                      "Duplicate query parameter",
                    );
                const raw = q.get("input");
                if ([...q.keys()].some((k) => k !== "input"))
                  throw new PluginError(
                    "invalid_input",
                    "Use the input JSON query parameter",
                  );
                if (raw && raw.length > 32768)
                  throw new HTTPError(413, "Input too large");
                input = raw ? JSON.parse(raw) : {};
              } else input = await readJSON(request, 32768);
              const key =
                request.headers.get("idempotency-key") ??
                (action.intent === "read" ? crypto.randomUUID() : "");
              if (!key)
                throw new PluginError(
                  "invalid_input",
                  "Idempotency-Key required",
                );
              return Response.json(
                await invokeExposedPlugin({
                  pluginID: params.pluginID,
                  projectID: params.projectID,
                  action: action.name,
                  input,
                  canWrite: access.canWrite,
                  idempotencyKey: key,
                }),
              );
            },
          );
        } catch (e) {
          return failure(e);
        }
      },
      { parse: "none" },
    );
  return { app, host: config.apiOrigin, category: "agent" };
}
