import type { HostingPorts } from "./ports";
import { HostingError } from "@spitzli/hosting-contracts";
export type Dependencies = {
  env: HostingPorts["env"];
};

export function createHostingHttp(dependencies: Dependencies) {
  function requireHostingEnvironment() {
    if (dependencies.env.NODE_ENV !== "production") return;
    // Vercel was explicitly selected by the owner. Enforce the actual function
    // region without claiming its global CDN/control plane is EU-only.
    const allowed =
      dependencies.env.VERCEL === "1"
        ? dependencies.env.SPITZLI_HOSTING_CONTROL_PLANE === "vercel-fra1" &&
          dependencies.env.VERCEL_REGION === "fra1"
        : dependencies.env.SPITZLI_HOSTING_EU_VERIFIED === "true";
    if (!allowed)
      throw new HostingError(
        503,
        "Hosting execution is not enabled for this runtime region.",
      );
  }
  async function readHostingJSON(request: Request, maximum = 65536) {
    if (
      request.headers.get("content-type")?.split(";")[0].trim() !==
      "application/json"
    )
      throw new HostingError(415, "Use application/json.");
    const reader = request.body?.getReader();
    if (!reader) throw new HostingError(400, "Invalid hosting input.");
    let size = 0;
    const chunks: Uint8Array[] = [];
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > maximum) {
          await reader.cancel();
          throw new HostingError(413, "Hosting request is too large.");
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
    try {
      return JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown;
    } catch {
      throw new HostingError(400, "Invalid JSON.");
    }
  }
  return { requireHostingEnvironment, readHostingJSON };
}
