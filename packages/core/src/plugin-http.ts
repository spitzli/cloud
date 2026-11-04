import https from "node:https";
import { isIP } from "node:net";
import { PluginError, httpRequestSchema } from "@spitzli/plugin-sdk/contracts";
export type HTTPBinding = {
  origin: string;
  addresses: string[];
  headers: Record<string, string>;
  rules: Array<{
    method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
    path: string;
    query: string[];
  }>;
  ca?: string;
};
export interface HTTPAuthority {
  authorize(
    request: Request,
    binding: string,
    method: string,
  ): Promise<HTTPBinding>;
}
export type HTTPResult = { status: number; body: unknown };
export type HTTPTransport = (input: {
  url: URL;
  addresses: string[];
  headers: Record<string, string>;
  method: string;
  body?: string;
  ca?: string;
}) => Promise<HTTPResult>;
const limit = 1024 * 1024;
function denied(message = "HTTP binding does not permit this request"): never {
  throw new PluginError("permission_denied", message);
}
/** No DNS lookup is performed: reviewed addresses are pinned, while TLS verifies the bound hostname. */
export const pinnedHTTPTransport: HTTPTransport = async (input) =>
  new Promise((resolve, reject) => {
    const deadline = setTimeout(
      () => req.destroy(new Error("Provider deadline exceeded")),
      10_000,
    );
    const req = https.request(
      input.url,
      {
        method: input.method,
        headers: {
          ...input.headers,
          accept: "application/json",
          ...(input.body === undefined
            ? {}
            : {
                "content-type": "application/json",
                "content-length": Buffer.byteLength(input.body),
              }),
        },
        ca: input.ca,
        rejectUnauthorized: true,
        servername: input.url.hostname,
        agent: false,
        lookup: (_host, options, callback) => {
          const entries = input.addresses.map((address) => ({
            address,
            family: isIP(address),
          }));
          if (options.all) callback(null, entries);
          else callback(null, entries[0].address, entries[0].family);
        },
      },
      (res) => {
        let bytes = 0;
        const chunks: Buffer[] = [];
        res.on("data", (chunk: Buffer) => {
          bytes += chunk.length;
          if (bytes > limit)
            res.destroy(new Error("Provider response exceeds limit"));
          else chunks.push(chunk);
        });
        res.on("error", () => {clearTimeout(deadline);reject(new PluginError("unavailable","Provider response failed",true));});
        res.on("end", () => {
          clearTimeout(deadline);
          try {
            const data = Buffer.concat(chunks).toString("utf8");
            resolve({
              status: res.statusCode ?? 502,
              body: data ? JSON.parse(data) : null,
            });
          } catch {
            reject(
              new PluginError(
                "unavailable",
                "Provider returned invalid JSON",
                true,
              ),
            );
          }
        });
      },
    );
    req.on("error", () => {
      clearTimeout(deadline);
      reject(new PluginError("unavailable", "Provider request failed", true));
    });
    req.end(input.body);
  });
/** Host only. A binding is selected using verified invocation scope, never client-supplied tenant IDs. */
export function createPluginHTTP(
  authority: HTTPAuthority,
  transport: HTTPTransport = pinnedHTTPTransport,
) {
  async function request(req: Request, raw: unknown): Promise<HTTPResult> {
    const parsed = httpRequestSchema.safeParse(raw);
    if (!parsed.success)
      throw new PluginError("invalid_input", "Invalid HTTP request");
    const input = parsed.data;
    if (
      !input.path.startsWith("/") ||
      input.path.startsWith("//") ||
      /[\\%#\x00-\x20]/.test(input.path) ||
      input.path
        .split("?")[0]
        .split("/")
        .some((p) => p === "." || p === "..")
    )
      denied();
    if (input.method === "GET" && input.body !== undefined) denied();
    const binding = await authority.authorize(req, input.binding, input.method);
    const origin = new URL(binding.origin);
    if (
      origin.protocol !== "https:" ||
      origin.username ||
      origin.password ||
      origin.pathname !== "/" ||
      origin.search ||
      origin.hash ||
      !binding.addresses.length ||
      binding.addresses.some((a) => !isIP(a))
    )
      denied("Invalid operator HTTP binding");
    const url = new URL(input.path, origin);
    if (url.origin !== origin.origin) denied();
    const rule = binding.rules.find(
      (r) => r.method === input.method && r.path === url.pathname,
    );
    if (!rule) denied();
    for (const key of url.searchParams.keys())
      if (
        !rule.query.includes(key) ||
        url.searchParams.getAll(key).length !== 1
      )
        denied();
    const body =
      input.body === undefined ? undefined : JSON.stringify(input.body);
    if (body !== undefined && Buffer.byteLength(body) > limit)
      throw new PluginError("invalid_input", "Provider request exceeds limit");
    const response = await transport({
      url,
      method: input.method,
      body,
      addresses: binding.addresses,
      headers: binding.headers,
      ca: binding.ca,
    });
    await authority.authorize(req, input.binding, input.method);
    if (response.status >= 300 && response.status < 400)
      denied("Provider redirects are forbidden");
    if (response.status === 401 || response.status === 403)
      denied("Provider rejected its configured credentials");
    if (response.status >= 500)
      throw new PluginError("unavailable", "Provider unavailable", true);
    return response;
  }
  return { request };
}
