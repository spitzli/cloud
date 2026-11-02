import { readAuthProtocolSelection } from "@webdock/auth-configuration/protocols";
import Elysia from "elysia";
import collectionRoutes from "../routes/operator/registry/[collection]/index";
import recordRoutes from "../routes/operator/registry/[collection]/[id]";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import { createPortalSession } from "../modules/portal/session";
import { setRegistryIdentity } from "../modules/registry/context";
export async function registryProtocol(config: ApiConfig): Promise<Mount> {
  const sso = readAuthProtocolSelection(process.env).oauthProvider === "better-auth"
    ? createPortalSession()
    : undefined;
  return {
    host: config.apiOrigin,
    category: "operator",
    app: new Elysia({ prefix: "/v1/operator/registry" }).group("/:collection", app => app.use(collectionRoutes))
      .group("/:collection/:id", app => app.use(recordRoutes)),
    guard: async (request) => {
      if (!sso) return new Response(null, { status: 404 });
      try {
        const identity = await sso.getDelegatedSession(request.headers);
        if (!identity?.sessionID) return new Response(null, { status: 401 });
        if (identity.role !== "operator")
          return new Response(null, { status: 403 });
        setRegistryIdentity(request, {
          subject: identity.subject,
          sessionID: identity.sessionID,
        });
      } catch {
        return new Response(null, { status: 503 });
      }
    },
  };
}
