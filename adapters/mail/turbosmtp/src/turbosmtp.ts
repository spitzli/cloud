import type {
  MailAccountProvider,
  MailCredentialMaster,
  MailCredentialIssuer,
  MailConsumerKeyProvider,
} from "@spitzli/module-mail/server/provider-ports";
import type { Fetch } from "./http";
// Server-only provider adapter. Authorization tokens stay server-side; newly created tenant keys are shown once to authorized administrators.
// Contract: https://serversmtp.com/turbo-api/turbo-smtp.yaml (V2, MIT).
import { isIP, isIPv4 } from "node:net";

export type TurboSMTPAuth =
  { consumerKey: string; consumerSecret: string } | { apiKey: string };
export interface TurboSMTPProfile {
  first_name: string;
  last_name: string;
  ip: string;
  policy_agree: true;
  address_1?: string | null;
  address_2?: string | null;
  city?: string | null;
  company_name?: string | null;
  country?: string | null;
  region?: string | null;
  zip_code?: string | null;
  phone_number?: string | null;
  site_url?: string | null;
  password?: string;
  confirm_password?: string;
}
export interface TurboSMTPCreateRequest extends TurboSMTPProfile {
  email: string;
  password: string;
  confirm_password: string;
}
export interface TurboSMTPSubaccount {
  id: string;
  email?: string;
  active: boolean;
  assignedIP?: string;
  limit?: number;
  sent?: number;
  interval?: "Daily" | "Monthly" | "Yearly";
  expired?: boolean;
  planExpiration?: string | null;
  lastUsed?: string | null;
}
export interface TurboSMTPSubaccountPage {
  count: number;
  results: TurboSMTPSubaccount[];
}
export type TurboSMTPPermission = "SEND_SMTP" | "SEND_API" | "APIS";
export interface TurboSMTPConsumerKey {
  consumerKey: string;
  label: string;
  creation_time: string;
  ips: string[];
  is_legacy: boolean;
  permissions: string[];
}
export interface TurboSMTPKeyOptions {
  permissions: TurboSMTPPermission[];
  ips?: string[];
}
export type TurboSMTPErrorCode =
  "invalid_input" | "upstream" | "invalid_response" | "timeout";
export class TurboSMTPError extends Error {
  constructor(
    public readonly code: TurboSMTPErrorCode,
    public readonly status?: number,
  ) {
    super(
      {
        invalid_input: "Invalid turboSMTP input.",
        upstream: "turboSMTP request failed.",
        invalid_response: "Invalid turboSMTP response.",
        timeout: "turboSMTP request timed out.",
      }[code],
    );
    this.name = "TurboSMTPError";
  }
}

