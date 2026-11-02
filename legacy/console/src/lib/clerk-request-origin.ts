import { NextRequest } from "next/server";

/** Both Clerk and Next's rewrite handling must see the configured public origin. */
export function canonicalClerkRequest(request: NextRequest, canonicalOrigin: string, local: {allowLocalHTTP?: boolean; environment?: string} = {}) {
  const source = new URL(request.url);
  if (source.pathname.startsWith("//")) throw new Error("Ambiguous Console request path.");
  const origin = new URL(canonicalOrigin);
  const loopback = local.allowLocalHTTP === true && (local.environment ?? process.env.NODE_ENV) !== "production" && origin.protocol === "http:" && ["localhost","127.0.0.1","[::1]"].includes(origin.hostname);
  if ((origin.protocol !== "https:" && !loopback) || origin.username || origin.password || origin.origin !== canonicalOrigin)
    throw new Error("Clerk production requires a configured HTTPS Console origin.");
  source.protocol = origin.protocol;
  source.host = origin.host;
  const headers = new Headers(request.headers);
  headers.set("x-forwarded-host", origin.host);
  headers.set("x-forwarded-proto", origin.protocol.slice(0,-1));
  // nextUrl mutation alone leaves Request.url unchanged, causing Clerk's rewrite to loop.
  return new NextRequest(source, {
    method: request.method, headers, signal: request.signal, redirect: request.redirect,
    body: ["GET","HEAD"].includes(request.method) ? undefined : request.body,
  });
}

/** Clerk decorates next() as a same-URL rewrite; resume locally behind a reverse proxy. */
export function resumeClerkRequest(url: string, response: Response) {
  if (response.headers.get("x-middleware-rewrite") === url) {
    response.headers.delete("x-middleware-rewrite");
    response.headers.set("x-middleware-next", "1");
  }
  return response;
}
