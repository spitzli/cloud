import {
  CHUNK_BYTES,
  JSON_BYTES,
  MEDIA_BYTES,
  METADATA_BYTES,
  type BridgeHTTP,
  type SerializedRequest,
  type SerializedResponse,
  type TransferResult,
} from "./types.ts";
const encode = (bytes: Uint8Array) => {
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK_BYTES)
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK_BYTES));
  return btoa(binary);
};
const decode = (data: string) =>
  Uint8Array.from(atob(data), (character) => character.charCodeAt(0));
const sha256 = async (bytes: Uint8Array) =>
  Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", bytes as Uint8Array<ArrayBuffer>),
    ),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
export class CMSUncertainSaveError extends Error {
  readonly code = "cms_save_uncertain";
  constructor() {
    super(
      "This save may have completed. Check the content list before trying again.",
    );
  }
}
/** callHTTP delegates each frame through the host's freshly authorized HTTP binding. */
export function createBridgeTransport(
  callHTTP: BridgeHTTP,
): (input: RequestInfo | URL, init?: RequestInit) => Promise<Response> {
  const call = async (args: Parameters<BridgeHTTP>[0]) => {
    const response = await callHTTP(args);
    if (response.status < 200 || response.status >= 300)
      throw new Error(
        (response.body as { error?: string })?.error ||
          "CMS bridge request failed.",
      );
    return response.body as Record<string, unknown>;
  };
  const download = async (result: TransferResult) => {
    if (
      typeof result.transfer !== "string" ||
      !Number.isSafeInteger(result.size) ||
      result.size < 1 ||
      result.size >
        Math.ceil(((MEDIA_BYTES + METADATA_BYTES) * 4) / 3) + METADATA_BYTES ||
      !/^[a-f0-9]{64}$/.test(result.sha256)
    )
      throw Error("Invalid CMS response descriptor.");
    const bytes = new Uint8Array(result.size);
    let offset = 0;
    while (offset < bytes.length) {
      const chunk = await call({
        method: "GET",
        path: `/api/webdock/cms?transfer=${encodeURIComponent(result.transfer)}&offset=${offset}`,
      });
      if (
        typeof chunk.data !== "string" ||
        chunk.data.length > Math.ceil(CHUNK_BYTES / 3) * 4
      )
        throw Error("Invalid CMS response chunk.");
      const decoded = decode(chunk.data);
      if (
        !decoded.length ||
        decoded.length > CHUNK_BYTES ||
        offset + decoded.length > bytes.length ||
        chunk.offset !== offset + decoded.length ||
        chunk.done !== (chunk.offset === bytes.length)
      )
        throw Error("Incomplete CMS response.");
      bytes.set(decoded, offset);
      offset += decoded.length;
    }
    if ((await sha256(bytes)) !== result.sha256)
      throw Error("CMS response integrity check failed.");
    const response = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    ) as SerializedResponse;
    if (
      !Number.isInteger(response.status) ||
      response.status < 200 ||
      response.status > 599 ||
      typeof response.contentType !== "string" ||
      response.contentType.length > 200 ||
      typeof response.bodyBase64 !== "string"
    )
      throw Error("Invalid CMS response.");
    const body = decode(response.bodyBase64);
    if (
      body.length >
      (response.contentType.startsWith("image/") ? MEDIA_BYTES : JSON_BYTES)
    )
      throw Error("CMS response too large.");
    return new Response(body, {
      status: response.status,
      headers: { "content-type": response.contentType },
    });
  };
  return async (input, init) => {
    const path =
      input instanceof Request
        ? new URL(input.url).pathname + new URL(input.url).search
        : String(input);
    if (!/^\/api\/cms(?:\?[^#\\]*)?$/.test(path) || path.length > 1500)
      throw Error("Invalid CMS request path.");
    const request = new Request(new URL(path, "https://cms.invalid"), init);
    const method = request.method as SerializedRequest["method"];
    if (!["GET", "POST", "DELETE"].includes(method))
      throw Error("Unsupported CMS request.");
    const descriptor: SerializedRequest = { method, path };
    if (method === "GET") {
      const encoded = encode(
        new TextEncoder().encode(JSON.stringify(descriptor)),
      )
        .replaceAll("+", "-")
        .replaceAll("/", "_")
        .replace(/=+$/, "");
      return download(
        (await call({
          method: "GET",
          path: `/api/webdock/cms?request=${encoded}`,
        })) as TransferResult,
      );
    }
    // Bun materializes multipart headers lazily; retain them before consuming a large body.
    const contentType = request.headers.get("content-type") ?? "";
    const body = new Uint8Array(await request.arrayBuffer());
    const kind = contentType.startsWith("multipart/form-data;")
      ? "media"
      : "json";
    if (
      body.length >
      (kind === "media" ? MEDIA_BYTES + METADATA_BYTES : JSON_BYTES)
    )
      throw Error("CMS request too large.");
    descriptor.contentType = contentType;
    descriptor.bodyBase64 = encode(body);
    const serialized = new TextEncoder().encode(JSON.stringify(descriptor));
    const started = await call({
      method: "POST",
      path: "/api/webdock/cms",
      body: {
        op: "begin",
        size: serialized.length,
        sha256: await sha256(serialized),
        kind,
      },
    });
    if (typeof started.transfer !== "string")
      throw Error("Invalid CMS upload descriptor.");
    for (let offset = 0; offset < serialized.length; offset += CHUNK_BYTES) {
      const chunk = serialized.subarray(offset, offset + CHUNK_BYTES);
      const appended = await call({
        method: "POST",
        path: "/api/webdock/cms",
        body: {
          op: "append",
          transfer: started.transfer,
          offset,
          data: encode(chunk),
        },
      });
      if (appended.offset !== offset + chunk.length)
        throw Error("Incomplete CMS upload.");
    }
    let committed: TransferResult;
    try {
      committed = (await call({
        method: "POST",
        path: "/api/webdock/cms",
        body: { op: "commit", transfer: started.transfer },
      })) as TransferResult;
    } catch {
      // Outer HTTP errors can follow an applied mutation, including response overflow.
      // Definite native validation failures arrive inside a completed receipt instead.
      throw new CMSUncertainSaveError();
    }
    try {
      const response = await download(committed);
      if (response.status >= 500) throw new CMSUncertainSaveError();
      return response;
    } catch {
      throw new CMSUncertainSaveError();
    }
  };
}
