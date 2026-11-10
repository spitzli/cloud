import type { Fetch } from "./http";
// Child-account API only. Contracts: official Analytics API and provider dashboard link-branding client.
export class TrackingProviderError extends Error {
  constructor() {
    super(
      "Tracking is unavailable for this Mail account. Check the connection and refresh before retrying changes.",
    );
  }
}
export type {
  TrackingTool,
  TrackingDomain,
  TrackingInfo,
  TrackingSettings,
} from "@spitzli/module-mail/server/tracking-provider";
import type {
  TrackingTool,
  TrackingDomain,
  TrackingInfo,
  TrackingSettings,
  MailTrackingProvider,
} from "@spitzli/module-mail/server/tracking-provider";
const fail = (): never => {
  throw new TrackingProviderError();
};
const object = (v: unknown): Record<string, unknown> => {
  if (!v || typeof v !== "object" || Array.isArray(v)) return fail();
  return v as Record<string, unknown>;
};
const text = (v: unknown, max = 253): v is string =>
  typeof v === "string" &&
  v.length > 0 &&
  v.length <= max &&
  !/[\x00-\x20\x7f]/.test(v);
const hostname = (v: unknown): v is string =>
  text(v) &&
  v.includes(".") &&
  v
    .split(".")
    .every((part) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(part));
const dnsName = (v: unknown): v is string =>
  text(v) &&
  v.includes(".") &&
  v
    .split(".")
    .every((part) => /^[a-z0-9_](?:[a-z0-9_-]{0,61}[a-z0-9_])?$/i.test(part));
