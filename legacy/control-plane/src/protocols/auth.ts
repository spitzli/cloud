import Elysia from "elysia";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
export function authProtocol(
  config: ApiConfig,
  handler: (request: Request) => Promise<Response>,
): Mount {
  return {
    host: config.authOrigin,
    category: "auth_protocol",
    app: new Elysia()
      .all("/api/auth/*", ({ request }) => handler(request), { parse: "none" })
      .get("/.well-known/openid-configuration", ({ request }) =>
        handler(
          new Request(
            new URL(
              "/api/auth/.well-known/openid-configuration",
              config.authOrigin,
            ),
            request,
          ),
        ),
      )
      .get("/.well-known/oauth-authorization-server/api/auth", ({ request }) =>
        handler(request),
      ),
  };
}
