import { consoleAuthConfiguration } from "./modules/auth/console-configuration";
import type { ApiConfig } from "./config";

// Provider registrations and MCP audiences keep their existing protocol identity.
// Only browser entry points move; never redirect bearer APIs or signed webhooks.
const browserEntrypoints = new Set([
  "/api/sso/login", "/api/sso/callback", "/api/github/callback",
  "/api/vercel/callback", "/api/hosting/git/callback", "/api/hosting/vercel/callback",
]);

/** Call only after normalizeIngress has verified the public origin and client IP. */
export async function handleConsoleOrigin(
  request: Request,
  { studioOrigin, consoleOrigin }: Pick<ApiConfig, "studioOrigin" | "consoleOrigin">,
  handle: (request: Request) => Response | Promise<Response>,
): Promise<Response> {
  if (!consoleOrigin) return handle(request);
  const url = new URL(request.url);
  if (url.origin === studioOrigin && request.method === "GET" && browserEntrypoints.has(url.pathname)) {
    return new Response(null, { status: 307, headers: {
      Location: consoleOrigin + url.pathname + url.search,
      "Cache-Control": "no-store", "Referrer-Policy": "no-referrer",
    } });
  }
  if (url.origin !== consoleOrigin) return handle(request);
  const headers = new Headers(request.headers);
  const origin = headers.get("origin");
  if (origin && origin !== consoleOrigin)
    return new Response("Forbidden origin", {status: 403, headers: {"Cache-Control": "no-store"}});
  // Production ingress sends /api/* directly here, bypassing the Next.js proxy.
  if (consoleAuthConfiguration().enabled &&
      ["GET", "HEAD"].includes(request.method) && url.pathname === "/api/sso/login")
    return new Response(null, {status: 307, headers: {
      Location: consoleOrigin + "/login", "Cache-Control": "no-store", "Referrer-Policy": "no-referrer",
    }});
  if (origin) headers.set("origin", studioOrigin);
  const protocol = new URL(studioOrigin);
  url.protocol = protocol.protocol;
  url.host = protocol.host;
  url.port = protocol.port;
  headers.set("host", protocol.host);
  const response = await handle(new Request(url, {
    method: request.method, headers,
    body: ["GET", "HEAD"].includes(request.method) ? undefined : request.body,
    signal: request.signal, redirect: request.redirect,
  }));
  const location = response.headers.get("location");
  if (!location || new URL(location, studioOrigin).origin !== studioOrigin) return response;
  const destination = new URL(location, studioOrigin);
  const outgoing = new Headers(response.headers);
  outgoing.set("location", consoleOrigin + destination.pathname + destination.search + destination.hash);
  return new Response(response.body, {status: response.status, statusText: response.statusText, headers: outgoing});
}
