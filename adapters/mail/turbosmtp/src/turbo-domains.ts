import type { MailDomainProvider } from "@spitzli/module-mail/server/provider-ports";
import type { Fetch } from "./http";
import { domainToASCII } from "node:url";
import { TurboSMTPError, type TurboSMTPAuth } from "./turbosmtp";

export type SenderDomain = {
  id: string;
  domain: string;
  spf_verified: boolean;
  dkim_verified: boolean;
  dmarc_verified: boolean;
};
const invalid = () => new TurboSMTPError("invalid_response");
export function senderDomain(value: unknown): SenderDomain {
  if (!value || typeof value !== "object") throw invalid();
  const row = value as Record<string, unknown>;
  if (
    !(typeof row.id === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(row.id)) &&
    !(typeof row.id === "number" && Number.isSafeInteger(row.id) && row.id > 0)
  )
    throw invalid();
  if (
    typeof row.domain !== "string" ||
    row.domain.length > 253 ||
    !row.domain.includes(".") ||
    !row.domain
      .split(".")
      .every((part) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(part))
  )
    throw invalid();
  if (
    [row.spf_verified, row.dkim_verified, row.dmarc_verified].some(
      (flag) => typeof flag !== "boolean",
    )
  )
    throw invalid();
  return {
    id: String(row.id),
    domain: row.domain.toLowerCase(),
    spf_verified: row.spf_verified as boolean,
    dkim_verified: row.dkim_verified as boolean,
    dmarc_verified: row.dmarc_verified as boolean,
  };
}
export class TurboDomainsClient implements MailDomainProvider {
  #headers: Record<string, string>;
  constructor(
    auth: TurboSMTPAuth,
    private fetchImpl: Fetch = fetch,
  ) {
    const secret = (v: unknown) =>
      typeof v === "string" && /^[\x21-\x7e]{1,4096}$/.test(v);
    if (
      "apiKey" in auth &&
      !("consumerKey" in auth) &&
      !("consumerSecret" in auth) &&
      secret(auth.apiKey)
    )
      this.#headers = { Authorization: auth.apiKey };
    else if (
      !("apiKey" in auth) &&
      "consumerKey" in auth &&
      "consumerSecret" in auth &&
      secret(auth.consumerKey) &&
      secret(auth.consumerSecret)
    )
      this.#headers = {
        consumerKey: auth.consumerKey,
        consumerSecret: auth.consumerSecret,
      };
    else throw new TurboSMTPError("invalid_input");
  }
  async #request(
    domain?: string,
    deleteID?: string,
    page?: number,
  ): Promise<unknown> {
    const controller = new AbortController();
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const deadline = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        controller.abort();
        reject(new TurboSMTPError("timeout"));
      }, 10_000);
    });
    try {
      return await Promise.race([
        deadline,
        (async () => {
          const response = await this.fetchImpl(
            `https://pro.api.serversmtp.com/api/v2/sender-domains${deleteID ? `/${encodeURIComponent(deleteID)}` : page ? `?page=${page}&limit=100` : ""}`,
            {
              method: deleteID ? "DELETE" : domain ? "POST" : "GET",
              headers: {
                ...this.#headers,
                Accept: "application/json",
                ...(domain ? { "Content-Type": "application/json" } : {}),
              },
              ...(domain ? { body: JSON.stringify({ domain }) } : {}),
              redirect: "error",
              cache: "no-store",
              signal: controller.signal,
            },
          );
          reader = response.body?.getReader();
          if (!response.ok)
            throw new TurboSMTPError("upstream", response.status);
          if (deleteID && response.status === 204) return null;
          if (
            !reader ||
            Number(response.headers.get("content-length")) > 1_048_576
          )
            throw invalid();
          let size = 0,
            body = "";
          const decoder = new TextDecoder("utf-8", { fatal: true });
          for (;;) {
            const part = await reader.read();
            if (part.done) break;
            size += part.value.byteLength;
            if (size > 1_048_576) throw invalid();
            body += decoder.decode(part.value, { stream: true });
          }
          if (deleteID && !body.trim()) return null;
          try {
            return JSON.parse(body + decoder.decode()) as unknown;
          } catch {
            throw invalid();
          }
        })(),
      ]);
    } catch (error) {
      if (error instanceof TurboSMTPError) throw error;
      throw new TurboSMTPError("upstream");
    } finally {
      clearTimeout(timer);
      controller.abort();
      void reader?.cancel().catch(() => {});
    }
  }
  async listSenderDomains(): Promise<SenderDomain[]> {
    const results: SenderDomain[] = [],
      ids = new Set<string>();
    let expected: number | undefined;
    for (let page = 1; page <= 10; page++) {
      const data = (await this.#request(undefined, undefined, page)) as {
        results?: unknown[];
        count?: unknown;
      } | null;
      if (!data || !Array.isArray(data.results)) throw invalid();
      if (data.count !== undefined) {
        const count =
          typeof data.count === "string" && /^\d+$/.test(data.count)
            ? Number(data.count)
            : data.count;
        if (
          typeof count !== "number" ||
          !Number.isSafeInteger(count) ||
          count < 0 ||
          (expected !== undefined && count !== expected)
        )
          throw invalid();
        expected = count;
      } else if (page !== 1) throw invalid();
      const rows = data.results.map(senderDomain);
      for (const row of rows) {
        if (ids.has(row.id)) throw invalid();
        ids.add(row.id);
        results.push(row);
      }
      // RelayKit's documented unpaginated envelope omits count entirely.
      if (expected === undefined || results.length === expected) return results;
      if (!rows.length || results.length > expected) throw invalid();
    }
    throw invalid();
  }
  async deleteSenderDomain(id: string): Promise<void> {
    if (typeof id !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(id))
      throw new TurboSMTPError("invalid_input");
    const result = await this.#request(undefined, id);
    if (
      result !== null &&
      (!result ||
        typeof result !== "object" ||
        Array.isArray(result) ||
        !(
          (result as { message?: unknown }).message === "success" ||
          (result as { message?: unknown }).message === "domain_deleted" ||
          (result as { success?: unknown }).success === true ||
          (result as { deleted?: unknown }).deleted === true
        ))
    )
      throw invalid();
  }
  async registerSenderDomain(domain: string): Promise<SenderDomain> {
    if (
      typeof domain !== "string" ||
      domainToASCII(domain) !== domain ||
      /^\d+(\.\d+){3}$/.test(domain)
    )
      throw new TurboSMTPError("invalid_input");
    try {
      senderDomain({
        id: "1",
        domain,
        spf_verified: false,
        dkim_verified: false,
        dmarc_verified: false,
      });
    } catch {
      throw new TurboSMTPError("invalid_input");
    }
    return senderDomain(await this.#request(domain));
  }
}