const id = (v: unknown): string => {
  if (!/^[1-9]\d{0,15}$/.test(String(v)) || !Number.isSafeInteger(Number(v)))
    return fail();
  return String(v);
};
function domain(v: unknown): TrackingDomain {
  const row = object(v);
  if (
    !hostname(row.domain_name) ||
    !dnsName(row.verification_domain) ||
    [row.verified, row.ssl, row.enabled, row.default].some(
      (v) => typeof v !== "boolean",
    )
  )
    return fail();
  return {
    id: id(row.id),
    domain_name: row.domain_name,
    verification_domain: row.verification_domain,
    verified: row.verified as boolean,
    ssl: row.ssl as boolean,
    enabled: row.enabled as boolean,
    default: row.default as boolean,
  };
}
function tool(v: unknown, expected: string): TrackingTool {
  const row = object(v);
  if (
    row.id !== expected ||
    (row.forced !== undefined && typeof row.forced !== "boolean") ||
    typeof row.enabled !== "boolean" ||
    (row.settings !== undefined && !Array.isArray(row.settings))
  )
    return fail();
  const settings = ((row.settings || []) as unknown[]).map((v) => {
    const pair = object(v);
    if (
      !text(pair.key, 100) ||
      !["boolean", "string", "number"].includes(typeof pair.value) ||
      (typeof pair.value === "string" && pair.value.length > 2048) ||
      (typeof pair.value === "number" && !Number.isFinite(pair.value))
    )
      return fail();
    return { key: pair.key, value: pair.value as boolean | string | number };
  });
  return {
    id: expected,
    forced: typeof row.forced === "boolean" ? row.forced : null,
    enabled: row.enabled,
    settings,
  };
}
export class TrackingClient implements MailTrackingProvider {
  private credentialHeaders: Record<string, string>;
  constructor(
    token: string | { consumerKey: string; consumerSecret: string },
    private fetchImpl: Fetch = fetch,
  ) {
    const values =
      typeof token === "string"
        ? [token]
        : [token.consumerKey, token.consumerSecret];
    if (
      values.some(
        (value) => !text(value, 4096) || !/^[\x21-\x7e]+$/.test(value),
      )
    )
      fail();
    this.credentialHeaders =
      typeof token === "string"
        ? { Authorization: token }
        : {
            consumerKey: token.consumerKey,
            consumerSecret: token.consumerSecret,
          };
  }
  async #request(
    path: string,
    method = "GET",
    body?: object,
    json = true,
  ): Promise<unknown> {
    const controller = new AbortController();
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => {
            controller.abort();
            reject(new TrackingProviderError());
          }, 10000);
        }),
        (async () => {
          const response = await this.fetchImpl(
            `https://pro.api.serversmtp.com/api/v2${path}`,
            {
              method,
              redirect: "error",
              cache: "no-store",
              signal: controller.signal,
              headers: {
                ...this.credentialHeaders,
                Accept: "application/json",
                ...(body ? { "Content-Type": "application/json" } : {}),
              },
              ...(body ? { body: JSON.stringify(body) } : {}),
            },
          );
          if (
            !response.ok ||
            Number(response.headers.get("content-length")) > 1048576
          )
            return fail();
          reader = response.body?.getReader();
          let result = "",
            size = 0;
          const decoder = new TextDecoder("utf-8", { fatal: true });
          if (reader)
            for (;;) {
              const chunk = await reader.read();
              if (chunk.done) break;
              size += chunk.value.byteLength;
              if (size > 1048576) return fail();
              result += decoder.decode(chunk.value, { stream: true });
            }
          const content = result + decoder.decode();
          let parsed: unknown;
          try {
            parsed = JSON.parse(content);
          } catch {
            if (json) return fail();
          }
          if (
            parsed &&
            typeof parsed === "object" &&
            "status" in parsed &&
            typeof parsed.status === "string" &&
            parsed.status.toLowerCase() === "error"
          )
            return fail();
          return json ? parsed : undefined;
        })(),
      ]);
    } catch {
      return fail();
    } finally {
      clearTimeout(timer);
      controller.abort();
      void reader?.cancel().catch(() => {});
    }
  }
  async settings(): Promise<TrackingSettings> {
    // The aggregate endpoint is verified with child tokens; individual tool GET routes return server errors.
    const [rows, value] = await Promise.all([
      this.#request("/tools"),
      this.#request("/tools/link_branding/domains_info"),
    ]);
    if (
      !Array.isArray(rows) ||
      !rows.every((value) => {
        const row = object(value);
        return text(row.id, 100) && typeof row.enabled === "boolean";
      })
    )
      return fail();
    const readTool = (id: string) => {
      const matches = rows.filter((row) => row.id === id);
      if (matches.length !== 1) return fail();
      return tool(matches[0], id);
    };
    const info = object(value);
    if (
      [
        info.all_domains_disabled,
        info.match_sender,
        info.no_default_domain,
      ].some((v) => typeof v !== "boolean")
    )
      return fail();
    const custom = rows.filter((row) => row.id === "link_branding");
    if (custom.length > 1) return fail();
    return {
      custom: custom.length === 1 ? custom[0].enabled : null,
      click: readTool("clickTracking"),
      opening: readTool("openingTracking"),
      info: {
        all_domains_disabled: info.all_domains_disabled as boolean,
        match_sender: info.match_sender as boolean,
        no_default_domain: info.no_default_domain as boolean,
      },
    };
  }
  async list(): Promise<TrackingDomain[]> {
    const results: TrackingDomain[] = [];
    let expectedCount: number | undefined;
    for (let page = 1; page <= 20; page++) {
      const row = object(
        await this.#request(`/tools/link_branding?page=${page}&limit=100`),
      );
      if (
        !Number.isSafeInteger(row.count) ||
        Number(row.count) < 0 ||
        Number(row.count) > 2000 ||
        !Array.isArray(row.results) ||
        row.results.length > 100
      )
        return fail();
      if (expectedCount !== undefined && expectedCount !== row.count)
        return fail();
      expectedCount = Number(row.count);
      results.push(...row.results.map(domain));
      if (
        results.length > expectedCount ||
        new Set(results.map((row) => row.id)).size !== results.length
      )
        return fail();
      if (results.length === expectedCount) return results;
      if (row.results.length === 0) return fail();
    }
    return fail();
  }
  async get(value: string) {
    return domain(await this.#request(`/tools/link_branding/${id(value)}`));
  }
  async create(senderDomain: string, fullHostname: string): Promise<string> {
    if (
      !hostname(senderDomain) ||
      !hostname(fullHostname) ||
      !fullHostname.endsWith(`.${senderDomain}`)
    )
      return fail();
    return id(
      object(
        await this.#request("/tools/link_branding", "POST", {
          domain: senderDomain,
          subdomain: fullHostname,
        }),
      ).id,
    );
  }
  async verify(value: string) {
    const row = object(
      await this.#request(
        `/tools/link_branding/verify/${id(value)}`,
        "POST",
        {},
      ),
    );
    if (row.message !== "success") fail();
  }
  async remove(value: string) {
    await this.#request(
      `/tools/link_branding/${id(value)}`,
      "DELETE",
      undefined,
      false,
    );
  }
  async domainSetting(
    value: string,
    setting: "enabled" | "default",
    enabled: boolean,
  ) {
    if (
      !["enabled", "default"].includes(setting) ||
      typeof enabled !== "boolean"
    )
      return fail();
    await this.#request(
      `/tools/link_branding/${setting}/${id(value)}/${enabled ? "enable" : "disable"}`,
      "PUT",
      {},
      false,
    );
  }
  async setting(
    setting: "click" | "opening" | "custom" | "matchSender",
    enabled: boolean,
  ) {
    const paths = {
      click: "clickTracking",
      opening: "openingTracking",
      custom: "link_branding",
      matchSender: "link_branding_match_sender",
    };
    if (!Object.hasOwn(paths, setting) || typeof enabled !== "boolean")
      return fail();
    await this.#request(
      `/tools/${paths[setting]}/${enabled ? "enable" : "disable"}`,
      "PUT",
      {},
      false,
    );
  }
}
