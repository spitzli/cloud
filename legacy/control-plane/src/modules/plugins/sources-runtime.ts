import { readFile } from "node:fs/promises";
import { isDeepStrictEqual } from "node:util";
import {
  pinnedHTTPTransport,
  type HTTPTransport,
} from "@webdock/core/plugin-http";
import {
  parseSourceManifest,
  PluginSourceError,
  runtimeBindingSchema,
  sourceFail,
  type SourceRow,
} from "./sources-policy";

export async function verifySourceRuntime(
  row: SourceRow,
  binding: ReturnType<typeof runtimeBindingSchema.parse>,
  file: string,
  transport: HTTPTransport = pinnedHTTPTransport,
) {
  try {
    const settings = JSON.parse(await readFile(file, "utf8")),
      manifest = parseSourceManifest(row.manifest_raw),
      mounted = settings.runtimes?.[manifest.name];
    if (!mounted)
      sourceFail(
        "runtime_not_ready",
        "Wait for the installed runtime binding to become visible, then retry activation.",
        503,
      );
    const parsed = runtimeBindingSchema.safeParse(
      Object.fromEntries(
        Object.keys(binding).map((key) => [key, mounted[key]]),
      ),
    );
    if (
      !parsed.success ||
      mounted.uiOrigin !== binding.uiOrigin ||
      !isDeepStrictEqual(binding, parsed.data)
    )
      sourceFail(
        "runtime_binding_changed",
        "The mounted runtime binding does not match this reviewed import.",
      );
    const response = await transport({
      url: new URL(
        `https://${binding.service}.${binding.namespace}.svc.cluster.local:3443/healthz`,
      ),
      addresses: binding.addresses,
      ca: binding.ca,
      headers: { authorization: "Bearer " + binding.credential },
      method: "GET",
    });
    const health = response.body as Record<string, unknown> | null;
    if (response.status !== 200)
      sourceFail(
        "runtime_not_ready",
        "Wait for the reviewed runtime to become healthy, then retry activation.",
        503,
      );
    if (
      health?.pluginID !== manifest.name ||
      health?.version !== manifest.version ||
      health?.generation !== 1 ||
      health?.protocolVersion !== 1
    )
      sourceFail(
        "runtime_identity_mismatch",
        "Runtime health does not match the reviewed plugin, version and generation.",
      );
  } catch (error) {
    if (error instanceof PluginSourceError) throw error;
    sourceFail(
      "runtime_not_ready",
      "The reviewed runtime is not ready. Check the installer and retry activation.",
      503,
    );
  }
}
