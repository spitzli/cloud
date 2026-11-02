/** Local HTTP/Redis integration. Static fixture grants are NOT production authorization. */
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { randomBytes, randomUUID } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import assert from "node:assert/strict";
import { createPluginJobs } from "@webdock/core/plugin-jobs";
import { createPluginContext } from "@webdock/plugin-sdk/server";
import { PluginError } from "@webdock/plugin-sdk/contracts";
const dir = resolve(process.argv[2] ?? "");
const port = Number(process.env.WEBDOCK_TEST_REDIS_PORT);
if (!process.argv[2] || !port)
  throw Error(
    "Usage: WEBDOCK_TEST_REDIS_PORT=... bun tools/plugins/test-job-demo.ts <built-plugin-dir>",
  );
const manifest = JSON.parse(
  await readFile(resolve(dir, "tabularium.json"), "utf8"),
);
const scope = {
  pluginID: manifest.name,
  customerID: "fixture-customer",
  projectID: "fixture-project",
  runtimeGeneration: "1",
  grantRevision: "1",
};
const token = randomBytes(32).toString("hex"),
  runtimeToken = randomBytes(32).toString("hex");
const expires = Date.now() + 60_000;
let pluginOrigin = "";
const service = createPluginJobs({
  prefix: "demo-" + randomUUID(),
  connection: { host: "127.0.0.1", port },
  authority: {
    authorize: async (req, cap) => {
      if (
        req.headers.get("authorization") !== "Bearer " + token ||
        Date.now() > expires ||
        !manifest.capabilities.includes(cap)
      )
        throw new PluginError("permission_denied", "Fixture invocation denied");
      return {
        scope,
        jobs: manifest.jobs.map((j: { name: string }) => j.name),
      };
    },
    canRun: async (s, name) =>
      Date.now() < expires &&
      JSON.stringify(s) === JSON.stringify(scope) &&
      manifest.jobs.some((j: { name: string }) => j.name === name),
  },
  execute: async (s, name, payload, { signal }) => {
    const r = await fetch(pluginOrigin + "/webdock/jobs", {
      method: "POST",
      signal,
      headers: {
        Authorization: "Bearer " + runtimeToken,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ name, payload }),
    });
    if (!r.ok) throw Error("Plugin execution failed");
    return r.json();
  },
});
const host = createServer(async (req, res) => {
  try {
    const chunks: Buffer[] = [];
    let n = 0;
    for await (const c of req) {
      n += c.length;
      if (n > 36 * 1024) {
        res.writeHead(413);
        res.end();
        return;
      }
      chunks.push(c);
    }
    const r = await service.handle(
      new Request("http://127.0.0.1" + req.url, {
        method: req.method,
        headers: req.headers as Record<string, string>,
        body: chunks.length ? Buffer.concat(chunks) : undefined,
      }),
    );
    res.writeHead(r.status, Object.fromEntries(r.headers));
    res.end(await r.text());
  } catch {
    res.writeHead(500);
    res.end();
  }
});
await new Promise<void>((r) => host.listen(0, "127.0.0.1", r));
const hostOrigin = "http://127.0.0.1:" + (host.address() as any).port;
const child = spawn(process.execPath, ["dist/index.js"], {
  cwd: dir,
  env: {
    ...process.env,
    PORT: "0",
    HOST: "127.0.0.1",
    WEBDOCK_PLUGIN_HOST_URL: hostOrigin,
    WEBDOCK_PLUGIN_RUNTIME_TOKEN: runtimeToken,
  },
  stdio: ["ignore", "pipe", "pipe"],
});
let worker: ReturnType<typeof service.startWorker> | undefined;
try {
  const pluginPort = await new Promise<number>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(Error("Plugin startup timeout")),
      10000,
    );
    let buffer = "";
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(Error("Plugin exited " + code));
    });
    child.stdout.on("data", (chunk) => {
      buffer += chunk;
      const line = buffer.split("\n")[0];
      try {
        const value = JSON.parse(line);
        if (value.port) {
          clearTimeout(timer);
          resolve(value.port);
        }
      } catch {}
    });
  });
  pluginOrigin = "http://127.0.0.1:" + pluginPort;
  assert.equal((await fetch(pluginOrigin + "/healthz")).status, 200);
  assert.equal(
    (
      await fetch(pluginOrigin + "/webdock/jobs", {
        method: "POST",
        body: "{}",
      })
    ).status,
    401,
  );
  worker = service.startWorker();
  await worker.waitUntilReady();
  const body = {
    name: "demo.enqueue",
    payload: { message: "hello from bullmq" },
    idempotencyKey: "first-demo-job",
  };
  const invoke = () =>
    fetch(pluginOrigin + "/webdock/actions", {
      method: "POST",
      headers: {
        Authorization: "Bearer " + runtimeToken,
        "x-plugin-invocation": token,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body),
    });
  const response = await invoke();
  assert.equal(response.status, 200);
  const { id } = (await response.json()) as { id: string };
  const jobs = createPluginContext({
    hostURL: hostOrigin,
    token: () => token,
  }).jobs;
  let result;
  for (let i = 0; i < 100; i++) {
    result = await jobs.get(id);
    if (result.state === "completed") break;
    await new Promise((r) => setTimeout(r, 50));
  }
  assert.equal(result?.state, "completed");
  assert.equal((result.result as any).message, "HELLO FROM BULLMQ");
  assert.equal(((await (await invoke()).json()) as any).id, id);
  console.log(
    JSON.stringify(
      {
        plugin: manifest.name,
        transport: "HTTP",
        queue: "BullMQ 6 / real Redis",
        separatePluginProcess: true,
        completed: true,
        idempotentEnqueue: true,
        result: result.result,
      },
      null,
      2,
    ),
  );
} finally {
  child.kill("SIGTERM");
  await worker?.close();
  await service.close();
  host.closeAllConnections();
  await new Promise<void>((r) => host.close(() => r()));
}
