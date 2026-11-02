import type { DatabaseLaunch } from "@webdock/database-contracts";

type LaunchResult = { launch?: Pick<DatabaseLaunch, "code" | "gatewayOrigin">; error?: string };

/** The portal owns session validation and delegation; the browser sends only the requested database. */
export async function launchDatabaseWorkspace(bindingID: string, managed = false): Promise<LaunchResult> {
  const unavailable = { error: "Database access is unavailable." };
  try {
    const response = await fetch("/api/portal/studio", {
      method: "POST", credentials: "same-origin", redirect: "error", cache: "no-store", signal: AbortSignal.timeout(45_000),
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ operation: "database.command", args: [managed ? { action: "openManaged", databaseID: bindingID } : { action: "open", bindingID }] }),
    });
    const body = await response.json() as { data?: { code?: unknown; gatewayOrigin?: unknown }; error?: { kind?: unknown; message?: unknown } };
    if (!response.ok || body.error) {
      if (!response.ok && body.error?.kind === "HostingError" && typeof body.error.message === "string" && body.error.message.length <= 300) {
        return { error: body.error.message || unavailable.error };
      }
      return unavailable;
    }
    const { code, gatewayOrigin } = body.data ?? {};
    if (typeof code !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(code) || typeof gatewayOrigin !== "string") return unavailable;
    const url = new URL(gatewayOrigin);
    const local = url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if ((!local && url.protocol !== "https:") || gatewayOrigin !== url.origin) return unavailable;
    return { launch: { code, gatewayOrigin } };
  } catch { return unavailable; }
}
