import assert from "node:assert/strict";
import test from "node:test";
import { handleMailCron } from "../../src/modules/mail/product/cron";
test("cron secrets never enable copied preview jobs and only exact production origin is accepted", async () => {
  const env = {
    NODE_ENV: "production",
    VERCEL_ENV: "production",
    BETTER_AUTH_URL: "https://auth.example.com",
    CRON_SECRET: "fixture-secret",
  };
  let runs = 0;
  const run = async (
    job: "dispatch" | "events" | "webhooks" | "maintenance",
  ) => {
    runs++;
    return { job, processed: 0, durationMs: 0 };
  };
  const request = new Request(
    "https://auth.example.com/api/cron/mail/dispatch",
    { headers: { authorization: "Bearer fixture-secret" } },
  );
  assert.equal(
    (
      await handleMailCron(request, "dispatch", {
        env: { ...env, VERCEL_ENV: "preview" },
        run,
      })
    ).status,
    503,
  );
  assert.equal(runs, 0);
  assert.equal(
    (
      await handleMailCron(
        new Request("https://auth.example.com/api/cron/mail/dispatch"),
        "dispatch",
        { env, run },
      )
    ).status,
    401,
  );
  assert.equal(
    (await handleMailCron(request, "dispatch", { env, run })).status,
    200,
  );
  assert.equal(runs, 1);
  assert.equal(
    (
      await handleMailCron(
        new Request("https://preview.example.com/api/cron/mail/dispatch", {
          headers: { authorization: "Bearer fixture-secret" },
        }),
        "dispatch",
        { env, run },
      )
    ).status,
    503,
  );
});
