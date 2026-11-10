import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { randomUUID } from "node:crypto";
import { createPluginJobs, type JobScope } from "../../src/plugin-jobs.ts";
import { createPluginContext } from "@spitzli/plugin-sdk/server";
import { PluginError } from "@spitzli/plugin-sdk/contracts";
const port = Number(process.env.SPITZLI_TEST_REDIS_PORT);
const pause = (ms: number) => new Promise((r) => setTimeout(r, ms));
test(
  "real Redis + HTTP: scoped enqueue, retry, deduplication, cancellation and revocation",
  { skip: !port, timeout: 25000 },
  async () => {
    const scope: JobScope = {
      pluginID: "job-demo",
      customerID: "1",
      projectID: "2",
      runtimeGeneration: "1",
      grantRevision: "1",
    };
    const grants = new Map([
      ["allowed", scope],
      ["readonly", scope],
      ["foreign", { ...scope, projectID: "3" }],
    ]);
    let allowed = true,
      calls = 0,
      failures = 0;
    const prefix = "fixture-" + randomUUID();
    const service = createPluginJobs({
      connection: {
        host: "127.0.0.1",
        port,
        maxRetriesPerRequest: 1,
        enableOfflineQueue: false,
      },
      prefix,
      authority: {
        authorize: async (request, capability) => {
          const token = request.headers
            .get("authorization")
            ?.replace(/^Bearer /, "");
          const current = grants.get(token ?? "");
          if (
            !current ||
            !allowed ||
            (token === "readonly" && capability !== "plugin.jobs.read")
          )
            throw new PluginError("permission_denied", "Denied");
          return {
            scope: current,
            jobs: ["demo.process", "demo.retry", "demo.slow"],
          };
        },
        canRun: async () => allowed,
      },
      execute: async (_scope, name, payload, { signal }) => {
        calls++;
        if (name === "demo.retry" && failures++ === 0)
          throw Error("secret must not escape");
        if (name === "demo.slow")
          await new Promise((resolve, reject) => {
            const t = setTimeout(resolve, 5000);
            signal.addEventListener(
              "abort",
              () => {
                clearTimeout(t);
                reject(Error("aborted"));
              },
              { once: true },
            );
          });
        return { processed: payload };
      },
    });
    const server = createServer(async (req, res) => {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const out = await service.handle(
        new Request("http://localhost" + req.url, {
          method: req.method,
          headers: req.headers as Record<string, string>,
          body: chunks.length ? Buffer.concat(chunks) : undefined,
        }),
      );
      res.writeHead(out.status, Object.fromEntries(out.headers));
      res.end(await out.text());
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    const origin = "http://127.0.0.1:" + (server.address() as any).port;
    const client = (token: string) =>
      createPluginContext({ hostURL: origin, token: () => token }).jobs;
    const jobs = client("allowed");
    let worker: ReturnType<typeof service.startWorker> | undefined;
    async function state(id: string, expected: string) {
      for (let i = 0; i < 100; i++) {
        const j = await jobs.get(id);
        if (j.state === expected) return j;
        await pause(50);
      }
      assert.fail("Timed out: " + expected);
    }
    try {
      await assert.rejects(() =>
        client("forged").enqueue({
          name: "demo.process",
          payload: 1,
          idempotencyKey: "denied-1",
        }),
      );
      await assert.rejects(() =>
        client("readonly").enqueue({
          name: "demo.process",
          payload: 1,
          idempotencyKey: "read-only",
        }),
      );
      const input = {
        name: "demo.process",
        payload: { message: "hello" },
        idempotencyKey: "request-one",
      };
      const [{ id }, duplicate] = await Promise.all([
        jobs.enqueue(input),
        jobs.enqueue(input),
      ]);
      assert.equal(id, duplicate.id);
      await assert.rejects(
        () => jobs.enqueue({ ...input, payload: { message: "changed" } }),
        /Idempotency/,
      );
      await assert.rejects(() => client("foreign").get(id), /not found/);
      await assert.rejects(() => client("foreign").cancel(id), /not found/);
      await assert.rejects(() =>
        jobs.enqueue({
          ...input,
          name: "undeclared",
          idempotencyKey: "unknown-job",
        }),
      );
      const delayed = await jobs.enqueue({
        ...input,
        idempotencyKey: "cancel-me",
        delaySeconds: 60,
      });
      await jobs.cancel(delayed.id);
      assert.equal((await jobs.get(delayed.id)).state, "cancelled");
      worker = service.startWorker();
      await worker.waitUntilReady();
      assert.deepEqual((await state(id, "completed")).result, {
        processed: { message: "hello" },
      });
      assert.equal(calls, 1);
      assert.equal((await jobs.enqueue(input)).id, id);
      await pause(100);
      assert.equal(calls, 1);
      const retry = await jobs.enqueue({
        ...input,
        name: "demo.retry",
        idempotencyKey: "retry-once",
      });
      assert.equal((await state(retry.id, "completed")).attempts, 2);
      const slow = await jobs.enqueue({
        ...input,
        name: "demo.slow",
        idempotencyKey: "cancel-active",
      });
      await state(slow.id, "active");
      await jobs.cancel(slow.id);
      await state(slow.id, "cancelled");
      const revoked = await jobs.enqueue({
        ...input,
        idempotencyKey: "revoke-later",
        delaySeconds: 1,
      });
      const before = calls;
      allowed = false;
      await assert.rejects(() => jobs.get(id));
      await pause(1500);
      allowed = true;
      await state(revoked.id, "failed");
      assert.equal(calls, before);
    } finally {
      await worker?.close();
      await service.close();
      server.closeAllConnections();
      await new Promise<void>((r) => server.close(() => r()));
    }
  },
);
