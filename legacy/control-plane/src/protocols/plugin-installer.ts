import Elysia from "elysia";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import { HTTPError, readJSON } from "../plugins/body";
import {
  installerAuthorized,
  PluginSourceError,
} from "../modules/plugins/sources-policy";
export function pluginInstallerProtocol(
  config: ApiConfig,
  dispatch: (action: string, input: unknown) => Promise<unknown> = async (
    action,
    input,
  ) =>
    (await import("../modules/plugins/sources")).pluginInstaller(action, input),
  actionsSource: (
    authorization: string | null,
    input: unknown,
  ) => Promise<unknown> = async (authorization, input) =>
    (await import("../modules/plugins/sources")).pluginActionsSource(
      authorization,
      input,
    ),
): Mount {
  const app = new Elysia().post(
    "/v1/plugin-installer/:action",
    async ({ request, params }) => {
      const headers = { "cache-control": "no-store" };
      if (
        params.action !== "actions-source" &&
        !installerAuthorized(request.headers.get("authorization"))
      )
        return Response.json(
          {
            error: {
              code: "unauthorized",
              message: "Installer authentication required.",
            },
          },
          { status: 401, headers },
        );
      if (
        ![
          "claim",
          "source",
          "complete",
          "lease",
          "actions-prepare",
          "actions-chunk",
          "actions-bind",
          "actions-source",
        ].includes(params.action)
      )
        return new Response(null, { status: 404, headers });
      try {
        if (params.action === "actions-source")
          return Response.json(
            await actionsSource(
              request.headers.get("authorization"),
              await readJSON(request, 4096),
            ),
            { headers },
          );
        return Response.json(
          await dispatch(params.action, await readJSON(request, 512 * 1024)),
          { headers },
        );
      } catch (error) {
        return Response.json(
          {
            error: {
              code:
                error instanceof PluginSourceError
                  ? error.code
                  : "invalid_request",
              message:
                error instanceof PluginSourceError
                  ? error.message
                  : "Plugin installer request failed.",
            },
          },
          {
            status:
              error instanceof PluginSourceError || error instanceof HTTPError
                ? error.status
                : 503,
            headers,
          },
        );
      }
    },
    { parse: "none" },
  );
  return { app, host: config.apiOrigin, category: "agent" };
}
