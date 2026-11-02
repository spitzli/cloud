import { consoleBrowserRequest } from "../modules/portal/console-browser";
import { ConsoleAuthError } from "../modules/auth/clerk-identity";
import { consoleAuthConfiguration } from "../modules/auth/console-configuration";
import { browserAuthProvider } from "../modules/auth/clerk-runtime";
import { handleClerkStudioRequest } from "../modules/portal/studio-api";
import { requireConsoleBrowserPrincipal } from "../modules/auth/clerk-provider";
import { withConsolePrincipal } from "../modules/auth/principal";
import { tenantPreview } from "../modules/portal/preview";
import Elysia from "elysia";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import { createPortalSession } from "../modules/portal/session";
import { portalRequest } from "../modules/portal/transport";
import {
  handleNativeStudioRequest,
  handleConsoleRequest,
  handleStudioRequest,
} from "../modules/portal/studio-api";
export function portalProtocols(config: ApiConfig): Mount[] {
  const browserCall = (request: Request) =>
    consoleBrowserRequest(request, config.studioOrigin, async () => {
      try {
        return await withConsolePrincipal(
          await requireConsoleBrowserPrincipal(request),
          () => handleNativeStudioRequest(request),
        );
      } catch (error) {
        return Response.json(
          {
            error: {
              kind: "AccessError",
              message:
                error instanceof ConsoleAuthError
                  ? error.message
                  : "Console is temporarily unavailable.",
            },
          },
          {
            status: error instanceof ConsoleAuthError ? error.status : 503,
            headers: { "cache-control": "no-store" },
          },
        );
      }
    });
  if (browserAuthProvider === "clerk") {
    const call = (request: Request) => handleClerkStudioRequest(request);
    return [
      {
        host: config.apiOrigin,
        category: "delegated",
        app: new Elysia()
          .post("/v1/studio", ({ request }) => call(request), {
            parse: "none",
          })
          .post("/v1/console", ({ request }) => handleConsoleRequest(request), {
            parse: "none",
          }),
      },
      {
        host: config.studioOrigin,
        category: "auth_protocol",
        app: new Elysia()
          .get("/api/sso/login", ({ request }) => {
            const source = new URL(request.url),
              target = new URL("/sign-in", config.studioOrigin),
              candidate = source.searchParams.get("returnTo");
            if (
              candidate?.startsWith("/") &&
              !candidate.startsWith("//") &&
              !/[\\\x00-\x1f]/.test(candidate)
            )
              target.searchParams.set("returnTo", candidate);
            return Response.redirect(target, 303);
          })
          .post("/api/portal/studio", ({ request }) => call(request), {
            parse: "none",
          }),
      },
    ];
  }
  if (
    process.env.WEBDOCK_AUTH_ISSUER !== config.authOrigin + "/api/auth" ||
    process.env.WEBDOCK_SSO_APP_ORIGIN !== config.studioOrigin
  )
    throw Error("Portal canonical origins mismatch");
  const session = createPortalSession();
  const clientID = process.env.WEBDOCK_SSO_CLIENT_ID,
    clientSecret = process.env.WEBDOCK_SSO_CLIENT_SECRET;
  if (
    !clientID ||
    !clientSecret ||
    clientID !== process.env.WEBDOCK_STUDIO_CLIENT_ID
  )
    throw Error("Studio client identity mismatch");
  const call = (request: Request) =>
    portalRequest(
      request,
      { origin: config.studioOrigin, clientID, clientSecret },
      (headers) => session.getDelegatedSession(headers),
      (internal) =>
        handleStudioRequest(internal, { browserHeaders: request.headers }),
    );
  const preview = (request: Request, action: string) =>
    tenantPreview(
      request,
      action,
      config.studioOrigin,
      async <T>(operation: string, args: unknown[] = []) => {
        const headers = new Headers(request.headers);
        headers.set("content-type", "application/json");
        headers.delete("content-length");
        const internal = new Request(request.url, {
          method: "POST",
          headers,
          body: JSON.stringify({ operation, args }),
        });
        const response = consoleAuthConfiguration().enabled
          ? await withConsolePrincipal(
              await requireConsoleBrowserPrincipal(request),
              () => handleNativeStudioRequest(internal),
            )
          : await call(internal);
        if (!response.ok) throw Error("Preview unavailable");
        return ((await response.json()) as { data: T }).data;
      },
    );
  return [
    {
      host: config.apiOrigin,
      category: "delegated",
      app: new Elysia()
        .post("/v1/studio", ({ request }) => call(request), {
          parse: "none",
        })
        .post("/v1/console", ({ request }) => handleConsoleRequest(request), {
          parse: "none",
        }),
    },
    {
      host: config.studioOrigin,
      category: "auth_protocol",
      app: new Elysia()
        .get("/api/sso/login", ({ request }) => session.login(request))
        .get("/api/sso/callback", ({ request }) => session.callback(request))
        .post("/api/sso/logout", ({ request }) => session.logout(request), {
          parse: "none",
        })
        .post(
          "/api/users/logout",
          async ({ request }) => {
            const response = await session.logout(request);
            if (response.status !== 303) return response;
            response.headers.delete("Location");
            return Response.json(
              { message: "Signed out." },
              { headers: response.headers },
            );
          },
          { parse: "none" },
        )
        .post(
          "/api/tenant-preview/:action",
          ({ request, params }) => preview(request, params.action),
          { parse: "none" },
        )
        .post(
          "/api/users/refresh-token",
          ({ request }) => session.refresh(request),
          { parse: "none" },
        )
        .post(
          "/api/portal/studio",
          ({ request }) =>
            consoleAuthConfiguration().enabled
              ? browserCall(request)
              : call(request),
          {
            parse: "none",
          },
        ),
    },
  ];
}
