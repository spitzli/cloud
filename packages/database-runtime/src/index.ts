/** Platform metadata PostgreSQL pools. Customer database bindings never reuse these credentials. */
export type DatabaseConfig = {
  environment: "production" | "development" | "test";
  businessURL: string;
  authURL: string;
  workerURL: string;
  signingSecret?: string;
  scope?: () => DatabaseScope | undefined;
  ca?: string;
  max: number;
};

import { AsyncLocalStorage } from "node:async_hooks";
import { createHmac, X509Certificate } from "node:crypto";
import pg from "pg";

export type ClerkDatabaseIdentity = { operator: boolean; issuer: string; userID: string; organizationID: string | null; clerkOrganizationID: string | null; customerID: string | null; role: "admin" | "member" | null; permissions?: readonly string[]; verifiedAt: number; expiresAt: number };
export type LegacyClerkMemberships={operator:boolean;twoFactorEnabled:boolean;sessionID?:string;subject?:string;issuer:string;userID:string;verifiedAt:number;expiresAt:number;roles:Array<{organizationID:string;customerID:string;role:string;permissions?:readonly string[]}>};
export type DatabaseScope =
  | { kind: "session"; authProvider?: "clerk" | "better-auth"; pluginInvocationID?: string; subject: string; sessionID: string; clerk?: ClerkDatabaseIdentity; memberships?:LegacyClerkMemberships }
  | {
      kind: "system";
      authProvider?: "clerk" | "better-auth";
      memberships?: LegacyClerkMemberships;
      clerk?: ClerkDatabaseIdentity & {subject:string;sessionID:string};
      purpose: string;
      pluginInvocationID?: string;
      participantID?: string;
      clusterID?: string;
      customerID?: string;
      setupID?: string;
      subject?: string;
      sessionID?: string;
      generation?: number;
    }
  | {
      kind: "mail-key";
      customerID: string;
      connectionID: string;
      keyID: string;
    };
const scopes = new AsyncLocalStorage<DatabaseScope>();
export function withDatabaseScope<T>(scope: DatabaseScope, fn: () => T): T {
  const inherited = scopes.getStore();
  // Nested service/registry transactions must retain the exact verified provider proof.
  if (scope.kind === "session" && !scope.clerk && inherited?.kind === "session" && inherited.clerk && scope.subject === inherited.subject && scope.sessionID === inherited.sessionID) {
    scope = { ...scope, clerk: inherited.clerk };
  }
  if(scope.kind==="session" && !scope.memberships && inherited?.kind==="session" && inherited.memberships && scope.subject===inherited.subject && scope.sessionID===inherited.sessionID)scope={...scope,memberships:inherited.memberships};
  if(scope.kind==="session" && inherited?.kind==="session" && scope.subject===inherited.subject && scope.sessionID===inherited.sessionID && inherited.authProvider)scope={...scope,authProvider:inherited.authProvider};
  return scopes.run(structuredClone(scope), fn);
}
export function currentDatabaseScope(): DatabaseScope | undefined {
  const scope = scopes.getStore();
  return scope && structuredClone(scope);
}

/** URL SSL flags override pg's ssl object, so remove them only with an explicit CA. */
export function verifiedPostgresConfig(
  url: string,
  ca?: string,
): { connectionString: string; ssl: { ca?: string; rejectUnauthorized: true } } {
  const parsed = new URL(url);
  if (!["postgres:", "postgresql:"].includes(parsed.protocol))
    throw new Error("Expected a PostgreSQL URL");
  for (const key of [
    "ssl",
    "sslmode",
    "sslcert",
    "sslkey",
    "sslrootcert",
    "uselibpqcompat",
    "sslnegotiation",
  ]) {
    if (parsed.searchParams.getAll(key).length > 1)
      throw new Error("Duplicate SSL URL options are forbidden");
  }
  if (ca !== undefined) {
    const certs = ca.match(
      /-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/g,
    );
    if (
      !certs?.length ||
      ca
        .replace(
          /-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/g,
          "",
        )
        .trim()
    )
      throw new Error("CA must contain PEM certificates");
    for (const cert of certs) new X509Certificate(cert);
    for (const key of [
      "ssl",
      "sslmode",
      "sslcert",
      "sslkey",
      "sslrootcert",
      "uselibpqcompat",
    ])
      parsed.searchParams.delete(key);
    return {
      connectionString: parsed.toString(),
      ssl: { ca, rejectUnauthorized: true },
    };
  }
  const mode = parsed.searchParams.get("sslmode");
  if (
    (mode && !["require", "verify-full"].includes(mode)) ||
    parsed.searchParams.has("ssl") ||
    parsed.searchParams.has("uselibpqcompat")
  ) {
    throw new Error("SSL URL options must require verified TLS");
  }
  // Keep require URLs compatible without allowing pg's URL parser to replace
  // the explicit verification setting with an inferred SSL object.
  parsed.searchParams.delete("sslmode");
  return {
    connectionString: parsed.toString(),
    ssl: { rejectUnauthorized: true },
  };
}

