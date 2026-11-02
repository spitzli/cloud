import type { AnyElysia } from "elysia";
export type RouteCategory =
  | "public"
  | "auth_protocol"
  | "tenant"
  | "operator"
  | "delegated"
  | "agent"
  | "mcp"
  | "signed_webhook"
  | "mail_key";
export type Mount = {
  app: AnyElysia;
  host: string;
  aliases?: string[];
  category: RouteCategory;
  guard?: (request: Request) => Response | void | Promise<Response | void>;
};
/** Validate before .use() discards duplicate routes. One path has one canonical host and optional explicit aliases. */
export function validateMounts(mounts: Mount[], origins: string[]): void {
  const seen = new Set<string>();
  for (const mount of mounts) {
    if (
      [mount.host, ...(mount.aliases ?? [])].some(
        (host) => !origins.includes(host),
      )
    )
      throw Error("Unknown mount host");
    if (
      ![
        "public",
        "auth_protocol",
        "tenant",
        "operator",
        "delegated",
        "agent",
        "mcp",
        "signed_webhook",
        "mail_key",
      ].includes(mount.category)
    )
      throw Error("Missing route category");
    if (["tenant", "operator"].includes(mount.category) && !mount.guard)
      throw Error("A static guard is required");
    for (const route of mount.app.routes) {
      for (const method of route.method === "ALL"
        ? [
            "GET",
            "POST",
            "PUT",
            "PATCH",
            "DELETE",
            "HEAD",
            "OPTIONS",
            "CONNECT",
            "TRACE",
          ]
        : [route.method]) {
        const key =
          method +
          " " +
          ("/" + route.path.split("/").filter(Boolean).join("/")).replace(
            /:[^/]+/g,
            ":param",
          );
        if (seen.has(key)) throw Error(`Route collision: ${key}`);
        seen.add(key);
      }
    }
  }
}
