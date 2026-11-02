type Options = { url: string; authorization: string; allowInsecureHttp?: boolean; timeoutMs?: number };
const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);

/** Internal adapter: the endpoint and credential must come from the instance registry. */
export class StalwartClient {
  private readonly endpoint: URL;
  private readonly authorization: string;
  private readonly timeoutMs: number;

  constructor(options: Options) {
    const url = new URL(options.url);
    if ((url.protocol !== "https:" && !(options.allowInsecureHttp === true && url.protocol === "http:")) ||
      url.username || url.password || url.search || url.hash || url.pathname !== "/")
      throw new Error("Invalid Stalwart endpoint configuration");
    if (!options.authorization || /[\r\n]/.test(options.authorization)) throw new Error("Invalid Stalwart credential configuration");
    this.endpoint = new URL("/jmap", url);
    this.authorization = options.authorization;
    this.timeoutMs = options.timeoutMs ?? 10_000;
    if (!Number.isSafeInteger(this.timeoutMs) || this.timeoutMs < 1 || this.timeoutMs > 60_000)
      throw new Error("Invalid Stalwart timeout configuration");
  }

  async call(method: string, args: Record<string, unknown>): Promise<Record<string, unknown>> {
    if (!/^x:[A-Za-z]+\/(get|query|set)$/.test(method)) throw new Error("Unsupported Stalwart management method");
    const mutation = method.endsWith("/set");
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    let result: unknown;
    try {
      const response = await fetch(this.endpoint, {
        method: "POST", redirect: "error", signal: controller.signal,
        headers: { "Content-Type": "application/json", Authorization: this.authorization },
        body: JSON.stringify({ using: ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"], methodCalls: [[method, args, "webdock"]] }),
      });
      if (!response.ok) {
        await response.body?.cancel();
        throw new Error("request");
      }
      const reader = response.body?.getReader();
      if (!reader) throw new Error("response");
      const chunks: Uint8Array[] = [];
      let size = 0;
      try {
        for (;;) {
          const part = await reader.read();
          if (part.done) break;
          size += part.value.byteLength;
          if (size > 1_048_576) throw new Error("response");
          chunks.push(part.value);
        }
        try { result = JSON.parse(Buffer.concat(chunks).toString("utf8")); }
        catch { throw new Error("response"); }
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
    } catch (error) {
      // Never retry writes: a lost reply does not prove the mutation was rejected.
      if (mutation) throw new Error("Stalwart mutation outcome unknown; reconcile before retrying");
      throw new Error(error instanceof Error && error.message === "response" ? "Stalwart returned an invalid response" : "Stalwart request failed");
    } finally { clearTimeout(timer); controller.abort(); }
    const responses = object(result) ? result.methodResponses : undefined;
    const response = Array.isArray(responses) && responses.length === 1 ? responses[0] : undefined;
    if (!Array.isArray(response) || response.length !== 3 || response[2] !== "webdock" || !object(response[1]))
      throw new Error(`Stalwart returned an invalid response${mutation ? "; mutation outcome unknown" : ""}`);
    if (response[0] === "error") throw new Error("Stalwart rejected the management request");
    if (response[0] !== method) throw new Error(`Stalwart returned an invalid response${mutation ? "; mutation outcome unknown" : ""}`);
    if (["notCreated", "notUpdated", "notDestroyed"].some(key => object(response[1][key]) && Object.keys(response[1][key]).length > 0))
      throw new Error("Stalwart object change rejected; reconcile any partial changes before retrying");
    return response[1];
  }
}
