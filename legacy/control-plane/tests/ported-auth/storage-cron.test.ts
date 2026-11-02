import assert from "node:assert/strict";
import test from "node:test";
import { GET } from "../../src/protocols/legacy-auth/api/cron/storage-usage/route";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";

test.after(async () => {
  await auth.$context;
  await database.end();
});
test("storage cron rejects missing configuration, absent credentials and wrong credentials", async () => {
  const previous = process.env.CRON_SECRET;
  try {
    delete process.env.CRON_SECRET;
    assert.equal(
      (await GET(new Request("http://localhost/api/cron/storage-usage")))
        .status,
      401,
    );
    process.env.CRON_SECRET = "local-cron-fixture";
    for (const authorization of [
      "",
      "Bearer wrong",
      "Bearer local-cron-fixturf",
    ]) {
      assert.equal(
        (
          await GET(
            new Request("http://localhost/api/cron/storage-usage", {
              headers: { authorization },
            }),
          )
        ).status,
        401,
      );
    }
  } finally {
    if (previous === undefined) delete process.env.CRON_SECRET;
    else process.env.CRON_SECRET = previous;
  }
});