const BASE = "https://pro.api.serversmtp.com/api/v2";
const MAX_BODY = 1_048_576;
const optionalProfileLengths = {
  address_1: 255,
  address_2: 255,
  city: 100,
  company_name: 100,
  country: 64,
  region: 100,
  zip_code: 10,
  phone_number: 30,
  site_url: 45,
};
const input = () => new TurboSMTPError("invalid_input");
const invalidResponse = () => new TurboSMTPError("invalid_response");
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw invalidResponse();
  return value as Record<string, unknown>;
}
function string(value: unknown, max = 4096): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= max &&
    !/[\u0000-\u001f\u007f]/.test(value)
  );
}
function secret(value: unknown): value is string {
  return string(value) && /^[\x21-\x7e]+$/.test(value);
}
function email(value: unknown): value is string {
  return string(value, 254) && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value);
}
function positiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}
function idPath(id: string | number): string {
  if (
    !(typeof id === "string" && /^[1-9]\d*$/.test(id)) &&
    !positiveInteger(id)
  )
    throw input();
  if (!positiveInteger(Number(id))) throw input();
  return `/subaccounts/${id}`;
}
function authHeaders(auth: TurboSMTPAuth): Record<string, string> {
  if (!auth || typeof auth !== "object") throw input();
  if (
    "apiKey" in auth &&
    !("consumerKey" in auth) &&
    !("consumerSecret" in auth) &&
    secret(auth.apiKey)
  )
    return { Authorization: auth.apiKey };
  if (
    !("apiKey" in auth) &&
    "consumerKey" in auth &&
    "consumerSecret" in auth &&
    secret(auth.consumerKey) &&
    secret(auth.consumerSecret)
  )
    return {
      consumerKey: auth.consumerKey,
      consumerSecret: auth.consumerSecret,
    };
  throw input();
}
function profileBody(
  value: TurboSMTPProfile | TurboSMTPCreateRequest,
  create: boolean,
): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw input();
  const allowed = [
    "first_name",
    "last_name",
    "ip",
    "policy_agree",
    "password",
    "confirm_password",
    ...Object.keys(optionalProfileLengths),
    ...(create ? ["email"] : []),
  ];
  if (Object.keys(value).some((key) => !allowed.includes(key))) throw input();
  if (
    !string(value.first_name, 50) ||
    !string(value.last_name, 50) ||
    !string(value.ip) ||
    !isIPv4(value.ip) ||
    value.policy_agree !== true
  )
    throw input();
  if (create && !("email" in value && email(value.email))) throw input();
  if (
    create ||
    value.password !== undefined ||
    value.confirm_password !== undefined
  ) {
    if (
      !string(value.password) ||
      value.password.length < 10 ||
      !/[A-Z]/.test(value.password) ||
      !/\d/.test(value.password) ||
      value.password !== value.confirm_password
    )
      throw input();
  }
  for (const [key, max] of Object.entries(optionalProfileLengths)) {
    const field = value[key as keyof typeof optionalProfileLengths];
    if (
      field !== undefined &&
      field !== null &&
      (typeof field !== "string" ||
        field.length > max ||
        /[\u0000-\u001f\u007f]/.test(field))
    )
      throw input();
  }
  // Copy only schema properties; callers cannot smuggle provider controls or auth into the body.
  return Object.fromEntries(
    allowed
      .filter((key) => Object.hasOwn(value, key))
      .map((key) => [key, value[key as keyof typeof value]]),
  );
}
function subaccount(value: unknown): TurboSMTPSubaccount {
  const data = record(value);
  if (!positiveInteger(data.subaccount_id) || typeof data.active !== "boolean")
    throw invalidResponse();
  const result: TurboSMTPSubaccount = {
    id: String(data.subaccount_id),
    active: data.active,
  };
  if (data.email !== undefined) {
    if (!email(data.email)) throw invalidResponse();
    result.email = data.email;
  }
  if (data.ip !== undefined) {
    if (typeof data.ip !== "string" || !isIPv4(data.ip))
      throw invalidResponse();
    result.assignedIP = data.ip;
  }
  for (const key of ["limit", "sent"] as const) {
    const field = data[key];
    if (field !== undefined) {
      if (
        typeof field !== "number" ||
        !Number.isSafeInteger(field) ||
        field < (key === "limit" ? -1 : 0)
      )
        throw invalidResponse();
      result[key] = field;
    }
  }
  if (data.plan_limit_interval !== undefined) {
    if (
      !["Daily", "Monthly", "Yearly"].includes(String(data.plan_limit_interval))
    )
      throw invalidResponse();
    result.interval =
      data.plan_limit_interval as TurboSMTPSubaccount["interval"];
  }
  if (data.expired !== undefined) {
    if (typeof data.expired !== "boolean") throw invalidResponse();
    result.expired = data.expired;
  }
  for (const [from, to] of [
    ["plan_expiration", "planExpiration"],
    ["last_used", "lastUsed"],
  ] as const) {
    const field = data[from];
    if (field !== undefined) {
      if (
        field !== null &&
        (typeof field !== "string" ||
          !/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(field))
      )
        throw invalidResponse();
      result[to] = field as string | null;
    }
  }
  return result;
}

