import { ModuleError } from "@webdock/module-sdk";
import type { SessionReference } from "@webdock/core/identity";
type Identity = {
  headers: Headers;
  reference: SessionReference;
  accessToken?: string;
};
/** A cookie is only a transport for the signed token; the Clerk bridge still verifies it live. */
export function clerkIntegrationProof(headers: Headers): Headers | null {
  const authorization = headers.get("authorization");
  if (authorization !== null) {
    if (!/^Bearer [A-Za-z0-9_.-]{1,16384}$/i.test(authorization)) return null;
    return new Headers({ authorization });
  }
  const cookies = (headers.get("cookie") ?? "")
    .split(";")
    .map((part) => part.trim())
    .filter((part) => part.startsWith("__session="));
  if (cookies.length !== 1) return null;
  const token = cookies[0].slice("__session=".length);
  if (
    !/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(token) ||
    token.length > 16384
  )
    return null;
  return new Headers({ authorization: "Bearer " + token });
}
export function createIntegrationBrowserRunner(dependencies: {
  provider: "clerk" | "better-auth";
  origin: string;
  refreshCallback?(request: Request): Response | null;
  requireAdmission?(intent: "access" | "provision"): void;
  clerk(headers: Headers): Promise<Identity | null>;
  legacy(headers: Headers): Promise<Identity | null>;
  withIdentity(
    identity: Identity,
    browser: Headers,
    kind: "clerk" | "legacy",
    run: () => Promise<Response>,
  ): Promise<Response>;
}) {
  return async (
    request: Request,
    intent: "access" | "provision",
    handler: (request: Request) => Promise<Response>,
    options: { refreshed?: boolean } = {},
  ): Promise<Response> => {
    const reply = (status: number) =>
      new Response(null, { status, headers: { "cache-control": "no-store" } });
    const unauthenticated = () =>
      dependencies.provider === "clerk" &&
      request.method === "GET" &&
      !options.refreshed
        ? (dependencies.refreshCallback?.(request) ?? reply(401))
        : reply(401);
    try {
      dependencies.requireAdmission?.(intent);
      // Cookie authentication requires its own CSRF check, including disconnect, before provider I/O.
      if (
        request.method === "POST" &&
        request.headers.get("origin") !== dependencies.origin
      )
        return reply(403);
      const proof =
        dependencies.provider === "clerk"
          ? clerkIntegrationProof(request.headers)
          : request.headers;
      if (!proof) return unauthenticated();
      const identity =
        dependencies.provider === "clerk"
          ? await dependencies.clerk(proof)
          : await dependencies.legacy(proof);
      if (!identity) return unauthenticated();
      return await dependencies.withIdentity(
        identity,
        request.headers,
        dependencies.provider === "clerk" ? "clerk" : "legacy",
        () => handler(request),
      );
    } catch (error) {
      return reply(error instanceof ModuleError ? error.status : 503);
    }
  };
}
