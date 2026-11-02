import Elysia from "elysia";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import { installationRegistry } from "../installation";
import { hostingCall } from "../modules/portal/hosting";
import { gitConnectionRoute } from "../modules/integrations/github/byok-routes";
import { githubSelect } from "../modules/integrations/github/routes";
import { POST as byokVercelConnect } from "./legacy-studio/hosting/vercel/connect";
import { GET as byokVercelCallback } from "./legacy-studio/hosting/vercel/callback";
import { vercelRegionEvent } from "../modules/integrations/vercel/events";
import {
  vercelConnect,
  vercelCallback,
  vercelDisconnect,
} from "../modules/integrations/vercel/vercel-routes";
import { createIntegrationBrowserRuntime } from "./integration-browser-runtime";
import { configureProductIntegrationCallbacks } from "./integration-resume";

export function integrationProtocols(config: ApiConfig): Mount[] {
  const git = (step: "connect" | "callback" | "select") => (request: Request) =>
    gitConnectionRoute(request, step, {
      origin: config.studioOrigin,
      call: async (command) =>
        (await hostingCall(command)) as Record<string, unknown>,
    });
  const { run } = createIntegrationBrowserRuntime(config, (intent) =>
    installationRegistry().require("hosting", intent),
  );
  const callbacks = {
    "/api/vercel/callback": vercelCallback,
    "/api/hosting/git/callback": git("callback"),
    "/api/hosting/vercel/callback": byokVercelCallback,
  };
  configureProductIntegrationCallbacks(
    config.studioOrigin,
    Object.fromEntries(
      Object.entries(callbacks).map(([path, handler]) => [
        path,
        (request: Request, options: { refreshed: true }) =>
          run(request, "provision", handler, options),
      ]),
    ),
  );
  return [
    {
      host: config.studioOrigin,
      category: "auth_protocol",
      app: new Elysia()
        .post(
          "/api/github/select",
          ({ request }) => run(request, "provision", githubSelect),
          { parse: "none" },
        )
        .post(
          "/api/vercel/connect",
          ({ request }) => run(request, "provision", vercelConnect),
          { parse: "none" },
        )
        .get("/api/vercel/callback", ({ request }) =>
          run(request, "provision", vercelCallback),
        )
        .post(
          "/api/vercel/disconnect",
          ({ request }) => run(request, "access", vercelDisconnect),
          { parse: "none" },
        )
        .post(
          "/api/hosting/git/connect",
          ({ request }) => run(request, "provision", git("connect")),
          { parse: "none" },
        )
        .get("/api/hosting/git/callback", ({ request }) =>
          run(request, "provision", git("callback")),
        )
        .post(
          "/api/hosting/git/select",
          ({ request }) => run(request, "provision", git("select")),
          { parse: "none" },
        )
        .post(
          "/api/hosting/vercel/connect",
          ({ request }) => run(request, "provision", byokVercelConnect),
          { parse: "none" },
        )
        .get("/api/hosting/vercel/callback", ({ request }) =>
          run(request, "provision", byokVercelCallback),
        ),
    },
    {
      host: config.studioOrigin,
      category: "signed_webhook",
      app: new Elysia().post(
        "/api/vercel/events",
        ({ request }) => vercelRegionEvent(request),
        { parse: "none" },
      ),
    },
  ];
}
