import { readCachedView } from "../cache";
import { authDatabase } from "../db/runtime";
import Elysia from "elysia";
import { verifyOAuthQueryParams } from "@better-auth/oauth-provider";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import { auth } from "../modules/auth/auth";
import { publicPlatformSettings } from "../modules/platform/platform-settings";
import { handleNativeStudioRequest } from "../modules/portal/studio-api";
import { readJSON } from "../plugins/body";
export function authUIProtocol(config: ApiConfig): Mount {
  return {
    host: config.apiOrigin,
    category: "auth_protocol",
    app: new Elysia()
      .post(
        "/v1/auth/studio",
        ({ request }) => handleNativeStudioRequest(request),
        { parse: "none" },
      )
      .get("/v1/auth/session", async ({ request }) => {
        const session = await auth.api.getSession({ headers: request.headers });
        if (!session) return Response.json({ data: null });
        const {
          id,
          name,
          email,
          emailVerified,
          role,
          banned,
          twoFactorEnabled,
          mustChangePassword,
        } = session.user;
        return Response.json({
          data: {
            user: {
              id,
              name,
              email,
              emailVerified,
              role,
              banned,
              twoFactorEnabled,
              mustChangePassword,
            },
            session: {
              id: session.session.id,
              userId: session.user.id,
              expiresAt: session.session.expiresAt,
            },
          },
        });
      })
      .get("/v1/public/branding", async () => ({
        data: await readCachedView(
          {
            kind: "branding",
            scope: { public: true },
            query: { origin: config.authOrigin },
          },
          async () =>
            (
              await authDatabase.query(
                "SELECT webdock_cache.version($1::text[]) AS version",
                [["webdock_auth.platform_settings"]],
              )
            ).rows[0].version,
          publicPlatformSettings,
        ).catch(() => publicPlatformSettings()),
      }))
      .post(
        "/v1/auth/consent",
        async ({ request }) => {
          if (request.headers.get("origin") !== config.authOrigin)
            return new Response(null, { status: 403 });
          const body = (await readJSON(request, 8192)) as { query?: unknown };
          if (typeof body?.query !== "string")
            return { data: { valid: false } };
          const params = new URLSearchParams(body.query),
            { secret } = await auth.$context;
          if (!(await verifyOAuthQueryParams(params.toString(), secret)))
            return { data: { valid: false } };
          const session = await auth.api.getSession({
            headers: request.headers,
          });
          if (!session) return { data: { valid: true, session: false } };
          try {
            const clientID = params.get("client_id");
            if (!clientID) return { data: { valid: false } };
            const client = await auth.api.getOAuthClientPublic({
              headers: request.headers,
              query: { client_id: clientID },
            });
            const raw = params.get("claims"),
              claims = raw
                ? (JSON.parse(raw) as { userinfo?: Record<string, unknown> })
                : undefined;
            return {
              data: {
                valid: true,
                session: true,
                name: client.client_name || null,
                scopes: (params.get("scope") || "").split(" ").filter(Boolean),
                claims: Object.keys(claims?.userinfo || {}),
              },
            };
          } catch {
            return { data: { valid: false } };
          }
        },
        { parse: "none" },
      ),
  };
}
