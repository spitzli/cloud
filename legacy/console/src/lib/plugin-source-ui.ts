import type {
  PluginImportStatus,
  PluginSource,
} from "@webdock/api-client/plugin-sources";

export function parsePluginConfiguration(
  value: string,
): Record<string, unknown> {
  try {
    const parsed: unknown = JSON.parse(value);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed))
      return parsed as Record<string, unknown>;
  } catch {
    /* Show one actionable message for invalid configuration. */
  }
  throw new Error("Configuration must be a JSON object.");
}

export function gitPluginSource(
  rawURL: string,
  ref: string,
  username: string,
  token: string,
): PluginSource {
  let url: URL;
  try {
    url = new URL(rawURL.trim());
  } catch {
    throw new Error(
      "Enter an HTTPS URL without embedded credentials or query parameters.",
    );
  }
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    throw new Error(
      "Enter an HTTPS URL without embedded credentials or query parameters.",
    );
  if (Boolean(username.trim()) !== Boolean(token))
    throw new Error(
      "Provide both a username and a token, or leave both empty.",
    );
  return {
    kind: "git",
    url: url.href,
    ...(ref.trim() ? { ref: ref.trim() } : {}),
    ...(token ? { credential: { username: username.trim(), token } } : {}),
  };
}

export function validatePluginArchive(file: { name: string; size: number }) {
  if (
    !file.name.toLowerCase().endsWith(".zip") ||
    file.size <= 0 ||
    file.size > 32 * 1024 * 1024
  )
    throw new Error("Choose a non-empty ZIP archive up to 32 MiB.");
}

export function encodePluginChunk(bytes: Uint8Array): string {
  if (bytes.length > 64 * 1024)
    throw new Error("Upload chunks must be at most 64 KiB.");
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export function shouldPollPluginImports(
  imports: { status: PluginImportStatus }[],
  attempts: number,
): boolean {
  return (
    attempts < 200 &&
    imports.some((job) =>
      ["queued", "building", "approved", "deploying", "ready"].includes(
        job.status,
      ),
    )
  );
}

export function shouldRetryPluginActivation(
  status: number | undefined,
  attempts: number,
): boolean {
  return status === 503 && attempts < 20;
}