export class TurboSMTPClient
  implements
    MailAccountProvider,
    MailCredentialMaster,
    MailCredentialIssuer,
    MailConsumerKeyProvider
{
  #headers: Record<string, string>;
  #fetch: Fetch;
  constructor(auth: TurboSMTPAuth, fetchImpl: Fetch = fetch) {
    this.#headers = authHeaders(auth);
    this.#fetch = fetchImpl;
  }

  async #request(
    path: string,
    method = "GET",
    body?: Record<string, unknown>,
    apiKey?: string,
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
    const request = async () => {
      const response = await this.#fetch(BASE + path, {
        method,
        redirect: "error",
        cache: "no-store",
        signal: controller.signal,
        headers: {
          ...(apiKey === undefined ? this.#headers : authHeaders({ apiKey })),
          Accept: "application/json",
          ...(body ? { "Content-Type": "application/json" } : {}),
        },
        ...(body ? { body: JSON.stringify(body) } : {}),
      });
      reader = response.body?.getReader();
      if (!response.ok) throw new TurboSMTPError("upstream", response.status);
      if (!reader || Number(response.headers.get("content-length")) > MAX_BODY)
        throw invalidResponse();
      const chunks: Uint8Array[] = [];
      let size = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > MAX_BODY) throw invalidResponse();
        chunks.push(value);
      }
      const buffer = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) {
        buffer.set(chunk, offset);
        offset += chunk.byteLength;
      }
      try {
        return JSON.parse(
          new TextDecoder("utf-8", { fatal: true }).decode(buffer),
        ) as unknown;
      } catch {
        throw invalidResponse();
      }
    };
    try {
      return await Promise.race([request(), deadline]);
    } catch (error) {
      if (controller.signal.aborted) throw new TurboSMTPError("timeout");
      if (error instanceof TurboSMTPError) throw error;
      throw new TurboSMTPError("upstream");
    } finally {
      clearTimeout(timer);
      controller.abort();
      void reader?.cancel().catch(() => {});
    }
  }

  async listSubaccounts({
    page = 1,
    limit = 100,
    filterByEmail,
  }: {
    page?: number;
    limit?: number;
    filterByEmail?: string;
  } = {}): Promise<TurboSMTPSubaccountPage> {
    if (
      !positiveInteger(page) ||
      !positiveInteger(limit) ||
      (filterByEmail !== undefined && !string(filterByEmail, 254))
    )
      throw input();
    const query = new URLSearchParams({
      page: String(page),
      limit: String(limit),
    });
    if (filterByEmail !== undefined)
      query.set("filter_by_email", filterByEmail);
    const data = record(await this.#request(`/subaccounts/list?${query}`));
    if (
      typeof data.count !== "number" ||
      !Number.isSafeInteger(data.count) ||
      data.count < 0 ||
      !Array.isArray(data.results)
    )
      throw invalidResponse();
    return { count: data.count, results: data.results.map(subaccount) };
  }
  async getSubaccount(id: string | number): Promise<TurboSMTPSubaccount> {
    return subaccount(await this.#request(idPath(id)));
  }
  async getSubaccountPlan(id: string | number): Promise<TurboSMTPSubaccount> {
    return subaccount(await this.#request(`${idPath(id)}/active-plan`));
  }
  async createSubaccount(
    request: TurboSMTPCreateRequest,
  ): Promise<TurboSMTPSubaccount> {
    return subaccount(
      await this.#request("/subaccounts", "POST", profileBody(request, true)),
    );
  }
  async updateSubaccount(
    id: string | number,
    profile: TurboSMTPProfile,
  ): Promise<TurboSMTPSubaccount> {
    return subaccount(
      await this.#request(idPath(id), "PATCH", profileBody(profile, false)),
    );
  }
  async setSubaccountLimit(
    id: string | number,
    limit: number,
  ): Promise<TurboSMTPSubaccount> {
    if (!Number.isSafeInteger(limit) || limit < -1) throw input();
    return subaccount(
      await this.#request(`${idPath(id)}/updatesubaccountsmtplimit`, "POST", {
        limit,
      }),
    );
  }
  async setSubaccountActive(
    id: string | number,
    active: boolean,
  ): Promise<TurboSMTPSubaccount> {
    if (typeof active !== "boolean") throw input();
    return subaccount(
      await this.#request(`${idPath(id)}/updatesubaccountstatus`, "POST", {
        active,
      }),
    );
  }
  async authorizeSubaccount(address: string): Promise<string> {
    if (!email(address)) throw input();
    const data = record(
      await this.#request("/subaccounts/authorize", "POST", { email: address }),
    );
    if (!secret(data.auth)) throw invalidResponse();
    return data.auth;
  }
  async listConsumerKeys(): Promise<{
    count: number;
    results: TurboSMTPConsumerKey[];
  }> {
    if (!this.#headers.Authorization) throw input();
    const data = record(await this.#request("/user/consumerKeys"));
    if (
      !Number.isSafeInteger(data.count) ||
      Number(data.count) < 0 ||
      !Array.isArray(data.results)
    )
      throw invalidResponse();
    const results = data.results.map((value) => {
      const row = record(value);
      if (
        !secret(row.consumerKey) ||
        !string(row.label, 255) ||
        !string(row.creation_time, 64) ||
        typeof row.is_legacy !== "boolean" ||
        !Array.isArray(row.ips) ||
        !row.ips.every((ip) => typeof ip === "string" && isIP(ip)) ||
        !Array.isArray(row.permissions) ||
        !row.permissions.every((p) => string(p, 100))
      )
        throw invalidResponse();
      return {
        consumerKey: row.consumerKey,
        label: row.label,
        creation_time: row.creation_time,
        ips: row.ips as string[],
        is_legacy: row.is_legacy,
        permissions: row.permissions as string[],
      };
    });
    return { count: Number(data.count), results };
  }
  async deleteConsumerKey(consumerKey: string): Promise<void> {
    if (
      !this.#headers.Authorization ||
      !secret(consumerKey) ||
      consumerKey.length > 255 ||
      [".", ".."].includes(consumerKey)
    )
      throw input();
    const data = record(
      await this.#request(
        `/user/consumerKeys/${encodeURIComponent(consumerKey)}`,
        "DELETE",
      ),
    );
    if (data.message !== "success") throw invalidResponse();
  }
  async createConsumerKey(
    apiKey: string,
    label: string,
    options: TurboSMTPKeyOptions = { permissions: ["APIS"] },
  ): Promise<{ consumerKey: string; consumerSecret: string }> {
    if (
      !secret(apiKey) ||
      !string(label, 255) ||
      !Array.isArray(options.permissions) ||
      !options.permissions.length ||
      options.permissions.some(
        (p) => !["SEND_SMTP", "SEND_API", "APIS"].includes(p),
      ) ||
      (options.ips !== undefined &&
        (!Array.isArray(options.ips) ||
          options.ips.some((ip) => typeof ip !== "string" || !isIP(ip))))
    )
      throw input();
    const data = record(
      await this.#request(
        "/user/consumerKeys",
        "POST",
        {
          label,
          permissions: [...new Set(options.permissions)],
          ...(options.ips === undefined ? {} : { ips: options.ips }),
        },
        apiKey,
      ),
    );
    if (!secret(data.consumerKey) || !secret(data.consumerSecret))
      throw invalidResponse();
    return {
      consumerKey: data.consumerKey,
      consumerSecret: data.consumerSecret,
    };
  }
}
