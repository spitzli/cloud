import ipaddr from "ipaddr.js";
import { createHash, timingSafeEqual } from "node:crypto";
export type IngressPolicy = {
  origins: string[];
  trustedProxies: string[];
  secret?: string;
  production: boolean;
};
const removed = [
  "forwarded",
  "x-forwarded-host",
  "x-forwarded-proto",
  "x-forwarded-for",
  "x-vercel-forwarded-for",
  "x-webdock-ingress-token",
  "x-webdock-client-ip",
];
export function normalizeIngress(
  request: Request,
  peer: string | undefined,
  policy: IngressPolicy,
): Request | Response {
  const reject = () => new Response(null, { status: 400 });
  if (!peer) return reject();
  try {
    const address = ipaddr.process(peer);
    const trusted = policy.trustedProxies.some((cidr) => {
      const [network, bits] = ipaddr.parseCIDR(cidr);
      return address.kind() === network.kind() && address.match(network, bits);
    });
    const headers = new Headers(request.headers),
      url = new URL(request.url);
    const forwarded =
      headers.has("x-forwarded-host") ||
      headers.has("x-forwarded-proto") ||
      headers.has("x-forwarded-for");
    let client = address.toString();
    if (forwarded) {
      const supplied = headers.get("x-webdock-ingress-token") || "";
      if (
        !trusted ||
        !policy.secret ||
        !timingSafeEqual(
          createHash("sha256").update(supplied).digest(),
          createHash("sha256").update(policy.secret).digest(),
        )
      )
        return reject();
      const origin = new URL(
        (headers.get("x-forwarded-proto") || "") +
          "://" +
          (headers.get("x-forwarded-host") || ""),
      );
      if (
        origin.username ||
        origin.password ||
        origin.pathname !== "/" ||
        origin.search ||
        origin.hash ||
        !policy.origins.includes(origin.origin)
      )
        return reject();
      const ip = headers.get("x-forwarded-for") || "";
      if (!ipaddr.isValid(ip)) return reject();
      client = ipaddr.process(ip).toString();
      url.protocol = origin.protocol;
      url.host = origin.host;
      url.port = origin.port;
    } else if (policy.production) {
      // Health probes stay private and contain no customer data.
      if (
        address.range() !== "loopback" ||
        !["/healthz", "/readyz"].includes(url.pathname)
      )
        return reject();
      const canonical = new URL(policy.origins[0]);
      url.protocol = canonical.protocol;
      url.host = canonical.host;
      url.port = canonical.port;
    }
    for (const name of removed) headers.delete(name);
    headers.set("x-webdock-client-ip", client);
    headers.set("host", url.host);
    return new Request(url, {
      ...request,
      method: request.method,
      headers,
      body: ["GET", "HEAD"].includes(request.method) ? undefined : request.body,
      signal: request.signal,
      redirect: request.redirect,
    });
  } catch {
    return reject();
  }
}
