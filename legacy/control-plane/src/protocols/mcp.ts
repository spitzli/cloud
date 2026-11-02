import Elysia from "elysia";
import { HTTPError } from "../plugins/body";
import { withDatabaseScope } from "@webdock/database-runtime";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
import { GET, POST } from "./mcp-handler";
import {
  authenticateMCP,
  challenge,
  mcpResource,
  mcpIssuer,
} from "../modules/mcp/mcp-auth";
import { handleRegistryRequest } from "../modules/mcp/registry-api";


function pathSegments(request: Request, prefix: string) {
  const path = new URL(request.url).pathname
    .slice(prefix.length)
    .replace(/^\//, "");
  if (!path) return [];
  try {
    return path.split("/").map((value) => {
      const segment = decodeURIComponent(value);
      if (!segment || segment.includes("/") || segment.includes("\\"))
        throw Error();
      return segment;
    });
  } catch {
    throw new HTTPError(400, "Invalid API path.");
  }
}
export function mcpProtocol(config: ApiConfig): Mount[] {
  const app = new Elysia()
    .get("/api/mcp", ({ request }) => GET(request))
    .post("/api/mcp", ({ request }) => POST(request), { parse: "none" })
    .get("/.well-known/oauth-protected-resource/api/mcp", () => ({
      resource: mcpResource(),
      resource_name: "Webdock Studio",
      authorization_servers: [mcpIssuer()],
      scopes_supported: [
        "webdock:read",
        "webdock:write",
      ],
      bearer_methods_supported: ["header"],
    }));
  for (const method of ["GET", "POST", "PUT", "PATCH", "DELETE"]) {
    for (const path of ["/api/registry", "/api/registry/*"])
      app.route(
        method,
        path,
        async ({ request }) => {
          const identity = await authenticateMCP(request);
          if (!identity) return challenge(401);
          return withDatabaseScope(
            {
              kind: "session",
              subject: identity.subject,
              sessionID: identity.sessionID,
            },
            () =>
              handleRegistryRequest(
                request,
                pathSegments(request, "/api/registry"),
                { authenticate: async () => identity },
              ),
          );
        },
        { parse: "none" },
      );

  }
  return [{ app, host: config.studioOrigin, category: "mcp" }];
}