type Options = {
  secret?: string;
  audience: string;
  scope?: () => DatabaseScope | undefined;
};
type ConnectCallback = (
  error: Error | undefined,
  client: pg.PoolClient | undefined,
  release: (error?: Error | boolean) => void,
) => void;
type Callback = (
  error: Error | null,
  result?: any,
  release?: (error?: Error | boolean) => void,
) => void;
function deliver<T>(
  promise: Promise<T>,
  callback?: Callback,
): Promise<T> | undefined {
  if (!callback) return promise;
  void promise.then(
    (result) => callback(null, result),
    (error) => callback(error),
  );
  return undefined;
}
function queryArgs(args: any[]): {
  config: pg.QueryConfig;
  callback?: Callback;
} {
  const [input, values, callback] = args;
  if (
    typeof input !== "string" &&
    (!input ||
      typeof input.text !== "string" ||
      typeof input.submit === "function")
  )
    throw new Error("ScopedPool supports SQL strings and query configs only");
  return {
    config: {
      ...(typeof input === "string" ? { text: input } : input),
      ...(Array.isArray(values) ? { values } : {}),
      queryMode: "extended",
    } as pg.QueryConfig,
    callback:
      typeof values === "function"
        ? values
        : typeof callback === "function"
          ? callback
          : undefined,
  };
}

// Read only command keywords; PostgreSQL allows comments between every keyword,
// including nested block comments. Extended protocol rejects multiple statements.
export function sqlCommandWords(sql: string): string[] {
  const words: string[] = [];
  let offset = 0;
  while (offset < sql.length && words.length < 8) {
    if (/\s/.test(sql.charAt(offset))) {
      offset++;
      continue;
    }
    if (sql.startsWith("--", offset)) {
      const newline = sql.indexOf("\n", offset + 2);
      offset = newline < 0 ? sql.length : newline + 1;
      continue;
    }
    if (sql.startsWith("/*", offset)) {
      let depth = 1;
      offset += 2;
      while (offset < sql.length && depth) {
        if (sql.startsWith("/*", offset)) {
          depth++;
          offset += 2;
        } else if (sql.startsWith("*/", offset)) {
          depth--;
          offset += 2;
        } else offset++;
      }
      continue;
    }
    const word = /^[a-z_][a-z_0-9$]*/i.exec(sql.slice(offset));
    if (!word) break;
    words.push(word[0].toUpperCase());
    offset += word[0].length;
  }
  return words;
}

/** Signed settings are transaction-local, including queries issued outside Payload transactions. */
export class ScopedPool extends pg.Pool {
  private readonly contextOptions: Options;
  constructor(config: pg.PoolConfig, options: Options) {
    super(config);
    if (options.secret && Buffer.byteLength(options.secret, "utf8") < 32)
      throw new Error(
        "ScopedPool signing secret must contain at least 32 bytes",
      );
    if (options.secret && !options.audience)
      throw new Error("ScopedPool requires an audience");
    this.contextOptions = options;
  }

  override connect(): Promise<pg.PoolClient>;
  override connect(callback: ConnectCallback): void;
  override connect(callback?: ConnectCallback): any {
    if (!this.contextOptions.secret)
      return callback ? super.connect(callback) : super.connect();
    const promise = super.connect().then((client) => this.wrap(client));
    if (callback) {
      void promise.then(
        (client) => callback(undefined, client, client.release),
        (error) => callback(error, undefined, () => {}),
      );
      return;
    }
    return promise;
  }

  override query: pg.Pool["query"] = (...args: any[]): any => {
    if (!this.contextOptions.secret) return (super.query as any)(...args);
    let parsed: ReturnType<typeof queryArgs>;
    try {
      parsed = queryArgs(args);
    } catch (error) {
      const cb = args.find((arg) => typeof arg === "function");
      return deliver(Promise.reject(error), cb);
    }
    const promise = (async () => {
      const client = (await this.connect()) as pg.PoolClient;
      try {
        return await client.query(parsed.config);
      } finally {
        client.release();
      }
    })();
    return deliver(promise, parsed.callback);
  };

