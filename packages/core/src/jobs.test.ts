import test from "node:test";
import assert from "node:assert/strict";
import { readJobQueueConfiguration, startCoreJobWorkers } from "./jobs.ts";
test("core jobs require explicit Redis and isolate their prefix", () => {
  assert.throws(() => readJobQueueConfiguration({}), /REDIS/);
  for (const url of [
    "http://localhost",
    "redis://localhost/1?x=y",
    "redis://localhost/not-a-db",
  ])
    assert.throws(() =>
      readJobQueueConfiguration({ SPITZLI_JOBS_REDIS_URL: url }),
    );
  const config = readJobQueueConfiguration({
    SPITZLI_JOBS_REDIS_URL: "rediss://worker:secret@redis.example:6380/2",
    SPITZLI_JOBS_PREFIX: "webdock-test",
  });
  assert.equal(config.connection.host, "redis.example");
  assert.equal(config.connection.db, 2);
  assert.deepEqual(config.connection.tls, {});
  assert.equal(config.prefix, "webdock-test");
});

test("worker ownership accepts registered extension names and validates all names before connecting", async () => {
  await assert.rejects(() => startCoreJobWorkers({
    connection: {host: "127.0.0.1", port: 1}, prefix: "test",
    handlers: {"notes:index": async () => {}}, onError: () => {},
  }), /Invalid Core worker ownership/);
  const empty = await startCoreJobWorkers({connection: {host: "127.0.0.1", port: 1}, prefix: "test", handlers: {}, onError: () => {}});
  await empty.close();
});
