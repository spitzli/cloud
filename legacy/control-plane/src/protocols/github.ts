import Elysia from "elysia";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import {
  githubConnect,
  githubCallback,
  githubDisconnect,
} from "../modules/integrations/github/routes";
import { createIntegrationBrowserRuntime } from "./integration-browser-runtime";
import { createIntegrationResumeHandler } from "./integration-resume";

export function githubProtocols(config: ApiConfig): Mount[] {
  const { run, handoff } = createIntegrationBrowserRuntime(config);
  return [
    {
      host: config.studioOrigin,
      category: "auth_protocol",
      app: new Elysia()
        .post(
          "/api/github/connect",
          ({ request }) => run(request, "provision", githubConnect),
          { parse: "none" },
        )
        .get("/api/github/callback", ({ request }) =>
          run(request, "provision", githubCallback),
        )
        .post(
          "/api/github/disconnect",
          ({ request }) => run(request, "access", githubDisconnect),
          { parse: "none" },
        ),
    },
    ...(handoff
      ? [
          {
            host: config.apiOrigin,
            category: "delegated" as const,
            app: new Elysia().post(
              "/v1/integrations/resume",
              ({ request }) =>
                createIntegrationResumeHandler(
                  config.studioOrigin,
                  handoff,
                  (request, options) =>
                    run(request, "provision", githubCallback, options),
                )(request),
              { parse: "none" },
            ),
          },
        ]
      : []),
  ];
}
