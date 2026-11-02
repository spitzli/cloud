import { currentDatabaseScope } from "@webdock/database-runtime";
import { createHash } from "node:crypto";
export type ViewKind =
  | "branding"
  | "catalog"
  | "registry"
  | "operations"
  | "navigation"
  | "projectsummary";
type JSONValue =
  null | boolean | number | string | JSONValue[] | { [key: string]: JSONValue };
export type CachedViewOptions = {
  kind: ViewKind;
  scope: Record<string, string | number | boolean | null>;
  query: JSONValue;
  ttlSeconds?: number;
};
type LegacyKey = {
  kind: "branding" | "catalog" | "usage";
  environment: string;
  customerID: string;
  resourceID: string;
  revision: string;
};
export type ViewStore = {
  get(key: string): Promise<string | null>;
  set(key: string, value: string, ttlSeconds: number): Promise<unknown>;
};
const namespace = "webdock:view:v3:";
const hash = (value: string) =>
  createHash("sha256").update(value).digest("hex");
function canonical(value: JSONValue): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
  return (
    "{" +
    Object.keys(value)
      .sort()
      .map((key) => JSON.stringify(key) + ":" + canonical(value[key]))
      .join(",") +
    "}"
  );
}
const sensitive =
  /^(password|secret|client_?secret|access_?token|refresh_?token|id_?token|token|authorization|cookie|set-cookie|credentials|credentials_encrypted|secret_encrypted|private_?key)$/i;
function cacheable(value: unknown): boolean {
  if (value instanceof Error) return false;
  if (!value || typeof value !== "object") return true;
  return Object.entries(value).every(
    ([key, item]) => !sensitive.test(key) && key !== "error" && cacheable(item),
  );
}
async function bounded<T>(work: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(Error("Cache timeout")), 100);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}
/** Only redacted presentation DTOs after live authorization; never authentication, credentials or arbitrary SQL. */
export class ViewCache {
  private unavailableUntil = 0;
  private readonly inFlight = new Map<string, Promise<unknown>>();
  constructor(private readonly store: ViewStore) {}
  async read<T>(key: LegacyKey, load: () => Promise<T>): Promise<T> {
    return this.readKey(namespace + "legacy:" + hash(canonical(key)), 60, load);
  }
  async readKey<T>(
    name: string,
    ttlSeconds: number,
    load: () => Promise<T>,
  ): Promise<T> {
    if (this.unavailableUntil > Date.now()) return load();
    let pending = this.inFlight.get(name) as Promise<T> | undefined;
    if (!pending) {
      pending = (async () => {
        try {
          const cached = await bounded(this.store.get(name));
          if (cached !== null) {
            const value = JSON.parse(cached);
            if (cacheable(value)) return value as T;
          }
        } catch {
          this.unavailableUntil = Date.now() + 10_000;
          return load();
        }
        const value = await load();
        let encoded: string | undefined;
        try {
          if (cacheable(value)) encoded = JSON.stringify(value);
        } catch {
          return value;
        }
        if (encoded !== undefined && Buffer.byteLength(encoded) <= 512 * 1024) {
          try {
            await bounded(this.store.set(name, encoded, ttlSeconds));
          } catch {
            this.unavailableUntil = Date.now() + 10_000;
          }
          return JSON.parse(encoded) as T;
        }
        return value;
      })();
      this.inFlight.set(name, pending);
    }
    try {
      return structuredClone(await pending);
    } finally {
      if (this.inFlight.get(name) === pending) this.inFlight.delete(name);
    }
  }
}
let configured:
  { cache: ViewCache; environment: string; ttlSeconds: number } | undefined;
export function configureViewCache(
  cache: ViewCache | undefined,
  options: { environment: string; defaultTTLSeconds?: number },
) {
  const ttlSeconds = options.defaultTTLSeconds ?? 60;
  if (
    !options.environment ||
    !Number.isInteger(ttlSeconds) ||
    ttlSeconds < 1 ||
    ttlSeconds > 300
  )
    throw Error("Invalid view cache configuration");
  configured = cache
    ? { cache, environment: options.environment, ttlSeconds }
    : undefined;
}
/** Call only after authorization. Version errors propagate so an aborted SQL transaction is never reused. */
export async function readCachedView<T>(
  options: CachedViewOptions,
  version: () => Promise<string | null>,
  load: () => Promise<T>,
): Promise<T> {
  // Clerk membership changes are external to the SQL revision ledger.
  const scope = currentDatabaseScope();
  if (scope && ((scope.kind !== "mail-key" && scope.clerk) || (scope.kind === "session" && scope.memberships))) return load();
  const runtime = configured;
  if (!runtime) return load();
  const ttlSeconds = options.ttlSeconds ?? runtime.ttlSeconds;
  if (!Number.isInteger(ttlSeconds) || ttlSeconds < 1 || ttlSeconds > 300)
    throw Error("Invalid view TTL");
  const revision = await version();
  if (revision === null) return load();
  if (!revision) throw Error("Missing view cache revision");
  const key =
    namespace +
    hash(runtime.environment) +
    ":" +
    options.kind +
    ":" +
    hash(canonical(options.scope)) +
    ":" +
    hash(canonical(options.query)) +
    ":" +
    hash(revision);
  return runtime.cache.readKey(key, ttlSeconds, load);
}
