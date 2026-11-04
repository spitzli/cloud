import test from "node:test";
import assert from "node:assert/strict";
import { jobIdentity } from "../../src/plugin-jobs.ts";
import { enqueueJobSchema } from "@spitzli/plugin-sdk/contracts";
const scope = {
  pluginID: "job-demo",
  customerID: "1",
  projectID: "2",
  runtimeGeneration: "1",
  grantRevision: "1",
};
test("identities isolate tenant, project and installation; retries retain identity", () => {
  assert.equal(
    jobIdentity(scope, "request-one"),
    jobIdentity(scope, "request-one"),
  );
  for (const change of [
    { customerID: "2" },
    { projectID: "3" },
    { pluginID: "another" },
  ])
    assert.notEqual(
      jobIdentity(scope, "request-one"),
      jobIdentity({ ...scope, ...change }, "request-one"),
    );
  assert.match(jobIdentity(scope, "request-one"), /^job-[a-f0-9]{64}$/);
});
test("queue contract forbids caller-selected scope, oversized payloads and delays", () => {
  const good = {
    name: "demo.process",
    payload: { message: "hello" },
    idempotencyKey: "request-one",
  };
  assert.equal(enqueueJobSchema.safeParse(good).success, true);
  for (const change of [
    { customerID: "evil" },
    { delaySeconds: 604801 },
    { attempts: 99 },
    { payload: "x".repeat(32769) },
    { name: "../run" },
  ])
    assert.equal(
      enqueueJobSchema.safeParse({ ...good, ...change }).success,
      false,
    );
});

test("absolute scheduling supports stable retries and rejects mixed/out-of-range delay", () => {
  const base = {
    name: "demo.process",
    payload: {},
    idempotencyKey: "reminder-key",
    runAt: new Date(Date.now() + 60000).toISOString(),
  };
  assert.equal(enqueueJobSchema.safeParse(base).success, true);
  assert.equal(
    enqueueJobSchema.safeParse({ ...base, delaySeconds: 5 }).success,
    false,
  );
  assert.equal(
    enqueueJobSchema.safeParse({
      ...base,
      runAt: new Date(Date.now() + 8 * 86400000).toISOString(),
    }).success,
    false,
  );
});
