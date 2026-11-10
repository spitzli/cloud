import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import type { ManagedRuntimeAccess, RuntimeAccess } from "@spitzli/database-contracts";
import { GatewayError } from "./policy";

export type ManagedRuntime = RuntimeAccess & { authenticationOrigin: string };
export interface ManagedRuntimeManager {
  acquire(sessionID: string, access: ManagedRuntimeAccess): Promise<ManagedRuntime>;
  release(sessionID: string): Promise<void>;
  shutdown(): Promise<void>;
}
interface Options {
  executable: string;
  temporaryDirectory?: string;
  webRoot?: string;
  capacity?: number;
  startupTimeoutMs?: number;
}
interface Entry {
  identity: string;
  starting: Promise<ManagedRuntime>;
  stopped: boolean;
  directory?: string;
  child?: ChildProcess;
  exited?: Promise<void>;
  cleanup?: Promise<void>;
  expiry?: NodeJS.Timeout;
}

// Include identity as well as revision so an existing session can never change owner or target.
export const managedIdentity = (access: ManagedRuntimeAccess) => JSON.stringify([access.scopeID, access.subject, access.connectionID, access.profile, access.revision]);

async function availablePort() {
  const server = createServer();
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  if (!address || typeof address === "string") throw Error();
  return address.port;
}

export function createManagedRuntimeManager(options: Options): ManagedRuntimeManager {
  const capacity = options.capacity ?? 8;
  if (!Number.isInteger(capacity) || capacity < 1 || capacity > 64) throw Error("Invalid managed runtime capacity");
  const entries = new Map<string, Entry>();
  let stopped = false;
  const cleanup = (entry: Entry) => entry.cleanup ??= (async () => {
    clearTimeout(entry.expiry);
    if (entry.child && entry.child.exitCode === null && entry.child.signalCode === null) {
      entry.child.kill("SIGTERM");
      const force = setTimeout(() => entry.child?.kill("SIGKILL"), 1000);
      try { await entry.exited; } finally { clearTimeout(force); }
    }
    if (entry.directory) await rm(entry.directory, { recursive: true, force: true });
  })();
  const start = async (entry: Entry, access: ManagedRuntimeAccess): Promise<ManagedRuntime> => {
    try {
      entry.directory = await mkdtemp(join(options.temporaryDirectory ?? tmpdir(), "webdock-tabularis-"));
      const ca = join(entry.directory, "ca.crt");
      const database = access.database;
      await Promise.all([
        writeFile(ca, database.caCertificate, { mode: 0o600 }),
        writeFile(join(entry.directory, "connections.json"), JSON.stringify([{ id: access.connectionID, name: database.database, params: {
          driver: "postgres", host: database.host, port: database.port, database: database.database,
          username: database.username, password: database.password, ssl_mode: "verify-full", ssl_ca: ca, save_in_keychain: false,
        } }]), { mode: 0o600 }),
        writeFile(join(entry.directory, "config.json"), JSON.stringify({ checkForUpdates: false, autoCheckUpdatesOnStartup: false }), { mode: 0o600 }),
      ]);
      if (entry.stopped) throw Error();
      const port = await availablePort();
      const runtimeOrigin = `http://127.0.0.1:${port}`;
      // Tabularis requires HTTPS proxy origins even when the internal transport is loopback HTTP.
      const authenticationOrigin = `https://127.0.0.1:${port}`;
      const proxySecret = randomBytes(32).toString("base64url");
      const args = ["web", "--host", "127.0.0.1", "--port", String(port), "--auth", "proxy", "--no-open", "--public-url", authenticationOrigin, "--allowed-origin", authenticationOrigin];
      if (options.webRoot) args.push("--web-root", options.webRoot);
      if (entry.stopped) throw Error();
      entry.child = spawn(options.executable, args, {
        cwd: entry.directory, stdio: "ignore", env: {
          PATH: process.env.PATH, LANG: "C.UTF-8", RUST_LOG: "off",
          TABULARIS_DATA_DIR: entry.directory, TABULARIS_WEB_PROXY_SECRET: proxySecret, TMPDIR: entry.directory,
          XDG_CONFIG_HOME: entry.directory, XDG_DATA_HOME: entry.directory, XDG_CACHE_HOME: entry.directory,
        },
      });
      let exited = false;
      entry.exited = new Promise(resolve => {
        entry.child!.once("error", () => { exited = true; resolve(); });
        entry.child!.once("exit", () => { exited = true; resolve(); });
      });
      const deadline = Date.now() + (options.startupTimeoutMs ?? 10_000);
      while (Date.now() < deadline && !entry.stopped && !exited) {
        try {
          const response = await fetch(runtimeOrigin + "/healthz", { redirect: "error", signal: AbortSignal.timeout(250) });
          await response.body?.cancel();
          if (response.ok && !entry.stopped && !exited) {
            return { scopeID: access.scopeID, subject: access.subject, connectionID: access.connectionID, profile: access.profile,
              expiresAt: access.expiresAt, runtimeOrigin, proxySecret, authenticationOrigin };
          }
        } catch { /* The child has not bound its listener yet. */ }
        await delay(25);
      }
      throw Error();
    } catch {
      await cleanup(entry);
      throw new GatewayError(503, "Managed database runtime could not start.");
    }
  };
  const release = async (sessionID: string) => {
    const entry = entries.get(sessionID); if (!entry) return;
    entry.stopped = true;
    await entry.starting.catch(() => {});
    await cleanup(entry);
    if (entries.get(sessionID) === entry) entries.delete(sessionID);
  };
  return {
    async acquire(sessionID, access) {
      if (stopped) throw new GatewayError(503, "Managed database runtime unavailable.");
      const existing = entries.get(sessionID);
      if (existing) {
        if (existing.identity !== managedIdentity(access)) throw new GatewayError(401, "Managed database access changed. Open a new session.");
        if (existing.stopped || existing.child?.exitCode != null || existing.child?.signalCode != null) throw new GatewayError(503, "Managed database runtime unavailable.");
        return existing.starting;
      }
      if (entries.size >= capacity) throw new GatewayError(503, "Managed database runtime capacity reached.");
      const expires = new Date(access.expiresAt).getTime();
      if (!Number.isFinite(expires) || expires <= Date.now()) throw new GatewayError(401, "Database session expired.");
      const entry = { identity: managedIdentity(access), stopped: false } as Entry;
      entries.set(sessionID, entry);
      entry.starting = start(entry, access).catch(error => { if (entries.get(sessionID) === entry) entries.delete(sessionID); throw error; });
      entry.expiry = setTimeout(() => { void release(sessionID); }, Math.min(expires - Date.now(), 2_147_483_647));
      entry.expiry.unref();
      return entry.starting;
    },
    release,
    async shutdown() { stopped = true; await Promise.all([...entries.keys()].map(release)); },
  };
}
