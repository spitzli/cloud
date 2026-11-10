import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { Queue } from "bullmq";
import { startCoreJobWorkers } from "@spitzli/core/jobs";
const port = Number(process.env.WEBDOCK_TEST_REDIS_PORT);
test(
  "Core BullMQ workers register durable schedulers and keep domain ownership",
  { skip: !port, timeout: 15000 },
  async () => {
    const prefix = "core-test-" + randomUUID();
    const connection = { host: "127.0.0.1", port };
    let hosting = 0,
      notes = 0;
    const errors: unknown[] = [];
    const first = await startCoreJobWorkers({
      prefix,
      connection,
      intervalMs: 200,
      handlers: {
        hosting: async () => {
          hosting++;
        },
      },
      onError: (e) => errors.push(e),
    });
    const second = await startCoreJobWorkers({
      prefix,
      connection,
      intervalMs: 200,
      handlers: {
        "project-notes": async () => {
          notes++;
        },
      },
      onError: (e) => errors.push(e),
    });
    const inspect = new Queue("core-hosting", { prefix, connection });
    try {
      for (let i = 0; i < 50 && (hosting === 0 || notes === 0); i++)
        await new Promise((r) => setTimeout(r, 50));
      assert.ok(hosting > 0 && notes > 0);
      assert.equal((await inspect.getJobSchedulers()).length, 1);
      await first.close();
      const count = hosting;
      await new Promise((r) => setTimeout(r, 400));
      assert.equal(hosting, count);
      const replacement = await startCoreJobWorkers({
        prefix,
        connection,
        intervalMs: 200,
        handlers: {
          hosting: async () => {
            hosting++;
          },
        },
        onError: (e) => errors.push(e),
      });
      try {
        for (let i = 0; i < 50 && hosting === count; i++)
          await new Promise((r) => setTimeout(r, 50));
        assert.ok(hosting > count);
        assert.equal((await inspect.getJobSchedulers()).length, 1);
      } finally {
        await replacement.close();
      }
      assert.deepEqual(errors, []);
    } finally {
      await first.close();
      await second.close();
      await inspect.close();
    }
  },
);
test(
  "Core startup fails and releases sockets when Redis is unavailable",
  { skip: !port, timeout: 13000 },
  async () => {
    const started = Date.now();
    await assert.rejects(
      () =>
        startCoreJobWorkers({
          prefix: "unavailable-" + randomUUID(),
          connection: { host: "127.0.0.1", port: 1 },
          handlers: { hosting: async () => {} },
          onError: () => {},
        }),
      /readiness timeout/,
    );
    assert.ok(Date.now() - started < 12000);
  },
);
