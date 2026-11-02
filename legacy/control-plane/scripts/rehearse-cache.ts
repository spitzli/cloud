/** Disposable local-container verification. Never reads deployment environment files. */
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";
import { RedisClient } from "bun";
import { connectViewCache } from "../src/cache-redis";
import { configureViewCache, readCachedView } from "../src/cache";
const endpoint =
  process.env.DOCKER_HOST ||
  execFileSync(
    "docker",
    [
      "context",
      "inspect",
      ...(process.env.DOCKER_CONTEXT ? [process.env.DOCKER_CONTEXT] : []),
      "--format",
      "{{.Endpoints.docker.Host}}",
    ],
    { encoding: "utf8" },
  ).trim();
if (!endpoint.startsWith("unix://"))
  throw Error("Local Docker engine required");
const env: NodeJS.ProcessEnv = { ...process.env, DOCKER_HOST: endpoint };
delete env.DOCKER_CONTEXT;
const docker = (args: string[]) =>
  execFileSync("docker", args, {
    encoding: "utf8",
    env,
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
const directory = await mkdtemp(join(tmpdir(), "webdock-cache-")),
  password = randomBytes(32).toString("hex"),
  name = "webdock-cache-fixture-" + randomBytes(6).toString("hex");
const file = join(directory, "cache.env");
await writeFile(file, "DFLY_requirepass=" + password + "\n", { mode: 0o600 });
let created = false,
  client: ReturnType<typeof connectViewCache> | undefined;
try {
  docker([
    "run",
    "-d",
    "--name",
    name,
    "--read-only",
    "--user",
    "65532:65532",
    "--cap-drop=ALL",
    "--security-opt=no-new-privileges",
    "--memory=512m",
    "--cpus=1",
    "--tmpfs",
    "/tmp:rw,noexec,nosuid,size=16m",
    "--env-file",
    file,
    "-p",
    "127.0.0.1::6379",
    "docker.dragonflydb.io/dragonflydb/dragonfly:v2.0.2@sha256:061f00b201b58e12f974e4e96c761a881ee1c05e9d8ee967466a604b7299753b",
    "dragonfly",
    "--logtostderr",
    "--proactor_threads=1",
    "--maxmemory=256MiB",
    "--cache_mode=true",
    "--version_check=false",
    "--snapshot_cron=",
    "--dir=/tmp",
  ]);
  created = true;
  const port = docker(["port", name, "6379/tcp"]).split(":").at(-1);
  await Bun.sleep(500);
  const anonymous = new RedisClient(`redis://127.0.0.1:${port}`, {
    connectionTimeout: 500,
    maxRetries: 0,
    autoReconnect: false,
  });
  let denied = false;
  try {
    await anonymous.connect();
    await anonymous.get("webdock:view:fixture");
  } catch {
    denied = true;
  } finally {
    anonymous.close();
  }
  if (!denied) throw Error("Unauthenticated cache access was accepted");
  client = connectViewCache(`redis://default:${password}@127.0.0.1:${port}`);
  const key = {
    kind: "branding" as const,
    environment: "fixture",
    customerID: "public",
    resourceID: "branding",
    revision: "1",
  };
  // Initial requests may use different keys before the adapter connects.
  const coldCounts = new Map<number, number>();
  const coldRead = (n: number) =>
    client!.cache.read({ ...key, resourceID: `cold-${n}` }, async () => ({
      value: coldCounts.set(n, (coldCounts.get(n) ?? 0) + 1).get(n),
    }));
  await Promise.all(Array.from({ length: 8 }, (_, n) => coldRead(n)));
  await Promise.all(Array.from({ length: 8 }, (_, n) => coldRead(n)));
  if ([...coldCounts.values()].some((count) => count !== 1))
    throw Error("Concurrent initial cache connections did not produce hits");
  let reads = 0;
  const load = async () => ({ value: ++reads });
  const first = await client.cache.read(key, load),
    second = await client.cache.read(key, load);
  if (first.value !== 1 || second.value !== 1 || reads !== 1)
    throw Error("Cache miss/hit failed");
  configureViewCache(client.cache, {
    environment: "fixture-api",
    defaultTTLSeconds: 60,
  });
  const view = {
    kind: "operations" as const,
    scope: { subject: "fixture-user", sessionID: "fixture-session" },
    query: { page: 1 },
    ttlSeconds: 1,
  };
  let revision = "one";
  const read = () => readCachedView(view, async () => revision, load);
  if ((await read()).value !== 2 || (await read()).value !== 2)
    throw Error("Shared view cache hit failed");
  revision = "two";
  if ((await read()).value !== 3)
    throw Error("Shared view revision invalidation failed");
  await Bun.sleep(1100);
  if ((await read()).value !== 4) throw Error("Dragonfly TTL did not expire");
  docker(["stop", "-t", "1", name]);
  const start = Date.now();
  const third = await read();
  if (third.value !== 5 || Date.now() - start > 500)
    throw Error("Outage fallback failed");
  console.log(
    "Dragonfly2.0.2: authentication, non-root/read-only runtime, legacy/shared cache hits, revision invalidation, TTL and bounded outage fallback passed.",
  );
} finally {
  configureViewCache(undefined, { environment: "fixture-api" });
  client?.close();
  if (created) docker(["rm", "-f", name]);
  await rm(directory, { recursive: true, force: true });
}