  private wrap(client: pg.PoolClient): pg.PoolClient {
    const rawQuery = client.query.bind(client);
    const originalQuery = client.query;
    const release = client.release.bind(client);
    let queue: Promise<unknown> = Promise.resolve();
    let transaction = false;
    let fingerprint: string | undefined;
    let released = false;
    let poisoned: Error | undefined;
    const raw = async (config: string | pg.QueryConfig) => {
      if (poisoned) throw poisoned;
      const result = await rawQuery(
        typeof config === "string"
          ? ({ text: config, queryMode: "extended" } as pg.QueryConfig)
          : config,
      );
      // Destruction can race a successful query response; never follow it with COMMIT.
      if (poisoned) throw poisoned;
      return result;
    };
    const install = async (scope: DatabaseScope | undefined) => {
      const context = scope
        ? JSON.stringify({
            v: 1,
            aud: this.contextOptions.audience,
            exp: Math.floor(Date.now() / 1000) + 60,
            scope,
          })
        : "";
      const signature = context
        ? createHmac("sha256", this.contextOptions.secret!)
            .update(context)
            .digest("hex")
        : "";
      await raw({
        text: "SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",
        values: [context, signature],
      });
    };
    const rollback = async () => {
      try {
        await raw("ROLLBACK");
      } catch (error) {
        poisoned = error as Error;
      }
      transaction = false;
      fingerprint = undefined;
    };
    client.query = ((...args: any[]) => {
      let parsed: ReturnType<typeof queryArgs>;
      try {
        parsed = queryArgs(args);
      } catch (error) {
        return deliver(
          Promise.reject(error),
          args.find((arg) => typeof arg === "function"),
        );
      }
      const scope = structuredClone(
        (this.contextOptions.scope || currentDatabaseScope)(),
      );
      const identity = JSON.stringify(scope);
      const run = async () => {
        if (poisoned) throw poisoned;
        const words = sqlCommandWords(parsed.config.text);
        const begin =
          words[0] === "BEGIN" ||
          (words[0] === "START" && words[1] === "TRANSACTION");
        const rollbackTarget =
          words[1] === "WORK" || words[1] === "TRANSACTION"
            ? words[2]
            : words[1];
        const end =
          ["COMMIT", "END", "ABORT"].includes(words[0] ?? "") ||
          (words[0] === "ROLLBACK" && rollbackTarget !== "TO");
        const inheritedTransaction = transaction;
        try {
          if (
            (end && words.includes("CHAIN")) ||
            (words[0] === "PREPARE" && words[1] === "TRANSACTION") ||
            (["COMMIT", "ROLLBACK"].includes(words[0] ?? "") &&
              words[1] === "PREPARED")
          )
            throw new Error(
              "Chained and prepared transactions are unsupported by ScopedPool",
            );
          if (transaction && identity !== fingerprint)
            throw new Error("Database scope changed within a transaction");
          if (begin) {
            if (transaction) throw new Error("Transaction already active");
            transaction = true;
            fingerprint = identity;
            const result = await raw(parsed.config);
            await install(scope);
            return result;
          }
          if (transaction) {
            const result = await raw(parsed.config);
            if (end) {
              transaction = false;
              fingerprint = undefined;
            }
            return result;
          }
          if (end) return await raw(parsed.config);
          transaction = true;
          fingerprint = identity;
          await raw("BEGIN");
          await install(scope);
          const result = await raw(parsed.config);
          await raw("COMMIT");
          transaction = false;
          fingerprint = undefined;
          return result;
        } catch (error) {
          // Explicit transaction errors remain available for ROLLBACK TO SAVEPOINT.
          if (transaction && !inheritedTransaction) await rollback();
          if (end && inheritedTransaction) poisoned = error as Error;
          throw error;
        }
      };
      const pending = released
        ? Promise.reject(new Error("Client already released"))
        : queue.then(run);
      queue = pending.catch(() => undefined);
      return deliver(pending, parsed.callback);
    }) as typeof client.query;
    client.release = (error?: Error | boolean) => {
      if (released) throw new Error("Client already released");
      released = true;
      if (error) {
        // A deadline/error release cancels now, rather than waiting for queued SQL to commit.
        poisoned =
          error instanceof Error
            ? error
            : new Error(
                "Database client destroyed before pending work completed",
              );
        release(error);
        return;
      }
      void queue.then(async () => {
        if (transaction) await rollback();
        client.query = originalQuery;
        release(error || poisoned);
      });
    };
    return client;
  }
}

export { retainedMailChecks, assertMailCanDisable } from "./legacy-retention";
